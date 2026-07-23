use super::*;

pub fn run(mut app: App) -> color_eyre::Result<()> {
    let kitty_keyboard_supported = crate::term_caps::supports_kitty_keyboard();
    if !kitty_keyboard_supported {
        return Err(eyre!(
            "karet requires a terminal with kitty keyboard protocol support \
             (kitty, ghostty, WezTerm, foot, …)"
        ));
    }
    app.kitty_keyboard_supported = true;

    // The session backend runs on its own Tokio runtime; the UI task selects over
    // terminal input, backend events, and document snapshots so it never blocks.
    let runtime = tokio::runtime::Runtime::new().map_err(|e| eyre!("tokio runtime: {e}"))?;
    let (session, events, snaps) = Session::new(SessionConfig {
        roots: vec![app.root.clone()],
        settings: app.settings.clone(),
        loaded_config: app.loaded_config.clone(),
        // The real app persists crash-recovery swaps to the user data directory;
        // headless/test sessions leave this unset and keep no backups.
        swap_dir: karet_session::backup::default_swap_dir(),
    });

    let mut terminal = ratatui::init();
    let _ = crossterm::execute!(
        io::stdout(),
        SetTitle(format!("karet - {}", app.root.display()))
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
        app.graphics = GraphicsProtocol::Kitty;
        app.kitty_graphics_supported = true;
    }
    // Same handshake for OSC 22 pointer-shape hints (col-resize/row-resize over
    // the sidebar/SCM dividers) — confirmed support only, never assumed.
    if crate::term_caps::probe_osc22_pointer_shape(crate::term_caps::PROBE_TIMEOUT) == Some(true) {
        app.pointer_shapes_supported = true;
    }

    let result = runtime.block_on(async move {
        let backend: Arc<dyn Backend> = Arc::new(local(session));
        app.backend = Some(backend);
        app.register_open_tabs();
        // Surface any configuration-load problems as startup notifications, now that
        // the notification center will render on the first frame.
        for diag in std::mem::take(&mut app.config_diagnostics) {
            app.notify(
                diag.severity,
                NotificationKind::System,
                format!("config: {}", diag.message),
            );
        }
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
        let Some(prepared) = app.prepare_rx.take() else {
            return Err(eyre!("diff preparation result stream is unavailable"));
        };
        event_loop(&mut terminal, &mut app, events, snaps, prepared).await
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
    mut prepared: tokio::sync::mpsc::UnboundedReceiver<prepare::PrepareResult>,
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
            result = prepared.recv() => if let Some(result) = result {
                app.on_prepare_result(result);
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
        while let Ok(result) = prepared.try_recv() {
            app.on_prepare_result(result);
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
