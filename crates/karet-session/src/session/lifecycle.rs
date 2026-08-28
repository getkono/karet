use super::*;

impl Session {
    /// Create a session and its paired event and snapshot receivers.
    #[must_use]
    pub fn new(config: SessionConfig) -> (Self, EventRx, SnapshotRx) {
        let (events, erx) = mpsc::unbounded_channel();
        let (snapshots, srx) = mpsc::unbounded_channel();
        // Discover the source-control repository (from the first root) before the
        // watcher, so its git-metadata directories can be watched for index/HEAD/refs
        // changes — that is what keeps the status fresh after external `git` commands.
        let vcs = config
            .roots
            .first()
            .and_then(|root| Repository::discover(root).ok());
        let git_dirs = vcs
            .as_ref()
            .map(Repository::metadata_dirs)
            .unwrap_or_default();
        let config_manager = ConfigManager::from_loaded(&config.loaded_config);
        let config_paths = config_manager
            .as_ref()
            .map(ConfigManager::paths)
            .unwrap_or_default();
        // Best-effort: a watcher failure (or no roots) just disables external-change
        // detection; editing still works.
        let (watcher, fs_rx) = if config.roots.is_empty() && config_paths.is_empty() {
            (None, None)
        } else {
            match Watcher::spawn_with_paths(&config.roots, &git_dirs, &config_paths) {
                Ok((w, rx)) => (Some(w), Some(rx)),
                Err(_) => (None, None),
            }
        };
        // Seed the tip so the first ref change reconciles against a known baseline.
        let last_head = vcs.as_ref().and_then(|r| r.head_hash().ok().flatten());
        let cancellations = crate::cancellation::CancellationHub::default();
        #[cfg(feature = "github")]
        let github_repository = github::eligible_repository(&config.roots, vcs.as_ref());
        let vcs_worker = crate::vcs_worker::spawn(config.roots.first().cloned(), events.clone());
        let search_worker = crate::search_worker::spawn(events.clone());
        let seam_worker = crate::seam_worker::spawn(events.clone(), config.seam_cache_dir.clone());
        let spell_scan_worker = crate::spell_scan::spawn(events.clone());
        let todo_scan_worker = crate::todo_scan::spawn(events.clone());
        let latex_worker = crate::latex::spawn(events.clone());
        // Open this session's swap store and scan for swaps a previous run left behind
        // (a crash, or a save that failed). They are offered to the UI for recovery.
        let session_id = u64::from(std::process::id());
        let swaps = config
            .swap_dir
            .clone()
            .map(|dir| SwapStore::with_dir(dir, session_id));
        let pending_swaps = if config.settings.files.backup {
            swaps
                .as_ref()
                .map(|store| scan(store.dir()))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        // Layered highlighting runs on its own thread; the actor only sends it text and
        // applies the spans it sends back. Each request carries the document's resolved
        // semantic-comment settings, so language overrides can update live.
        let (highlight_tx, highlight_rx) = crate::highlight::spawn();
        let (spell_tx, spell_rx) = crate::spell::spawn();
        #[cfg(feature = "mdlint")]
        let lint_config = super::mdlint::discover_config(&config.roots);
        // Language servers spawn lazily, per language, on the first matching open.
        let debug = crate::dap::DebugManager::new(
            config.settings.debug.clone(),
            config.roots.first().cloned(),
            config.process_supervisor.clone(),
            events.clone(),
        );
        #[cfg(feature = "notebook-kernel")]
        let notebooks = crate::notebook_kernel::NotebookKernels::new(
            config.process_supervisor.clone(),
            events.clone(),
        );
        let (lsp, lsp_rx) = LspManager::new(
            config.settings.lsp.clone(),
            config.roots.first().cloned(),
            config.process_supervisor.clone(),
            config.lsp_registry_dir.clone(),
        );
        let (lsp_registry, lsp_registry_rx) = crate::lsp_registry::spawn(
            config.lsp_registry_dir.clone(),
            config.process_supervisor.clone(),
        );
        let diff_syntax = config.diff_syntax;
        let mut session = Self {
            config,
            config_manager,
            events,
            snapshots,
            store: DocumentStore {
                next: 1,
                ..DocumentStore::default()
            },
            highlight_tx,
            highlight_rx: Some(highlight_rx),
            spell_tx,
            #[cfg(feature = "mdlint")]
            lint_config,
            spell_rx: Some(spell_rx),
            spell_errors: HashMap::new(),
            clock: Instant::now(),
            watcher,
            fs_rx,
            vcs,
            vcs_worker,
            search_worker,
            seam_worker,
            spell_scan_worker,
            todo_scan_worker,
            wakatime_worker: None,
            wakatime_last: None,
            wakatime_clock: std::time::Instant::now(),
            #[cfg(feature = "deps")]
            manifest_hints_worker: None,
            cancellations,
            latex_worker,
            diff_syntax,
            last_head,
            swaps,
            pending_swaps,
            debug,
            #[cfg(feature = "notebook-kernel")]
            notebooks,
            lsp,
            lsp_rx: Some(lsp_rx),
            lsp_registry,
            lsp_registry_rx: Some(lsp_registry_rx),
            #[cfg(feature = "github")]
            github_repository,
            #[cfg(feature = "github")]
            github_tx: None,
        };
        // Announce any recoverable swaps so the UI can prompt on the first frame.
        session.announce_pending_swaps();
        (session, EventRx(erx), SnapshotRx(srx))
    }

    /// Kick off the work deferred until the session is actually being driven: the
    /// initial VCS status. Computing it eagerly in [`Session::new`] would run a full
    /// `git status` on the construction thread — for a huge repository that can stall
    /// the caller before the UI ever renders. Call this once, from the actor task that
    /// drives [`Session::handle`]/[`Session::handle_fs_event`], so it runs
    /// concurrently with the first frame instead of blocking it.
    pub(crate) fn start(&mut self) {
        // Seed the client with the initial status; it buffers until the UI reads it.
        self.emit_vcs_status(None);
        #[cfg(feature = "github")]
        self.start_github();
        #[cfg(not(feature = "github"))]
        self.emit(
            None,
            Event::GithubAvailability {
                repository: None,
                auth: crate::api::GithubAuth {
                    source: crate::api::GithubAuthSource::Anonymous,
                    can_write: false,
                    viewer_id: None,
                    viewer_login: None,
                },
            },
        );
    }
}
