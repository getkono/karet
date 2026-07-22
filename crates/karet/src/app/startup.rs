use super::*;

impl App {
    /// Build the shell rooted at `root`, with the staged/working change groups for
    /// the Source-Control panel.
    #[must_use]
    pub fn new(
        root: PathBuf,
        staged: Vec<FileChange>,
        working: Vec<FileChange>,
        syntax: bool,
    ) -> Self {
        let staged_count = staged.len();
        let mut changes = staged;
        changes.extend(working);
        let graphics = image::detect_protocol();
        Self {
            root,
            settings: Settings::default(),
            loaded_config: LoadedConfig::default(),
            config_diagnostics: Vec::new(),
            theme: Theme::dark(),
            syntax,
            icon_style: IconStyle::default(),
            icon_override: None,
            graphics,
            kitty_graphics_supported: graphics == GraphicsProtocol::Kitty,
            kitty_keyboard_supported: false,
            pointer_shapes_supported: false,
            pointer_shape: None,
            focus: Focus::Sidebar,
            sidebar_panel: SidebarPanel::Explorer,
            sidebar_visible: true,
            explorer: FileTreeState::new(),
            explorer_clipboard: None,
            context_menu: None,
            scm: Scm {
                selection: ListSelection::new(changes.len()),
                changes,
                staged_count,
                log: Vec::new(),
                log_has_more: false,
                log_loading: false,
                log_loading_since: None,
                repository: None,
                repository_loading_since: None,
                repository_request: None,
                operation: None,
            },
            live_blame: None,
            pending_blame: None,
            failed_blame: None,
            pending_pull_requests: None,
            pull_request_items: Vec::new(),
            pull_request_remote: None,
            vcs_after_save: None,
            tabs: vec![Tab::welcome()],
            active: 0,
            layout: PaneLayout::new(),
            stored: HashMap::new(),
            closed: Vec::new(),
            overlay: None,
            find_open: false,
            commit_input: CommitInput::default(),
            rev_input: None,
            pending_discard: None,
            pending_explorer_delete: None,
            pending_close: None,
            saving_close: None,
            pending_swaps: None,
            pending: Vec::new(),
            search: SearchPanel::default(),
            status: None,
            notifications: NotificationCenter::default(),
            toast_hits: Vec::new(),
            sidebar_rect: Rect::default(),
            main_rect: Rect::default(),
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            sidebar_divider_x: 0,
            sidebar_resizing: false,
            diff_layout: ViewMode::Unified,
            pane_frames: Vec::new(),
            tab_drag: None,
            sidebar_content_rect: Rect::default(),
            hover: None,
            sidebar_header_hover: None,
            panel_hits: Vec::new(),
            outline_visible: false,
            outline_sel: ListSelection::new(0),
            outline_rect: Rect::default(),
            outline_content_rect: Rect::default(),
            outline_width: OUTLINE_WIDTH,
            outline_scroll: 0,
            header_action_hits: Vec::new(),
            scm_row_map: Vec::new(),
            scm_header_hits: Vec::new(),
            scm_offset: 0,
            scm_changes_rect: Rect::default(),
            scm_commit_rect: Rect::default(),
            scm_total_rows: 0,
            scm_commits_offset: 0,
            scm_commits_rect: Rect::default(),
            scm_commits_total: 0,
            scm_more_row: None,
            scm_commits_h: DEFAULT_SCM_COMMITS_H,
            scm_divider_y: 0,
            scm_resizing: false,
            search_results_rect: Rect::default(),
            search_offset: 0,
            search_query_row: 0,
            search_replace_row: None,
            search_action_hits: Vec::new(),
            status_rect: Rect::default(),
            status_hits: Vec::new(),
            editor_rect: Rect::default(),
            blame_rect: None,
            markdown_link_hits: Vec::new(),
            markdown_link_hover: None,
            commit_badge_rect: None,
            editor_selecting: false,
            last_click: None,
            click_streak: 0,
            clipboard: Clipboard::new(),
            image_area: None,
            shown_image: None,
            shown_page: 0,
            shown_graphics_caret: None,
            graphics_caret_blink_epoch: Instant::now(),
            should_quit: false,
            backend: None,
            pending_open: HashMap::new(),
            pending_saves: HashMap::new(),
            pending_completion: None,
            completion: None,
            completion_matcher: karet_fuzzy::Matcher::new(),
            pending_commit_detail: HashMap::new(),
            graph_log_req: None,
            open_docs: HashSet::new(),
            next_view: 1,
        }
    }

    /// Set the icon style (builder-style; chains off [`App::new`]).
    #[must_use]
    pub fn with_icons(mut self, style: IconStyle) -> Self {
        self.icon_style = style;
        self.icon_override = Some(style);
        self
    }

    /// Apply the loaded configuration to the UI shell (builder-style). Stores the
    /// settings (later handed to the session backend) and any load diagnostics (shown
    /// as startup notifications), and applies the `workbench.*` slice: colour theme,
    /// icon style, and the startup sidebar panel.
    #[must_use]
    pub fn with_settings(mut self, settings: Settings, diagnostics: Vec<ConfigDiagnostic>) -> Self {
        let mut loaded = LoadedConfig::from_settings(settings);
        loaded.diagnostics = diagnostics;
        self = self.with_loaded_config(loaded);
        self
    }

