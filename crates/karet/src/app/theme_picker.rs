//! Settings: Select Color Theme — a fuzzy picker over the themes karet can
//! load, previewing the highlighted one live.
//!
//! The candidates are the built-in dark theme, every VS Code `.json` theme in
//! a `themes` directory beside the user's and the project's `setting.jsonc`,
//! and the theme `workbench.colorTheme` names when it lives anywhere else.
//! Moving through the list repaints the whole UI in the highlighted theme;
//! Esc puts back the theme that was showing, Enter keeps the highlighted one.
//!
//! A pick lasts for the session and is never written to a settings file:
//! `workbench.colorTheme` stays the one place a theme is configured. A live
//! reload keeps the pick while it leaves `workbench.colorTheme` alone, and
//! drops it in favour of the setting once the setting itself changes.

use std::path::Path;
use std::path::PathBuf;

use karet_core::NotificationKind;
use karet_core::Severity;
use karet_session::ConfigDiagnostic;
use karet_theme::Theme;

use crate::app::App;
use crate::app::notifications::Report;
use crate::app::util::load_theme;
use crate::overlay::Overlay;
use crate::overlay::OverlayEvent;
use crate::overlay::Picker;
use crate::overlay::PickerKind;
use crate::overlay::ThemeChoice;
use crate::overlay::ThemeSource;

/// The picker's title. An open overlay is recognised as it by
/// [`PickerKind::ColorTheme`], not by this.
pub(crate) const THEME_PICKER_TITLE: &str = "Select Color Theme";

/// The directory, beside a `setting.jsonc`, that holds the themes offered.
pub(crate) const THEMES_DIR: &str = "themes";

/// The picker's preview and the session's pick.
#[derive(Debug, Default)]
pub(crate) struct ThemeUi {
    /// Set while the picker is open: what to put back if it is dismissed.
    preview: Option<Preview>,
    /// The theme chosen in the picker this session, while it still applies.
    pick: Option<Pick>,
}

/// An open picker's preview.
#[derive(Debug)]
struct Preview {
    /// The theme showing when the picker opened (or the one a live reload
    /// configured since): restored when the picker is dismissed.
    original: Theme,
    /// The setting of the theme now painted, `None` for `original`.
    shown: Option<String>,
}

/// A theme picked for the session.
#[derive(Debug)]
struct Pick {
    /// `workbench.colorTheme` when it was picked: a reload that changes the
    /// setting supersedes the pick.
    configured: String,
    /// The setting of the theme picked.
    setting: String,
}

/// The `themes` directories searched, user first: beside the user's
/// `setting.jsonc` and beside the project's.
pub(crate) fn theme_dirs(root: &Path) -> Vec<(ThemeSource, PathBuf)> {
    use karet_session::config::project_config_dir;
    use karet_session::config::user_config_dir;
    [
        user_config_dir().map(|dir| (ThemeSource::User, dir.join(THEMES_DIR))),
        project_config_dir(std::slice::from_ref(&root.to_path_buf()))
            .map(|dir| (ThemeSource::Project, dir.join(THEMES_DIR))),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The themes on offer: the built-in one, then each directory's `.json` files
/// by name, then `configured` when it names a theme file none of them holds.
/// A missing or unreadable directory offers nothing.
pub(crate) fn discover(dirs: &[(ThemeSource, PathBuf)], configured: &str) -> Vec<ThemeChoice> {
    let mut choices = vec![ThemeChoice::built_in()];
    for (source, dir) in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut found: Vec<ThemeChoice> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file() && is_json(path))
            .map(|path| ThemeChoice {
                label: stem(&path),
                setting: path.to_string_lossy().into_owned(),
                source: *source,
            })
            .collect();
        found.sort_by(|a, b| {
            a.label
                .cmp(&b.label)
                .then_with(|| a.setting.cmp(&b.setting))
        });
        choices.extend(found);
    }
    if !choices
        .iter()
        .any(|choice| same_theme(&choice.setting, configured))
    {
        choices.push(ThemeChoice {
            label: stem(Path::new(configured)),
            setting: configured.to_string(),
            source: ThemeSource::Settings,
        });
    }
    choices
}

/// Whether two `workbench.colorTheme` values load the same theme: both the
/// built-in one, or the same file however it is spelled.
pub(crate) fn same_theme(a: &str, b: &str) -> bool {
    let built_in = |s: &str| s.is_empty() || s == "dark";
    if built_in(a) || built_in(b) {
        return built_in(a) && built_in(b);
    }
    a == b
        || matches!(
            (std::fs::canonicalize(a), std::fs::canonicalize(b)),
            (Ok(a), Ok(b)) if a == b
        )
}

