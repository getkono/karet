//! The rows of the color-theme picker.
//!
//! The picker overlay carries the chosen row back in
//! [`OverlayEvent::AcceptTheme`](super::OverlayEvent::AcceptTheme), so the row
//! model lives beside the overlay rather than in the app's picker logic
//! (`app::theme_picker`), which discovers the rows and applies the choice.

/// Where a theme the picker offers comes from, shown beside its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ThemeSource {
    /// Compiled in.
    BuiltIn,
    /// The user's `themes` directory.
    User,
    /// The project's `.karet/themes` directory.
    Project,
    /// Named by `workbench.colorTheme`, outside both directories.
    Settings,
}

impl ThemeSource {
    /// The hint shown at the right of the theme's row.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::BuiltIn => "built-in",
            Self::User => "user",
            Self::Project => "project",
            Self::Settings => "settings",
        }
    }
}

/// One theme the picker offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ThemeChoice {
    /// The row's name: the file stem, or `Dark` for the built-in theme.
    pub(crate) label: String,
    /// The `workbench.colorTheme` value that loads it.
    pub(crate) setting: String,
    /// Where it comes from.
    pub(crate) source: ThemeSource,
}

impl ThemeChoice {
    /// The built-in dark theme.
    pub(crate) fn built_in() -> Self {
        Self {
            label: "Dark".to_string(),
            setting: "dark".to_string(),
            source: ThemeSource::BuiltIn,
        }
    }
}
