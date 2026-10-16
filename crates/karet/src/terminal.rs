//! The integrated terminal: a shell on a pseudo-terminal, emulated in-process.
//!
//! Each [`TerminalPane`] owns one shell started on a `portable-pty` PTY whose
//! output is parsed by an `alacritty_terminal` [`Term`]. Two threads per pane
//! keep the UI loop from ever blocking on the shell: a reader feeds PTY output
//! through the VT parser into the shared grid, and a writer drains the bytes the
//! UI queued (keys, pastes, mouse reports, the emulator's own replies). A third
//! waits on the child so its exit status reaches the tab.
//!
//! The UI learns about new output through one [`TerminalEvent`] channel shared by
//! every pane ([`TerminalHost::take_receiver`]), and repaints from the grid under
//! a brief lock. Output is coalesced: a pane announces new output once, and only
//! announces again after the UI has acknowledged it ([`TerminalPane::acknowledge_output`]),
//! so a shell printing as fast as it can never floods the channel.
//!
//! Everything here is behind the `karet` app's default-on `terminal` feature.

mod input;
mod selection;

use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use alacritty_terminal::event::Event as TermEvent;
use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::term::Term;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::Processor;
pub(crate) use input::MouseAction;
pub(crate) use input::MouseButtonKind;
pub(crate) use input::MouseReport;
pub(crate) use input::encode_key;
pub(crate) use input::encode_mouse;
pub(crate) use input::encode_paste;
use portable_pty::ChildKiller;
use portable_pty::CommandBuilder;
use portable_pty::MasterPty;
use portable_pty::PtySize;
use portable_pty::native_pty_system;
use ratatui::layout::Rect;
pub(crate) use selection::ClickTracker;
pub(crate) use selection::selection_type;
use tokio::sync::mpsc;

/// How many lines of history each terminal keeps above the screen.
pub(crate) const SCROLLBACK_LINES: usize = 10_000;

/// Lines one mouse-wheel notch scrolls the history (or the arrows it sends to a
/// full-screen program on the alternate screen).
pub(crate) const WHEEL_LINES: i32 = 3;

/// The smallest grid a pane is ever sized to, so a collapsed split never asks the
/// PTY for a zero-sized window (which some shells treat as an error).
const MIN_COLUMNS: u16 = 2;
const MIN_ROWS: u16 = 1;

/// Identifies one terminal for the lifetime of the app (never reused).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TerminalId(pub(crate) u64);

/// Something a terminal's background threads tell the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TerminalEventKind {
    /// New output was parsed into the grid; repaint.
    Output,
    /// The program set its window title (OSC 0/2).
    Title(String),
    /// The program reset its window title.
    ResetTitle,
    /// The program asked to put text on the clipboard (OSC 52 copy).
    Clipboard(String),
    /// The program rang the bell.
    Bell,
    /// The shell exited; `None` when its status could not be read.
    Exited(Option<String>),
}

/// A [`TerminalEventKind`] tagged with the terminal it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TerminalEvent {
    /// The terminal that produced it.
    pub(crate) id: TerminalId,
    /// What happened.
    pub(crate) kind: TerminalEventKind,
}

/// The receiving end the UI loop selects over.
pub(crate) type TerminalEvents = mpsc::UnboundedReceiver<TerminalEvent>;

/// Starts terminals and owns the channel their events arrive on.
pub(crate) struct TerminalHost {
    tx: mpsc::UnboundedSender<TerminalEvent>,
    rx: Option<TerminalEvents>,
    next: u64,
    /// The program [`Self::spawn`] starts in place of the user's shell, so app
    /// tests run a known, side-effect-free command.
    #[cfg(test)]
    program: Option<Vec<String>>,
}

impl Default for TerminalHost {
    fn default() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tx,
            rx: Some(rx),
            next: 1,
            #[cfg(test)]
            program: None,
        }
    }
}