fn is_json(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

impl Overlay {
    /// The Select Color Theme picker over `choices`, with the one `current`
    /// loads highlighted.
    #[must_use]
    pub(crate) fn color_themes(choices: Vec<ThemeChoice>, current: &str) -> Self {
        let row = choices
            .iter()
            .position(|choice| same_theme(&choice.setting, current));
        let items = choices
            .into_iter()
            .map(|choice| (choice.label.clone(), OverlayEvent::AcceptTheme(choice)))
            .collect();
        let mut picker = Picker::new(THEME_PICKER_TITLE, items);
        for _ in 0..row.unwrap_or_default() {
            picker.select_down();
        }
        Self::picker(PickerKind::ColorTheme, picker)
    }

    /// The setting of the highlighted theme, when this is the theme picker
    /// and a row is highlighted.
    fn highlighted_theme(&self) -> Option<String> {
        match self.accept() {
            OverlayEvent::AcceptTheme(choice) => Some(choice.setting),
            _ => None,
        }
    }

    /// Whether this is the Select Color Theme picker.
    pub(super) fn is_theme_picker(&self) -> bool {
        self.picker_kind() == Some(PickerKind::ColorTheme)
    }
}

impl App {
    /// The `workbench.colorTheme` value the loaded settings hold.
    fn configured_theme(&self) -> String {
        self.loaded_config.settings.workbench.color_theme.clone()
    }

    /// Open the Select Color Theme picker over the themes in the user's and
    /// the project's `themes` directories.
    pub(super) fn open_theme_picker(&mut self) {
        let dirs = theme_dirs(&self.root);
        self.open_theme_picker_from(&dirs);
    }

    /// Open the Select Color Theme picker over the themes in `dirs`, with the
    /// theme now in effect highlighted.
    pub(crate) fn open_theme_picker_from(&mut self, dirs: &[(ThemeSource, PathBuf)]) {
        let configured = self.configured_theme();
        let current = self
            .theme_ui
            .pick
            .as_ref()
            .map_or_else(|| configured.clone(), |pick| pick.setting.clone());
        self.open_overlay(Overlay::color_themes(discover(dirs, &configured), &current));
        // Reopened over a preview that never settled, the theme to put back is
        // still the one from before it (replacing the open picker restored it).
        let original = match self.theme_ui.preview.take() {
            Some(preview) => preview.original,
            None => self.theme.clone(),
        };
        self.theme_ui.preview = Some(Preview {
            original,
            shown: None,
        });
        self.sync_theme_preview();
    }

    /// Follow the open picker: paint the theme on its highlighted row. A theme
    /// that fails to load previews as the one from before the picker. Closing
    /// the picker puts that theme back ([`App::close_overlay`]).
    pub(crate) fn sync_theme_preview(&mut self) {
        let Some(preview) = self.theme_ui.preview.as_mut() else {
            return;
        };
        let Some(overlay) = self.keyboard.overlay().filter(|o| o.is_theme_picker()) else {
            return;
        };
        let highlighted = overlay.highlighted_theme();
        if highlighted == preview.shown {
            return;
        }
        self.theme = highlighted
            .as_deref()
            .and_then(|setting| load_theme(setting).ok())
            .unwrap_or_else(|| preview.original.clone());
        preview.shown = highlighted;
    }

    /// Put back the theme from before the picker, ending its preview.
    pub(super) fn restore_theme_preview(&mut self) {
        if let Some(preview) = self.theme_ui.preview.take() {
            self.theme = preview.original;
        }
    }

    /// Apply the theme accepted in the picker for the rest of the session.
    pub(super) fn accept_theme(&mut self, choice: ThemeChoice) {
        let original = match self.theme_ui.preview.take() {
            Some(preview) => preview.original,
            None => self.theme.clone(),
        };
        let theme = match load_theme(&choice.setting) {
            Ok(theme) => theme,
            Err(message) => {
                self.theme = original;
                self.notify(Report::Failure, NotificationKind::System, message);
                return;
            },
        };
        self.theme = theme;
        let configured = self.configured_theme();
        if same_theme(&choice.setting, &configured) {
            self.theme_ui.pick = None;
            self.notify(
                Report::Outcome,
                NotificationKind::System,
                format!("color theme: {}", choice.label),
            );
            return;
        }
        self.notify(
            Report::Outcome,
            NotificationKind::System,
            format!(
                "color theme: {} for this session; set workbench.colorTheme to keep it",
                choice.label
            ),
        );
        self.theme_ui.pick = Some(Pick {
            configured,
            setting: choice.setting,
        });
    }

    /// Apply the theme a (re)loaded configuration asks for: the session's pick
    /// while `configured` is still what it was picked over, otherwise
    /// `configured` itself. While the picker is open the theme goes behind the
    /// preview, so dismissing the picker shows it. A failure is a config
    /// diagnostic, and leaves the theme as it was.
    pub(super) fn apply_configured_theme(&mut self, configured: &str) {
        let wanted = match self.theme_ui.pick.as_ref() {
            Some(pick) if pick.configured == configured => pick.setting.clone(),
            _ => {
                self.theme_ui.pick = None;
                configured.to_string()
            },
        };
        match load_theme(&wanted) {
            Ok(theme) => {
                if let Some(preview) = self.theme_ui.preview.as_mut() {
                    preview.original = theme.clone();
                    // Show the new theme until the sync below repaints the
                    // highlighted row over it, reread in case it was the file
                    // that changed.
                    preview.shown = None;
                }
                self.theme = theme;
                self.sync_theme_preview();
            },
            Err(message) => self.config_diagnostics.push(ConfigDiagnostic {
                path: PathBuf::from(&wanted),
                message,
                severity: Severity::Warning,
            }),
        }
    }
}
