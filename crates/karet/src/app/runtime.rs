use super::*;

/// The session configuration for `app` — shared by the interactive runtime and
/// the capture harness so the two attach identically. `swap_dir` differs: the
/// real app persists crash-recovery swaps; a throwaway capture must not.
pub(super) fn session_config(app: &App, swap_dir: Option<std::path::PathBuf>) -> SessionConfig {
    SessionConfig {
        roots: vec![app.root.clone()],
        diff_syntax: app.syntax,
        settings: app.settings.clone(),
        loaded_config: app.loaded_config.clone(),
        swap_dir,
        // Every external process is owned by a hidden copy of this executable.
        process_supervisor: std::env::current_exe().ok(),
        // Immutable installations are shared by every local karet instance.
        lsp_registry_dir: directories::ProjectDirs::from("", "getkono", "karet")
            .map(|dirs| dirs.data_local_dir().join("language-servers")),
        // The stored seam index, so opening the view a second time does not re-parse a
        // repository that has not changed. Losing it costs a rebuild and nothing else.
        seam_cache_dir: karet_session::seam_cache::default_cache_dir(),
    }
}

/// Build the local backend from `config`, attach it to `app`, and run the
/// shared post-attach steps (tab registration, deferred startup requests, and
/// config-load notifications). Returns the event and snapshot streams the
/// caller's loop selects over.
pub(super) fn attach_backend(
    app: &mut App,
    config: SessionConfig,
) -> color_eyre::Result<(EventRx, SnapshotRx)> {
    let (local_backend, snaps) = local(config);
    // The composition root only ever sees the `Backend` seam: the local
    // implementation is constructed here, and the event stream comes off the
    // trait — the exact shape a remote backend will slot into.
    let backend: Arc<dyn Backend> = Arc::new(local_backend);
    let Some(events) = backend.take_events() else {
        return Err(eyre!("backend event stream is unavailable"));
    };
    app.backend = Some(backend);
    app.register_open_tabs();
    app.request_pending_startup_diffs();
    app.request_pending_spelling_scan();
    // `--command` runs last, and only once the backend can answer: a palette
    // command that talks to it (Show Hover, Trigger Suggest, Go to Definition,
    // the markdown edits) returns early otherwise, so dispatching these during
    // startup-flag application silently dropped them. When the active tab is
    // still waiting for its document id this is a no-op and the answering
    // event runs them instead.
    app.run_startup_commands_when_ready();
    // Surface any configuration-load problems as startup notifications, now that
    // the notification center will render on the first frame.
    for diag in std::mem::take(&mut app.config_diagnostics) {
        app.notify(
            Report::from_severity(diag.severity),
            NotificationKind::System,
            format!("config: {}", diag.message),
        );
    }
    Ok((events, snaps))
}