    /// Apply a loaded configuration report to the UI shell (builder-style).
    #[must_use]
    pub fn with_loaded_config(mut self, loaded: LoadedConfig) -> Self {
        self.apply_loaded_config(loaded, true);
        self
    }

    /// Apply a configuration snapshot. Live reload deliberately leaves the startup
    /// panel alone; it is a startup action rather than persistent UI state.
    pub(super) fn apply_loaded_config(&mut self, loaded: LoadedConfig, apply_startup_panel: bool) {
        use karet_session::config::schema::IconStyleSetting;
        use karet_session::config::schema::StartupPanel;

        let settings = loaded.settings.clone();
        self.config_diagnostics = loaded.diagnostics.clone();
        self.loaded_config = loaded;

        // Theme: the built-in "dark", or a path to a .tmTheme / VS Code .json theme.
        match load_theme(&settings.workbench.color_theme) {
            Ok(theme) => self.theme = theme,
            Err(message) => self.config_diagnostics.push(ConfigDiagnostic {
                path: PathBuf::from(&settings.workbench.color_theme),
                message,
                severity: Severity::Warning,
            }),
        }

        if self.icon_override.is_none() {
            self.icon_style = match settings.workbench.icon_style {
                IconStyleSetting::NerdFont => IconStyle::NerdFont,
                IconStyleSetting::Unicode => IconStyle::Unicode,
                IconStyleSetting::Ascii => IconStyle::Ascii,
            };
        }

        if apply_startup_panel {
            match settings.workbench.startup_panel {
                StartupPanel::Explorer => {
                    self.sidebar_panel = SidebarPanel::Explorer;
                    self.sidebar_visible = true;
                },
                StartupPanel::Search => {
                    self.sidebar_panel = SidebarPanel::Search;
                    self.sidebar_visible = true;
                },
                StartupPanel::SourceControl => {
                    self.sidebar_panel = SidebarPanel::SourceControl;
                    self.sidebar_visible = true;
                },
                StartupPanel::None => self.sidebar_visible = false,
            }
        }

        self.settings = settings;
    }

    /// Open `path` as the initial tab at startup (used when `karet <file>` is run).
    pub fn open_initial(&mut self, path: &Path) {
        self.open_path(path);
    }

    /// Open `path` as a startup preview without stealing focus from the configured
    /// startup panel.
    pub fn open_initial_preview(&mut self, path: &Path) {
        self.open_path_preview(path, false);
    }

    /// Open `path` at startup and place the caret at 1-based `line`/`col` (from the
    /// `--goto` flag), then focus the editor. The file opens as a permanent tab (or
    /// re-focuses an already-open one); the target is converted to the editor's
    /// 0-based coordinates and clamped into the buffer. A non-text target (image,
    /// binary, …) simply opens with no caret to place.
    pub fn open_startup_goto(&mut self, path: &Path, line: u32, col: u32) {
        self.open_path(path);
        // `line`/`col` are 1-based with a minimum of 1; `saturating_sub` maps them to
        // the editor's 0-based coordinates, and `goto` clamps into the buffer.
        let pos = LineCol::new(line.saturating_sub(1), col.saturating_sub(1));
        let buffer = match self.tabs.get(self.active).map(|t| &t.kind) {
            Some(TabKind::Code { buffer, .. }) => Some(buffer.clone()),
            _ => None,
        };
        if let (Some(buffer), Some(tab)) = (buffer, self.tabs.get_mut(self.active)) {
            tab.editor.goto(&buffer, pos);
        }
        self.focus = Focus::Editor;
    }

    /// Open `path` in a new right split at startup (from the `--split` flag): the
    /// file becomes the sole tab of a fresh pane to the right of the focused pane,
    /// which then takes focus, so repeated calls chain panes left-to-right. When the
    /// layout has no room for another pane (per `can_split` against
    /// [`Self::main_rect`], which the caller seeds with the terminal size before the
    /// first draw), the file opens as a tab in the current pane instead and a
    /// warning notification says so — automation gets its file either way.
    pub fn open_startup_split(&mut self, path: &Path) {
        let from = self.focus_pane();
        if !self.layout.can_split(from, SplitDir::Right, self.main_rect) {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file")
                .to_string();
            self.notify(
                Severity::Warning,
                NotificationKind::System,
                format!("--split: no room for another pane; opened {name} in the current pane"),
            );
            self.open_path(path);
            return;
        }
        let mut tab = workspace::open_file(path);
        tab.view = self.alloc_view();
        self.stash_focused();
        let new_pane = self.layout.split(from, SplitDir::Right);
        self.stored.insert(
            new_pane,
            StoredPane {
                tabs: vec![tab],
                active: 0,
            },
        );
        // `split` already focuses the pane it created; make that explicit, then pull
        // the new pane's tabs live and register its document with the backend.
        self.layout.set_focus(new_pane);
        self.load_focused();
        self.focus = Focus::Editor;
        self.register_doc(self.active);
    }