impl TerminalHost {
    /// Hand the event receiver to the UI loop (once; later calls get `None`).
    pub(crate) fn take_receiver(&mut self) -> Option<TerminalEvents> {
        self.rx.take()
    }

    /// Start the user's shell in `cwd` on a `columns` x `rows` grid.
    ///
    /// The shell is `$SHELL` (the platform's default command interpreter when it is
    /// unset), with `TERM=xterm-256color` and `COLORTERM=truecolor` so programs use
    /// the colours and keys the emulator understands.
    pub(crate) fn spawn(
        &mut self,
        cwd: &Path,
        columns: u16,
        rows: u16,
    ) -> Result<TerminalPane, String> {
        let mut command = CommandBuilder::new_default_prog();
        #[cfg(test)]
        if let Some(argv) = &self.program {
            command = CommandBuilder::from_argv(argv.iter().map(Into::into).collect());
        }
        command.cwd(cwd);
        self.spawn_command(command, columns, rows)
    }

    /// Make [`Self::spawn`] start `argv` instead of the user's shell.
    #[cfg(test)]
    pub(crate) fn use_program(&mut self, argv: &[&str]) {
        self.program = Some(argv.iter().map(|arg| (*arg).to_owned()).collect());
    }

    /// Start `command` on a fresh PTY (the shared body of [`Self::spawn`], also
    /// what the tests drive with a scripted command).
    pub(crate) fn spawn_command(
        &mut self,
        mut command: CommandBuilder,
        columns: u16,
        rows: u16,
    ) -> Result<TerminalPane, String> {
        let id = TerminalId(self.next);
        self.next += 1;
        let (columns, rows) = clamp_size(columns, rows);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("TERM_PROGRAM", "karet");

        let pair = native_pty_system()
            .openpty(pty_size(columns, rows))
            .map_err(|e| format!("cannot open a pseudo-terminal: {e:#}"))?;
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| format!("cannot start the shell: {e:#}"))?;
        // The slave end belongs to the child now; holding it open here would keep
        // the PTY alive after the shell exits, so the reader would never see EOF.
        drop(pair.slave);
        let killer = child.clone_killer();
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("cannot read the pseudo-terminal: {e:#}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("cannot write the pseudo-terminal: {e:#}"))?;

        let (write_tx, write_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let output_pending = Arc::new(AtomicBool::new(false));
        let listener = Listener {
            id,
            events: self.tx.clone(),
            writer: write_tx.clone(),
        };
        let term = Arc::new(FairMutex::new(Emulator::new_term(columns, rows, listener)));

        spawn_thread("karet-term-write", move || write_loop(writer, &write_rx))?;
        {
            let term = Arc::clone(&term);
            let events = self.tx.clone();
            let pending = Arc::clone(&output_pending);
            spawn_thread("karet-term-read", move || {
                read_loop(reader, &term, &events, id, &pending);
            })?;
        }
        {
            let events = self.tx.clone();
            spawn_thread("karet-term-wait", move || {
                let status = child.wait().ok().map(|status| describe_exit(&status));
                let _ = events.send(TerminalEvent {
                    id,
                    kind: TerminalEventKind::Exited(status),
                });
            })?;
        }

        Ok(TerminalPane {
            id,
            term,
            writer: write_tx,
            master: pair.master,
            killer,
            columns,
            rows,
            title: None,
            exited: None,
            output_pending,
            area: Rect::ZERO,
            selecting: false,
            clicks: ClickTracker::default(),
        })
    }
}

/// A short human description of how a shell ended.
fn describe_exit(status: &portable_pty::ExitStatus) -> String {
    match status.signal() {
        Some(signal) => format!("terminated by {signal}"),
        None => format!("exited with code {}", status.exit_code()),
    }
}

fn spawn_thread(name: &str, body: impl FnOnce() + Send + 'static) -> Result<(), String> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(body)
        .map(|_| ())
        .map_err(|e| format!("cannot start a terminal thread: {e}"))
}

/// Drain queued input to the PTY until the pane is dropped or the PTY closes.
fn write_loop(mut writer: Box<dyn Write + Send>, queue: &std::sync::mpsc::Receiver<Vec<u8>>) {
    while let Ok(bytes) = queue.recv() {
        if writer
            .write_all(&bytes)
            .and_then(|()| writer.flush())
            .is_err()
        {
            break;
        }
    }
}

/// Parse PTY output into the grid until EOF, announcing new output to the UI.
fn read_loop(
    mut reader: Box<dyn Read + Send>,
    term: &FairMutex<Term<Listener>>,
    events: &mpsc::UnboundedSender<TerminalEvent>,
    id: TerminalId,
    pending: &AtomicBool,
) {
    let mut processor: Processor = Processor::new();
    let mut buf = vec![0_u8; 64 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                processor.advance(&mut *term.lock(), &buf[..n]);
                if !pending.swap(true, Ordering::AcqRel)
                    && events
                        .send(TerminalEvent {
                            id,
                            kind: TerminalEventKind::Output,
                        })
                        .is_err()
                {
                    break;
                }
            },
        }
    }
}