pub fn run(mut app: App) -> color_eyre::Result<()> {
    let kitty_keyboard_supported = crate::term_caps::supports_kitty_keyboard();
    if !kitty_keyboard_supported {
        return Err(eyre!(
            "karet requires a terminal with kitty keyboard protocol support \
             (kitty, ghostty, WezTerm, foot, …)"
        ));
    }
    app.caps.kitty_keyboard = true;

    // The session backend runs on its own Tokio runtime; the UI task selects over
    // terminal input, backend events, and document snapshots so it never blocks.
    let runtime = tokio::runtime::Runtime::new().map_err(|e| eyre!("tokio runtime: {e}"))?;
    // The real app persists crash-recovery swaps to the user data directory;
    // headless/test sessions leave this unset and keep no backups.
    let config = session_config(&app, karet_session::backup::default_swap_dir());
    // Only the live editor remembers what was opened: tests and `--capture`
    // keep the in-memory store and never write the user's state directory.
    if let Some(path) = workspace::RecentStore::default_path() {
        app.recent = workspace::RecentStore::open(path);
    }
    let root = app.root.clone();
    app.recent.touch_workspace(&root);
    // Walk now, so the list is ready by the first Go to File.
    app.file_index.ensure_started();

    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(
        io::stdout(),
        SetTitle(format!(
            "karet - {}",
            crate::paths::window_title_path(&app.root)
        ))
    );
    let _keyboard = {
        let _ = crossterm::execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            )
        );
        KeyboardEnhancementGuard
    };
    // Bracketed paste makes a multi-line paste arrive as one `Event::Paste`, never a
    // storm of keystrokes the keymap would misinterpret.
    let _ = crossterm::execute!(
        io::stdout(),
        EnableMouseCapture,
        EnableBracketedPaste,
        EnableFocusChange
    );

    // Refine the env-var graphics heuristic with a real handshake (raw mode is on and
    // the input reader thread has not started yet, so we can read the reply here).
    // Upgrade to Kitty when the terminal actually answers; never downgrade a terminal
    // the heuristic already trusts.
    if crate::term_caps::probe_kitty_graphics(crate::term_caps::PROBE_TIMEOUT) == Some(true) {
        app.caps.graphics = GraphicsProtocol::Kitty;
        app.caps.kitty_graphics = true;
        app.caps.placeholders = super::state::unicode_placeholders();
    }
    // Same handshake for OSC 22 pointer-shape hints (col-resize/row-resize over
    // the sidebar/SCM dividers) — confirmed support only, never assumed.
    if crate::term_caps::probe_osc22_pointer_shape(crate::term_caps::PROBE_TIMEOUT) == Some(true) {
        app.caps.pointer_shapes = true;
    }
    // Themes are RGB; a terminal without 24-bit colour gets every frame
    // quantised to the 256-colour palette instead.
    let color_depth = crate::term_caps::probe_color_depth();

    // The saved workspace layout lives beside the swaps. Only the live editor
    // reads or writes it: `--capture` and tests never touch the user's state.
    let layout_dir = karet_session::backup::default_swap_dir();

    let result = runtime.block_on(async {
        // Restored before the backend attaches, so attaching registers every
        // restored pane's tabs with the rest of startup's.
        if let Some(dir) = &layout_dir {
            app.restore_workspace_layout(dir);
        }
        let (events, snaps) = attach_backend(&mut app, config)?;
        let graphical_cursor_requested = app.tabs.get(app.active).is_some_and(|tab| {
            app.settings
                .editor
                .for_language(tab_language(tab))
                .graphical_cursor()
                == Some(true)
        });
        if graphical_cursor_requested && !app.graphical_cursor_compatible() {
            app.notify(
                Report::Failure,
                NotificationKind::System,
                "graphical cursor is not compatible with this terminal",
            );
        }
        event_loop(&mut terminal, &mut app, events, snaps, color_depth).await
    });

    let _ = write!(io::stdout(), "{}", image::kitty_delete_all());
    let _ = crossterm::execute!(
        io::stdout(),
        DisableFocusChange,
        DisableBracketedPaste,
        DisableMouseCapture
    );
    drop(_keyboard);
    ratatui::restore();
    // Only a clean exit saves the layout; after a failure, the one the last
    // clean exit saved is the better guess at where the user was.
    if result.is_ok()
        && let Some(dir) = &layout_dir
        && let Err(error) = app.save_workspace_layout(dir)
    {
        app.farewell
            .push(format!("could not save the workspace layout: {error}"));
    }
    // Only now is there a screen to read it on: printed earlier, it would land in
    // the alternate screen and vanish with it.
    let mut stderr = io::stderr().lock();
    for line in std::mem::take(&mut app.farewell) {
        let _ = writeln!(stderr, "karet: {}", farewell_line(&line));
    }
    drop(stderr);
    let relaunch = app.take_relaunch();
    // Shut this window down completely — the session and its language servers,
    // the recent-store writes still queued — before another one starts.
    drop(app);
    drop(runtime);
    match (result, relaunch) {
        (Ok(()), Some(root)) => relaunch_on(&root),
        (result, _) => result,
    }
}