    /// Open a diff of two arbitrary files as a startup tab (from the `--diff`
    /// flag): `old` renders as the "before" side and `new` as the "after",
    /// syntax-aware like any Source-Control diff. `old_text`/`new_text` carry each
    /// file's content, already read by the caller (which fails fast on an unreadable
    /// file), with `None` marking non-UTF-8 bytes — either side non-text flags the
    /// change binary, rendering the standard binary-change placeholder (matching the
    /// [`FileChange::is_binary`] contract that both texts are then empty).
    pub fn open_startup_diff(
        &mut self,
        old: &Path,
        new: &Path,
        old_text: Option<String>,
        new_text: Option<String>,
    ) {
        let is_binary = old_text.is_none() || new_text.is_none();
        let change = FileChange {
            path: new.to_path_buf(),
            // The "renamed from" marker only applies when the two sides differ.
            old_path: (old != new).then(|| old.to_path_buf()),
            status: StatusKind::Modified,
            is_binary,
            old: if is_binary {
                String::new()
            } else {
                old_text.unwrap_or_default()
            },
            new: if is_binary {
                String::new()
            } else {
                new_text.unwrap_or_default()
            },
        };
        let name = |p: &Path| {
            p.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("diff")
                .to_string()
        };
        let (old_name, new_name) = (name(old), name(new));
        let title = if old_name == new_name {
            new_name
        } else {
            format!("{old_name} ↔ {new_name}")
        };
        let file = FileView::new(change, Section::Working, self.syntax);
        self.push_tab(Tab::new(
            title,
            TabKind::Diff {
                file: Box::new(file),
                view: self.diff_layout,
                scroll: 0,
            },
        ));
    }

    /// Dispatch a palette command at startup (from the `--command` flag), after
    /// every other startup flag is applied. Runs through the same
    /// [`dispatch`](Self::dispatch) path a key binding or the palette uses, so CLI
    /// automation and interactive use cannot drift.
    pub fn apply_startup_command(&mut self, command: Command) {
        self.dispatch(command);
    }

    /// Apply the CLI's startup focus override after startup tabs are opened.
    pub fn apply_startup_focus(&mut self, focus: crate::cli::FocusChoice) {
        self.focus = match focus {
            crate::cli::FocusChoice::Sidebar if self.sidebar_visible => Focus::Sidebar,
            crate::cli::FocusChoice::Sidebar | crate::cli::FocusChoice::Editor => Focus::Editor,
        };
    }

    /// Whether the active tab is a diff (enables diff-specific keys).
    pub(super) fn active_is_diff(&self) -> bool {
        self.tabs.get(self.active).is_some_and(Tab::is_diff)
    }

    /// The content kind of the active editor tab, mapping the shell's tab model
    /// down to the coarse [`EditorTab`] the keymap layers on. Read-only scrollable
    /// views ([`EditorTab::Pager`]) scroll on the arrows; a too-large placeholder
    /// gets its own "open anyway" layer; a diff its layout/next-change keys; every
    /// other tab is [`EditorTab::Plain`].
    pub(super) fn active_editor_tab(&self) -> EditorTab {
        match self.tabs.get(self.active).map(|t| &t.kind) {
            Some(TabKind::Diff { .. }) => EditorTab::Diff,
            Some(
                TabKind::CommitLoading { .. }
                | TabKind::Commit { .. }
                | TabKind::Compare { .. }
                | TabKind::StashPreview { .. }
                | TabKind::Graph { .. }
                | TabKind::LoadedConfig { .. }
                | TabKind::MarkdownPreview { .. }
                | TabKind::Hex { .. },
            ) => EditorTab::Pager,
            Some(TabKind::Github(_)) => EditorTab::Github,
            Some(TabKind::CommitGraph { .. }) => EditorTab::CommitGraph,
            Some(TabKind::Placeholder {
                kind: FileKind::TooLarge { .. },
                ..
            }) => EditorTab::Oversize,
            _ => EditorTab::Plain,
        }
    }

    /// The pane that currently holds keyboard focus — the single value that
    /// determines which keybinding layer is live.
    pub(crate) fn focus_target(&self) -> FocusTarget {
        FocusTarget::from(self.focus, self.sidebar_panel, self.active_editor_tab())
    }

    /// Whether the active frame should suppress the editor's cell caret because the
    /// app will draw the Kitty graphics caret after ratatui flushes the frame.
    pub(crate) fn graphical_cursor_enabled(&self) -> bool {
        let configured =
            self.tabs
                .get(self.active)
                .map_or(self.settings.editor.graphical_cursor, |tab| {
                    self.settings
                        .editor
                        .for_language(tab_language(tab))
                        .graphical_cursor()
                });
        match configured {
            Some(false) => false,
            Some(true) => self.graphical_cursor_compatible(),
            None => self.graphical_cursor_compatible(),
        }
    }

    pub(super) fn graphical_cursor_compatible(&self) -> bool {
        self.kitty_keyboard_supported
            && self.kitty_graphics_supported
            && self.graphics == GraphicsProtocol::Kitty
    }
}