/// The emulator's event sink: replies go straight back to the PTY, everything
/// the UI must act on goes to the shared channel.
pub(crate) struct Listener {
    id: TerminalId,
    events: mpsc::UnboundedSender<TerminalEvent>,
    writer: std::sync::mpsc::Sender<Vec<u8>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let kind = match event {
            // Device-status and similar replies the program is waiting on.
            TermEvent::PtyWrite(text) => {
                let _ = self.writer.send(text.into_bytes());
                return;
            },
            TermEvent::Title(title) => TerminalEventKind::Title(title),
            TermEvent::ResetTitle => TerminalEventKind::ResetTitle,
            TermEvent::ClipboardStore(_, text) => TerminalEventKind::Clipboard(text),
            TermEvent::Bell => TerminalEventKind::Bell,
            // Clipboard reads (OSC 52 paste) stay refused: `Osc52::OnlyCopy` never
            // asks. Colour and pixel-size queries go unanswered, as on any
            // terminal that does not support them; the rest is renderer chatter.
            _ => return,
        };
        let _ = self.events.send(TerminalEvent { id: self.id, kind });
    }
}

/// Builders for the emulator half, separate from the PTY so tests can drive a
/// grid with bytes alone.
pub(crate) struct Emulator;

impl Emulator {
    /// A [`Term`] of `columns` x `rows` with this app's emulator configuration.
    pub(crate) fn new_term<T: EventListener>(columns: u16, rows: u16, listener: T) -> Term<T> {
        let config = TermConfig {
            scrolling_history: SCROLLBACK_LINES,
            ..TermConfig::default()
        };
        Term::new(config, &GridSize::new(columns, rows), listener)
    }
}

/// A grid size in cells, the [`Dimensions`] the emulator resizes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GridSize {
    columns: usize,
    rows: usize,
}