/// Start karet again on the workspace `root`, in this terminal. On Unix the
/// process image is replaced, so the shell sees one editor session; elsewhere
/// the new editor runs as a child and its exit status becomes this one's.
fn relaunch_on(root: &Path) -> color_eyre::Result<()> {
    let exe = std::env::current_exe().map_err(|e| eyre!("open recent: {e}"))?;
    let mut command = std::process::Command::new(exe);
    command.arg(root).current_dir(root);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // `exec` only returns when it failed.
        let error = command.exec();
        Err(eyre!(
            "open recent: cannot start karet on {}: {error}",
            root.display()
        ))
    }
    #[cfg(not(unix))]
    {
        let status = command
            .status()
            .map_err(|e| eyre!("open recent: cannot start karet on {}: {e}", root.display()))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// `message` made safe to write raw to a terminal: every control character (C0,
/// DEL, and C1, which covers ESC and the single-byte CSI) is replaced by its
/// `\u{..}`-style escape, so text carrying a filesystem path or a server's words
/// cannot drive the terminal it is printed on. Everything else, non-ASCII
/// included, passes through unchanged.
fn farewell_line(message: &str) -> String {
    let mut line = String::with_capacity(message.len());
    for character in message.chars() {
        if character.is_control() {
            line.extend(character.escape_default());
        } else {
            line.push(character);
        }
    }
    line
}

/// The async UI loop: render, then wake on terminal input, a backend event, or a
/// document snapshot — coalescing each burst into a single repaint.
async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    mut events: EventRx,
    mut snaps: SnapshotRx,
    color_depth: crate::color_depth::ColorDepth,
) -> color_eyre::Result<()> {
    // A dedicated thread turns the blocking `event::read` into an async stream.
    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<Event>();
    std::thread::spawn(move || {
        while let Ok(event) = event::read() {
            if input_tx.send(event).is_err() {
                break;
            }
        }
    });

    let mut decoded_images = preview_image_results(app);
    let mut file_scans = app.file_index.take_receiver();
    let mut terminal_events = terminal_events(app);
    // Saving a `keymap.jsonc` rebinds live; the watcher lives as long as the loop.
    let keymap_paths: Vec<std::path::PathBuf> =
        crate::keymap::user::paths(std::slice::from_ref(&app.root))
            .into_iter()
            .map(|(_, path)| path)
            .collect();
    let (_keymap_watcher, mut keymap_changes) = match crate::keymap::user::watch(&keymap_paths) {
        Some((watcher, rx)) => (Some(watcher), Some(rx)),
        None => (None, None),
    };

    // Every wake-up repaints, except one that brought only integrated-terminal
    // output arriving faster than its frame cap: that output waits for the
    // scheduled terminal frame (`next_wake`) instead.
    let mut redraw = true;
    loop {
        // Backend events (a definition answer, a debugger stop) move the caret
        // too; one look per frame records them for Go Back.
        app.observe_navigation();
        if redraw {
            app.sync_preview_graphics();
            terminal.draw(|f| {
                ui::draw(f, app);
                crate::color_depth::fit_buffer(f.buffer_mut(), color_depth);
            })?;
            app.flush_graphics();
            note_frame(app);
        }
        redraw = true;
        // The graph view only learns its viewport height by being painted; top its
        // history up once that is known, so the first screen is already backed by more
        // history than it shows.
        app.graph_prefetch();
        // Inlay hints need the painted viewport for the same reason the graph
        // does: the range worth annotating is only known once a frame has said
        // how much of the document is on screen.
        app.request_inlay_hints();
        app.request_code_lenses();
        // The lightbulb follows the painted caret the same way.
        app.probe_code_actions(Instant::now());
        // So does the occurrence highlight.
        app.probe_occurrences(Instant::now());
        // A workspace-symbol query goes out once typing has paused.
        app.probe_workspace_symbols(Instant::now());

        // Wake for notification expiry or a save-spinner frame; park on the event
        // sources when nothing time-based is pending (no idle repaints).
        let deadline = app.next_wake();

        tokio::select! {
            biased;
            input = input_rx.recv() => match input {
                Some(event) => handle_terminal_event(app, event),
                None => app.should_quit = true,
            },
            event = events.recv() => if let Some((id, ev)) = event {
                app.on_backend_event(id, ev);
            },
            snap = snaps.recv() => if let Some((doc, snap)) = snap {
                app.on_snapshot(doc, &snap);
            },
            image = recv_decoded(&mut decoded_images) => accept_decoded(app, image),
            scan = recv_decoded(&mut file_scans) => if let Some(scan) = scan {
                app.on_file_index(scan);
            },
            terminal = recv_decoded(&mut terminal_events) => redraw = accept_terminal(app, terminal),
            change = recv_decoded(&mut keymap_changes) => if change.is_some() {
                // One reload covers a burst of events (an atomic-rename save).
                while keymap_changes.as_mut().is_some_and(|rx| rx.try_recv().is_ok()) {}
                app.reload_keymap();
            },
            () = async move {
                match deadline {
                    Some(d) => tokio::time::sleep(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {},
        }
        app.tick_drag_autoscroll(Instant::now());
        app.notifications.expire(Instant::now());
        app.expire_operation_blocker(Instant::now());
        app.fire_auto_save(Instant::now());

        // Drain everything else that is ready so a burst collapses into one frame.
        while let Ok(event) = input_rx.try_recv() {
            redraw = true;
            handle_terminal_event(app, event);
            if app.should_quit {
                break;
            }
        }
        while let Some((id, ev)) = events.try_recv() {
            redraw = true;
            app.on_backend_event(id, ev);
        }
        while let Some((doc, snap)) = snaps.try_recv() {
            redraw = true;
            app.on_snapshot(doc, &snap);
        }
        while let Some(image) = decoded_images.as_mut().and_then(|rx| rx.try_recv().ok()) {
            redraw = true;
            accept_decoded(app, Some(image));
        }
        while let Some(scan) = file_scans.as_mut().and_then(|rx| rx.try_recv().ok()) {
            redraw = true;
            app.on_file_index(scan);
        }
        while let Some(event) = terminal_events.as_mut().and_then(|rx| rx.try_recv().ok()) {
            redraw |= accept_terminal(app, Some(event));
        }

        if app.should_quit {
            app.release_preview_graphics();
            return Ok(());
        }
    }
}

/// The channel finished preview-image decodes arrive on (`None` in a lean build).
#[cfg(feature = "images")]
pub(super) type DecodedImages = Option<mpsc::UnboundedReceiver<crate::preview_images::Decoded>>;
#[cfg(not(feature = "images"))]
pub(super) type DecodedImages = Option<mpsc::UnboundedReceiver<std::convert::Infallible>>;

#[cfg(feature = "images")]
pub(super) fn preview_image_results(app: &App) -> DecodedImages {
    app.preview_images.take_receiver()
}

#[cfg(not(feature = "images"))]
pub(super) fn preview_image_results(_app: &App) -> DecodedImages {
    None
}

/// The next finished decode; never resolves when there is no channel.
pub(super) async fn recv_decoded<T>(rx: &mut Option<mpsc::UnboundedReceiver<T>>) -> Option<T> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

#[cfg(feature = "images")]
pub(super) fn accept_decoded(app: &mut App, image: Option<crate::preview_images::Decoded>) {
    if let Some(image) = image {
        app.preview_images.accept(image);
    }
}

#[cfg(not(feature = "images"))]
pub(super) fn accept_decoded(_app: &mut App, _image: Option<std::convert::Infallible>) {}

/// The channel the integrated terminals report on (`None` in a lean build).
#[cfg(feature = "terminal")]
type TerminalEvents = Option<crate::terminal::TerminalEvents>;
#[cfg(not(feature = "terminal"))]
type TerminalEvents = Option<mpsc::UnboundedReceiver<std::convert::Infallible>>;

#[cfg(feature = "terminal")]
fn terminal_events(app: &mut App) -> TerminalEvents {
    app.terminals.take_receiver()
}

#[cfg(not(feature = "terminal"))]
fn terminal_events(_app: &mut App) -> TerminalEvents {
    None
}

/// Apply a terminal report; returns whether it needs a repaint now.
#[cfg(feature = "terminal")]
fn accept_terminal(app: &mut App, event: Option<crate::terminal::TerminalEvent>) -> bool {
    event.is_some_and(|event| app.on_terminal_event(event, Instant::now()))
}

#[cfg(not(feature = "terminal"))]
fn accept_terminal(_app: &mut App, _event: Option<std::convert::Infallible>) -> bool {
    false
}

/// Record that a frame was painted, for the terminal frame cap.
#[cfg(feature = "terminal")]
fn note_frame(app: &mut App) {
    app.terminal_frame.drawn(Instant::now());
}

#[cfg(not(feature = "terminal"))]
fn note_frame(_app: &mut App) {}

/// Dispatch one terminal event to the app.
pub(super) fn handle_terminal_event(app: &mut App, event: Event) {
    app.reset_graphics_caret_blink();
    let previous = (app.focus == Focus::Editor)
        .then(|| app.active_code_doc())
        .flatten();
    match event {
        // A key ends a pointer drag: whatever it does (open a picker, switch a
        // tab, type) the selection under the held button is no longer the subject.
        Event::Key(key) if key.kind == KeyEventKind::Press => {
            app.editor_drag = None;
            app.handle_key(key);
        },
        Event::Mouse(mouse) => app.handle_mouse(mouse),
        Event::Paste(text) => app.handle_paste(text),
        // The release may happen in another window, where it never reaches us.
        Event::FocusLost => {
            app.editor_drag = None;
            app.auto_save_focus_lost();
        },
        _ => {},
    }
    app.auto_save_context_changed(previous);
    // Per event, not per frame: a far click followed by typing in the same burst
    // would otherwise read as one edit and lose the step back.
    app.observe_navigation();
}

#[cfg(test)]
mod tests {
    use super::farewell_line;

    #[test]
    fn a_farewell_line_escapes_control_characters_and_keeps_the_rest() {
        assert_eq!(
            farewell_line("opened /tmp/\u{1b}]0;pwned\u{7}x\u{9b}2J\nnext read-only"),
            "opened /tmp/\\u{1b}]0;pwned\\u{7}x\\u{9b}2J\\nnext read-only",
            "ESC, BEL, C1 CSI and newline are escaped"
        );
        assert_eq!(
            farewell_line("quit: 1 save(s) abandoned — /home/zoë/t.rs"),
            "quit: 1 save(s) abandoned — /home/zoë/t.rs",
            "printable non-ASCII passes through untouched"
        );
    }
}
