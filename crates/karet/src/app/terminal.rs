//! The integrated terminal in the shell: opening and toggling terminal tabs,
//! routing keys, pastes and the mouse to the active one, and applying what the
//! terminals' background threads report.

use std::time::Duration;
use std::time::Instant;

use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::TermMode;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use crossterm::event::MouseButton;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;
use karet_core::NotificationKind;
use karet_session::ViewId;
use karet_widgets::PaneId;

use crate::app::App;
use crate::app::notifications::Report;
use crate::app::util::rect_contains;
use crate::command::Command;
use crate::keymap;
use crate::keymap::Context;
use crate::keymap::FocusTarget;
use crate::keymap::KeyChord;
use crate::keymap::Resolved;
use crate::tab::Tab;
use crate::tab::TabKind;
use crate::terminal::MouseAction;
use crate::terminal::MouseButtonKind;
use crate::terminal::MouseReport;
use crate::terminal::TerminalEvent;
use crate::terminal::TerminalEventKind;
use crate::terminal::TerminalPane;

/// The grid a terminal starts on before its first paint sizes it to its pane.
const DEFAULT_SIZE: (u16, u16) = (80, 24);

/// The least time between two repaints that terminal output asks for (about 60
/// frames a second), so a program printing without pause cannot keep a core
/// busy repainting the whole screen.
pub(super) const TERMINAL_FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// The frame cap terminal output repaints under: when the last frame was
/// painted, and the repaint scheduled for output that arrived too soon after it.
#[derive(Debug, Default)]
pub(crate) struct TerminalFrame {
    drawn: Option<Instant>,
    due: Option<Instant>,
}

impl TerminalFrame {
    /// Output for a visible terminal arrived at `now`: whether to repaint at
    /// once. Too soon after the last frame, the repaint is scheduled instead.
    fn output(&mut self, now: Instant) -> bool {
        match self.drawn.map(|drawn| drawn + TERMINAL_FRAME_INTERVAL) {
            Some(next) if now < next => {
                self.due.get_or_insert(next);
                false
            },
            _ => true,
        }
    }

    /// A frame was painted at `now`; it showed whatever output was waiting.
    pub(super) fn drawn(&mut self, now: Instant) {
        self.drawn = Some(now);
        self.due = None;
    }

    /// How long until the scheduled terminal repaint, if one is.
    pub(super) fn next_wake(&self, now: Instant) -> Option<Duration> {
        self.due.map(|due| due.saturating_duration_since(now))
    }
}

impl App {
    /// The active tab's terminal, when the active tab is one.
    pub(super) fn active_terminal(&self) -> Option<&TerminalPane> {
        match &self.tabs.get(self.active)?.kind {
            TabKind::Terminal(pane) => Some(pane),
            _ => None,
        }
    }

    fn active_terminal_mut(&mut self) -> Option<&mut TerminalPane> {
        match &mut self.tabs.get_mut(self.active)?.kind {
            TabKind::Terminal(pane) => Some(pane),
            _ => None,
        }
    }

    /// Whether keys go to the active terminal's shell: the editor shell owns the
    /// keyboard (no modal, editor focus, editor view) and its tab is a terminal.
    pub(super) fn terminal_owns_keyboard(&self) -> bool {
        self.active_terminal().is_some() && self.editor_owns_keyboard()
    }

    /// Run one of the terminal commands.
    pub(super) fn terminal_command(&mut self, command: Command) {
        match command {
            Command::ToggleTerminal => self.toggle_terminal(),
            Command::NewTerminal => self.new_terminal(),
            Command::TerminalCopy => self.terminal_copy(),
            Command::TerminalPaste => match self.clipboard.get() {
                Ok(text) => self.terminal_paste(&text),
                Err(_) => self.notify(
                    Report::Failure,
                    NotificationKind::System,
                    "paste: clipboard unavailable",
                ),
            },
            Command::TerminalScrollPageUp => self.terminal_scroll(Scroll::PageUp),
            Command::TerminalScrollPageDown => self.terminal_scroll(Scroll::PageDown),
            Command::TerminalScrollToTop => self.terminal_scroll(Scroll::Top),
            Command::TerminalScrollToBottom => self.terminal_scroll(Scroll::Bottom),
            _ => {},
        }
    }