impl GridSize {
    pub(crate) fn new(columns: u16, rows: u16) -> Self {
        Self {
            columns: usize::from(columns),
            rows: usize::from(rows),
        }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

fn clamp_size(columns: u16, rows: u16) -> (u16, u16) {
    (columns.max(MIN_COLUMNS), rows.max(MIN_ROWS))
}

fn pty_size(columns: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols: columns,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Strip control characters from a program-supplied title before it reaches the
/// tab strip, so a hostile title cannot smuggle escapes into the host terminal.
pub(crate) fn sanitize_title(title: &str) -> String {
    title.chars().filter(|c| !c.is_control()).collect()
}

/// One running (or exited) shell and the UI state of the pane showing it.
pub(crate) struct TerminalPane {
    id: TerminalId,
    term: Arc<FairMutex<Term<Listener>>>,
    writer: std::sync::mpsc::Sender<Vec<u8>>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    columns: u16,
    rows: u16,
    /// The program's own title (OSC 0/2), sanitized; `None` until it sets one.
    title: Option<String>,
    /// How the shell ended, once it has.
    exited: Option<String>,
    output_pending: Arc<AtomicBool>,
    /// The rect the grid was last painted into, for mouse hit-testing.
    pub(crate) area: Rect,
    /// Whether a left-button selection drag is in progress.
    pub(crate) selecting: bool,
    /// Multi-click detection for word and line selection.
    pub(crate) clicks: ClickTracker,
}

impl std::fmt::Debug for TerminalPane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalPane")
            .field("id", &self.id)
            .field("columns", &self.columns)
            .field("rows", &self.rows)
            .field("exited", &self.exited)
            .finish_non_exhaustive()
    }
}

impl Drop for TerminalPane {
    fn drop(&mut self) {
        // Hang up on the shell (SIGHUP on Unix, terminate on Windows); closing the
        // master as the pane drops finishes what the signal starts.
        if self.exited.is_none() {
            let _ = self.killer.kill();
        }
    }
}

impl TerminalPane {
    /// This terminal's identity.
    pub(crate) fn id(&self) -> TerminalId {
        self.id
    }

    /// The grid, locked for reading or mutation. Hold it briefly: the reader
    /// thread waits on it to parse more output.
    pub(crate) fn term(&self) -> impl std::ops::DerefMut<Target = Term<Listener>> + '_ {
        self.term.lock()
    }

    /// The program-set title, if any.
    pub(crate) fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Record a program-set title (sanitized); `None` resets it.
    pub(crate) fn set_title(&mut self, title: Option<&str>) {
        self.title = title.map(sanitize_title).filter(|t| !t.trim().is_empty());
    }

    /// How the shell ended, once it has.
    pub(crate) fn exit_status(&self) -> Option<&str> {
        self.exited.as_deref()
    }

    /// Record that the shell ended.
    pub(crate) fn mark_exited(&mut self, status: Option<String>) {
        self.exited = Some(status.unwrap_or_else(|| "exited".to_owned()));
    }

    /// Allow the reader to announce the next batch of output.
    pub(crate) fn acknowledge_output(&self) {
        self.output_pending.store(false, Ordering::Release);
    }

    /// The emulator's current mode flags.
    pub(crate) fn mode(&self) -> TermMode {
        *self.term().mode()
    }

    /// The current grid size in cells.
    #[cfg(test)]
    pub(crate) fn size(&self) -> (u16, u16) {
        (self.columns, self.rows)
    }

    /// Queue `bytes` for the shell. Never blocks; a no-op once the shell is gone.
    pub(crate) fn write(&self, bytes: impl Into<Vec<u8>>) {
        if self.exited.is_none() {
            let _ = self.writer.send(bytes.into());
        }
    }

    /// Typing returns the view to the live screen, as in every terminal.
    pub(crate) fn input(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        {
            let mut term = self.term();
            if term.grid().display_offset() != 0 {
                term.scroll_display(Scroll::Bottom);
            }
            term.selection = None;
        }
        self.write(bytes);
    }

    /// Resize the grid and the PTY to `columns` x `rows` when they changed.
    pub(crate) fn resize(&mut self, columns: u16, rows: u16) {
        let (columns, rows) = clamp_size(columns, rows);
        if (columns, rows) == (self.columns, self.rows) {
            return;
        }
        self.columns = columns;
        self.rows = rows;
        self.term().resize(GridSize::new(columns, rows));
        let _ = self.master.resize(pty_size(columns, rows));
    }

    /// Scroll the history view.
    pub(crate) fn scroll(&self, scroll: Scroll) {
        self.term().scroll_display(scroll);
    }

    /// The selected text, if any.
    pub(crate) fn selection_text(&self) -> Option<String> {
        self.term()
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    /// Drop the selection.
    pub(crate) fn clear_selection(&self) {
        self.term().selection = None;
    }
}

#[cfg(test)]
mod tests;
