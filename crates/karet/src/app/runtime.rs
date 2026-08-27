use super::*;

/// The session configuration for `app` — shared by the interactive runtime and
/// the capture harness so the two attach identically. `swap_dir` differs: the
/// real app persists crash-recovery swaps; a throwaway capture must not.
pub(crate) fn session_config(app: &App, swap_dir: Option<std::path::PathBuf>) -> SessionConfig {
    session_config_for(
        app.root.clone(),
        app.loaded_config.clone(),
        app.syntax,
        swap_dir,
    )
}

/// The session configuration for a workspace, without a shell around it.
///
/// A backend serving a remote client never builds an [`App`], but must configure
/// its session identically to a local one — same settings layers, same supervisor,
/// same shared language-server installs. Having one constructor is what keeps a
/// served workspace and a local one from drifting.
///
/// Every path resolved here belongs to the machine holding the workspace, which is
/// the point: `current_exe` is *this* karet, and the language-server directory is
/// the one whose servers can actually see these files.
pub(crate) fn session_config_for(
    root: std::path::PathBuf,
    loaded_config: LoadedConfig,
    syntax: bool,
    swap_dir: Option<std::path::PathBuf>,
) -> SessionConfig {
    SessionConfig {
        roots: vec![root],
        diff_syntax: syntax,
        settings: loaded_config.settings.clone(),
        loaded_config,
        swap_dir,
        // Every external process is owned by a hidden copy of this executable.
        process_supervisor: std::env::current_exe().ok(),
        // Immutable installations are shared by every local karet instance.
        lsp_registry_dir: directories::ProjectDirs::from("", "getkono", "karet")
            .map(|dirs| dirs.data_local_dir().join("language-servers")),
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
    attach(app, Arc::new(local_backend), snaps)
}

/// Attach an already-constructed backend and run the shared post-attach steps.
///
/// The composition root only ever sees the `Backend` seam, so a session running
/// in this process and one running on another machine arrive here identically —
/// which is the whole reason remote mode needed no changes below this line.
pub(super) fn attach(
    app: &mut App,
    backend: Arc<dyn Backend>,
    snaps: SnapshotRx,
) -> color_eyre::Result<(EventRx, SnapshotRx)> {
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
            diag.severity,
            NotificationKind::System,
            format!("config: {}", diag.message),
        );
    }
    Ok((events, snaps))
}

/// Where the session this shell drives comes from.
pub enum Source {
    /// An in-process session over the given configuration.
    Local(Box<SessionConfig>),
    /// A session on the other end of a connection, opened by `open`.
    ///
    /// The connection is established inside the shell's runtime rather than
    /// handed in already made, because a `RemoteBackend` and its pump are tied to
    /// the runtime that spawned them.
    Remote(RemoteOpen),
}

/// Opens a connection to a remote backend, inside the shell's Tokio runtime.
pub type RemoteOpen = Box<
    dyn FnOnce() -> std::pin::Pin<
            Box<dyn Future<Output = color_eyre::Result<(Arc<dyn Backend>, SnapshotRx)>>>,
        > + Send,
>;

pub fn run(mut app: App, source: Source) -> color_eyre::Result<()> {
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
    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(
        io::stdout(),
        SetTitle(format!("karet - {}", crate::window_title_path(&app.root)))
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
    }
    // Same handshake for OSC 22 pointer-shape hints (col-resize/row-resize over
    // the sidebar/SCM dividers) — confirmed support only, never assumed.
    if crate::term_caps::probe_osc22_pointer_shape(crate::term_caps::PROBE_TIMEOUT) == Some(true) {
        app.caps.pointer_shapes = true;
    }

    let result = runtime.block_on(async move {
        let (events, snaps) = match source {
            Source::Local(config) => attach_backend(&mut app, *config)?,
            Source::Remote(open) => {
                let (backend, snaps) = open().await?;
                attach(&mut app, backend, snaps)?
            },
        };
        let graphical_cursor_requested = app.tabs.get(app.active).is_some_and(|tab| {
            app.settings
                .editor
                .for_language(tab_language(tab))
                .graphical_cursor()
                == Some(true)
        });
        if graphical_cursor_requested && !app.graphical_cursor_compatible() {
            app.notify(
                Severity::Error,
                NotificationKind::System,
                "graphical cursor is not compatible with this terminal",
            );
        }
        event_loop(&mut terminal, &mut app, events, snaps).await
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
    result
}

/// The async UI loop: render, then wake on terminal input, a backend event, or a
/// document snapshot — coalescing each burst into a single repaint.
async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    mut events: EventRx,
    mut snaps: SnapshotRx,
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

    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        app.flush_graphics();
        // The visible line range is only known once the editor has been laid out,
        // so the backend hears about it after the frame that established it.
        app.report_viewports();

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
            () = async move {
                match deadline {
                    Some(d) => tokio::time::sleep(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {},
        }
        app.notifications.expire(Instant::now());
        app.expire_operation_blocker(Instant::now());
        app.fire_auto_save(Instant::now());

        // Drain everything else that is ready so a burst collapses into one frame.
        while let Ok(event) = input_rx.try_recv() {
            handle_terminal_event(app, event);
            if app.should_quit {
                break;
            }
        }
        while let Some((id, ev)) = events.try_recv() {
            app.on_backend_event(id, ev);
        }
        while let Some((doc, snap)) = snaps.try_recv() {
            app.on_snapshot(doc, &snap);
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

/// Dispatch one terminal event to the app.
fn handle_terminal_event(app: &mut App, event: Event) {
    app.reset_graphics_caret_blink();
    let previous = (app.focus == Focus::Editor)
        .then(|| app.active_code_doc())
        .flatten();
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key),
        Event::Mouse(mouse) => app.handle_mouse(mouse),
        Event::Paste(text) => app.handle_paste(text),
        Event::FocusLost => app.auto_save_focus_lost(),
        _ => {},
    }
    app.auto_save_context_changed(previous);
}