    /// Ctrl+`: from a focused terminal, go back to the most recent other tab;
    /// anywhere else, show the most recent terminal, starting one if none is open.
    pub(super) fn toggle_terminal(&mut self) {
        if self.terminal_owns_keyboard() {
            if let Some((pane, index)) = self.recent_tab_where(|tab| !tab.is_terminal()) {
                self.focus_pane_switch(pane);
                self.select_tab(index);
            }
            return;
        }
        match self.recent_tab_where(Tab::is_terminal) {
            Some((pane, index)) => {
                self.focus_pane_switch(pane);
                self.select_tab(index);
            },
            None => self.new_terminal(),
        }
    }

    /// The pane and index of the most recently activated tab matching `wanted`,
    /// across every pane.
    fn recent_tab_where(&self, wanted: impl Fn(&Tab) -> bool) -> Option<(PaneId, usize)> {
        let focused = self.focus_pane();
        let panes = std::iter::once((focused, self.tabs.as_slice())).chain(
            self.stored
                .iter()
                .map(|(pane, stored)| (*pane, stored.tabs.as_slice())),
        );
        let panes: Vec<(PaneId, &[Tab])> = panes.collect();
        let find = |view: ViewId| {
            panes.iter().find_map(|(pane, tabs)| {
                tabs.iter()
                    .position(|tab| tab.view == view && wanted(tab))
                    .map(|index| (*pane, index))
            })
        };
        self.view_history
            .iter()
            .rev()
            .find_map(|view| find(*view))
            .or_else(|| {
                // A matching tab never activated yet (none in practice): take the
                // focused pane's first.
                self.tabs
                    .iter()
                    .position(&wanted)
                    .map(|index| (focused, index))
            })
    }

    /// The grid a new terminal in the focused pane starts on: the pane's content
    /// area, or a default before the first paint has measured it.
    pub(super) fn terminal_grid_size(&self) -> (u16, u16) {
        self.last_frame
            .panes
            .iter()
            .find(|frame| frame.pane == self.focus_pane())
            .map(|frame| (frame.content_rect.width, frame.content_rect.height))
            .filter(|(columns, rows)| *columns > 0 && *rows > 0)
            .unwrap_or(DEFAULT_SIZE)
    }

    /// The pane and index of terminal `id`'s tab, wherever it is.
    pub(super) fn terminal_tab(&self, id: crate::terminal::TerminalId) -> Option<(PaneId, usize)> {
        let shows = |tab: &Tab| matches!(&tab.kind, TabKind::Terminal(pane) if pane.id() == id);
        std::iter::once((self.focus_pane(), self.tabs.as_slice()))
            .chain(
                self.stored
                    .iter()
                    .map(|(pane, stored)| (*pane, stored.tabs.as_slice())),
            )
            .find_map(|(pane, tabs)| tabs.iter().position(shows).map(|index| (pane, index)))
    }

    /// Start a shell in the workspace root in a new tab of the focused pane.
    pub(super) fn new_terminal(&mut self) {
        let (columns, rows) = self.terminal_grid_size();
        match self.terminals.spawn(&self.root, columns, rows) {
            Ok(pane) => self.push_tab(Tab::terminal(pane)),
            Err(error) => self.notify(
                Report::Failure,
                NotificationKind::System,
                format!("terminal: {error}"),
            ),
        }
    }

    fn terminal_scroll(&mut self, scroll: Scroll) {
        if let Some(pane) = self.active_terminal() {
            pane.scroll(scroll);
        }
    }

    fn terminal_copy(&mut self) {
        let Some(text) = self
            .active_terminal()
            .and_then(TerminalPane::selection_text)
        else {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "copy: no text selected",
            );
            return;
        };
        self.copy_to_clipboard(text, "selection");
    }

    /// Send pasted text to the active terminal.
    pub(super) fn terminal_paste(&mut self, text: &str) {
        if let Some(pane) = self.active_terminal() {
            pane.input(crate::terminal::encode_paste(text, pane.mode()));
        }
    }

    /// A key while the terminal owns the keyboard: the terminal layer's app keys
    /// first, then Ctrl+C over a selection copies it, and everything else is
    /// encoded for the shell. On a terminal whose shell has exited, any other key
    /// closes the tab.
    pub(super) fn terminal_key(&mut self, key: KeyEvent) {
        self.pending.clear();
        let ctx = Context::focus(FocusTarget::Terminal);
        if let Resolved::Command(command) = keymap::resolve(ctx, &[KeyChord::from_event(key)]) {
            self.dispatch(command);
            return;
        }
        let Some(pane) = self.active_terminal() else {
            return;
        };
        if pane.exit_status().is_some() {
            self.close_tab_at(self.active);
            return;
        }
        let copy_chord = key.code == KeyCode::Char('c')
            && key.modifiers == KeyModifiers::CONTROL
            && pane.selection_text().is_some();
        if copy_chord {
            if let Some(text) = pane.selection_text() {
                pane.clear_selection();
                self.copy_to_clipboard(text, "selection");
            }
            return;
        }
        if let Some(bytes) = crate::terminal::encode_key(key, pane.mode()) {
            pane.input(bytes);
        }
    }

    /// A mouse event over the focused pane's terminal (or continuing a selection
    /// drag that began there). Returns whether it was consumed.
    pub(super) fn terminal_mouse(&mut self, mouse: MouseEvent) -> bool {
        let Some(pane) = self.active_terminal() else {
            return false;
        };
        let area = pane.area;
        let point = (mouse.column, mouse.row);
        if !pane.selecting && !rect_contains(area, point) {
            return false;
        }
        let column = mouse
            .column
            .saturating_sub(area.x)
            .min(area.width.saturating_sub(1));
        let row = mouse
            .row
            .saturating_sub(area.y)
            .min(area.height.saturating_sub(1));
        let mode = pane.mode();
        if matches!(mouse.kind, MouseEventKind::Down(_)) {
            self.keyboard.focus_editor();
        }
        // A program tracking the mouse (htop, vim with `mouse=a`) gets the event;
        // Shift is the escape hatch to select text over it, as in xterm.
        let reporting = mode.intersects(TermMode::MOUSE_MODE)
            && !mouse.modifiers.contains(KeyModifiers::SHIFT)
            && !self.active_terminal().is_some_and(|pane| pane.selecting);
        if reporting {
            if let Some(action) = mouse_action(mouse.kind)
                && let Some(bytes) = crate::terminal::encode_mouse(
                    MouseReport {
                        action,
                        column,
                        row,
                        modifiers: mouse.modifiers,
                    },
                    mode,
                )
                && let Some(pane) = self.active_terminal()
            {
                pane.write(bytes);
            }
            return true;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if mouse.modifiers.contains(KeyModifiers::CONTROL)
                    && let Some(uri) = self
                        .active_terminal()
                        .and_then(|pane| pane.hyperlink_at(column, row))
                {
                    self.open_terminal_link(&uri);
                    return true;
                }
                if let Some(pane) = self.active_terminal_mut() {
                    let clicks = pane.clicks.press(column, row, Instant::now());
                    let ty = crate::terminal::selection_type(clicks);
                    pane.start_selection(ty, column, row);
                }
            },
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(pane) = self.active_terminal().filter(|pane| pane.selecting) {
                    pane.extend_selection(column, row);
                }
            },
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(pane) = self.active_terminal_mut().filter(|pane| pane.selecting) {
                    pane.finish_selection();
                }
            },
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = mouse.kind == MouseEventKind::ScrollUp;
                if let Some(pane) = self.active_terminal() {
                    wheel(pane, mode, up);
                }
            },
            _ => {},
        }
        true
    }

    /// Ctrl+click on an OSC 8 hyperlink: open it with the desktop when its scheme
    /// is one this app trusts.
    fn open_terminal_link(&mut self, uri: &str) {
        let Some(url) = crate::links::safe_external(uri) else {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "terminal: this link's kind of address is not opened",
            );
            return;
        };
        if let Err(error) = crate::links::open_external(url) {
            self.notify(
                Report::Failure,
                NotificationKind::System,
                format!("could not open link: {error}"),
            );
        }
    }

    /// Whether terminal `id` is the active tab of some pane, so its output can
    /// show.
    fn terminal_visible(&self, id: crate::terminal::TerminalId) -> bool {
        let shows = |tab: Option<&Tab>| matches!(tab.map(|tab| &tab.kind), Some(TabKind::Terminal(pane)) if pane.id() == id);
        shows(self.tabs.get(self.active))
            || self
                .stored
                .values()
                .any(|stored| shows(stored.tabs.get(stored.active)))
    }

    /// Apply one report from a terminal's background threads, wherever its tab
    /// is, at `now`. Returns whether the screen needs repainting now.
    ///
    /// Output is acknowledged by painting the terminal (`ui::terminal`), not
    /// here: a terminal no pane shows stays unacknowledged, so it announces no
    /// more output, and wakes nothing, until it is shown again. A task's
    /// terminal is the exception: its scans acknowledge it (`app::tasks`), so
    /// its problems stay live in a background tab, and a scan that changes them
    /// asks for a repaint even when no pane shows the terminal.
    pub(super) fn on_terminal_event(&mut self, event: TerminalEvent, now: Instant) -> bool {
        if event.kind == TerminalEventKind::Output {
            let problems_changed = self.task_output(event.id, now);
            let shown = self.terminal_visible(event.id) && self.terminal_frame.output(now);
            return problems_changed || shown;
        }
        let mut copied = None;
        let mut exited = None;
        for tab in self.all_tabs_mut() {
            let TabKind::Terminal(pane) = &mut tab.kind else {
                continue;
            };
            if pane.id() != event.id {
                continue;
            }
            match &event.kind {
                TerminalEventKind::Output => {},
                TerminalEventKind::Title(title) => pane.set_title(Some(title)),
                TerminalEventKind::ResetTitle => pane.set_title(None),
                TerminalEventKind::Clipboard(text) => copied = Some(text.clone()),
                // Not echoed: a toast per `\a` from a shell's tab completion would
                // bury every other notification.
                TerminalEventKind::Bell => {},
                TerminalEventKind::Exited(status) => {
                    pane.mark_exited(status.clone());
                    exited = pane.exit_status().map(str::to_owned);
                },
            }
            tab.title = crate::tab::terminal_title(pane);
            break;
        }
        if let Some(text) = copied {
            self.copy_to_clipboard(text, "terminal selection");
        }
        if let TerminalEventKind::Exited(_) = &event.kind {
            // Also when no tab showed it: a closed task terminal is forgotten.
            self.task_exited(event.id, exited.as_deref());
        }
        // A title, an exit or a copy notice shows at once; the bell shows nothing.
        event.kind != TerminalEventKind::Bell
    }
}

/// What a crossterm mouse event is, in terminal-report terms.
fn mouse_action(kind: MouseEventKind) -> Option<MouseAction> {
    let button = |button: MouseButton| match button {
        MouseButton::Left => MouseButtonKind::Left,
        MouseButton::Middle => MouseButtonKind::Middle,
        MouseButton::Right => MouseButtonKind::Right,
    };
    Some(match kind {
        MouseEventKind::Down(b) => MouseAction::Press(button(b)),
        MouseEventKind::Up(b) => MouseAction::Release(button(b)),
        MouseEventKind::Drag(b) => MouseAction::Motion(button(b)),
        MouseEventKind::Moved => MouseAction::Motion(MouseButtonKind::None),
        MouseEventKind::ScrollUp => MouseAction::WheelUp,
        MouseEventKind::ScrollDown => MouseAction::WheelDown,
        MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => return None,
    })
}

/// One wheel notch over a terminal that is not tracking the mouse: a full-screen
/// program on the alternate screen (less, man) gets arrow keys, as xterm's
/// alternate-scroll mode does; otherwise the history scrolls.
fn wheel(pane: &TerminalPane, mode: TermMode, up: bool) {
    if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
        let arrow = KeyEvent::new(
            if up { KeyCode::Up } else { KeyCode::Down },
            KeyModifiers::NONE,
        );
        if let Some(bytes) = crate::terminal::encode_key(arrow, mode) {
            let lines = usize::try_from(crate::terminal::WHEEL_LINES).unwrap_or(1);
            pane.write(bytes.repeat(lines));
        }
        return;
    }
    let lines = crate::terminal::WHEEL_LINES;
    pane.scroll(Scroll::Delta(if up { lines } else { -lines }));
}
