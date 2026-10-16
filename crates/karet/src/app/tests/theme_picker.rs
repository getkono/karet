use super::support::*;
use crate::app::theme_picker::THEME_PICKER_TITLE;
use crate::app::theme_picker::ThemeSource;
use crate::app::theme_picker::discover;
use crate::app::theme_picker::same_theme;
use crate::app::*;

/// A VS Code theme whose editor background is `hex`, so each one loads to a
/// theme distinct from the built-in dark one and from each other.
fn theme_json(hex: &str) -> String {
    format!(r#"{{ "colors": {{ "editor.background": "{hex}" }} }}"#)
}

/// A scratch `themes` directory holding `files` (name, contents).
fn themes_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("temp dir: {e}"));
    for (name, contents) in files {
        let _ = std::fs::write(dir.path().join(name), contents);
    }
    dir
}

fn loaded(path: &Path) -> Theme {
    load_theme(&path.to_string_lossy()).unwrap_or_else(|e| panic!("{e}"))
}

fn rows(app: &App) -> Vec<String> {
    app.overlay
        .as_ref()
        .map(|overlay| overlay.rows().into_iter().map(str::to_string).collect())
        .unwrap_or_default()
}

/// An app over a `themes` directory holding `amber.json` and `blue.json`,
/// with the picker open on it.
fn picker_app() -> (tempfile::TempDir, App) {
    let dir = themes_dir(&[
        ("blue.json", &theme_json("#0000ff")),
        ("amber.json", &theme_json("#ffbf00")),
    ]);
    let mut app = app();
    app.open_theme_picker_from(&[(ThemeSource::User, dir.path().to_path_buf())]);
    (dir, app)
}

fn key(app: &mut App, code: KeyCode) {
    send_key(app, code, KeyModifiers::NONE);
}

#[test]
fn discovery_offers_the_built_in_then_each_directory_by_name() {
    let user = themes_dir(&[
        ("zeta.json", "{}"),
        ("alpha.JSON", "{}"),
        ("notes.txt", "not a theme"),
    ]);
    let project = themes_dir(&[("mid.json", "{}")]);
    let choices = discover(
        &[
            (ThemeSource::User, user.path().to_path_buf()),
            (ThemeSource::Project, project.path().to_path_buf()),
            (
                ThemeSource::Project,
                PathBuf::from("/karet-test/no/such/dir"),
            ),
        ],
        "dark",
    );
    let listed: Vec<(&str, ThemeSource)> = choices
        .iter()
        .map(|choice| (choice.label.as_str(), choice.source))
        .collect();
    assert_eq!(
        listed,
        [
            ("Dark", ThemeSource::BuiltIn),
            ("alpha", ThemeSource::User),
            ("zeta", ThemeSource::User),
            ("mid", ThemeSource::Project),
        ],
        "non-JSON files and a missing directory offer nothing"
    );
    assert_eq!(choices[0].setting, "dark");
}

#[test]
fn the_configured_theme_is_offered_once_wherever_it_lives() {
    let dir = themes_dir(&[("inside.json", "{}")]);
    let elsewhere = themes_dir(&[("outside.json", "{}")]);
    let dirs = [(ThemeSource::User, dir.path().to_path_buf())];

    let outside = elsewhere.path().join("outside.json");
    let choices = discover(&dirs, &outside.to_string_lossy());
    let last = choices.last().map(|c| (c.label.as_str(), c.source));
    assert_eq!(last, Some(("outside", ThemeSource::Settings)));

    // The same file, spelled through `.`, is the row already listed.
    let inside = dir.path().join(".").join("inside.json");
    let choices = discover(&dirs, &inside.to_string_lossy());
    assert_eq!(choices.len(), 2, "{choices:?}");
    assert!(choices.iter().all(|c| c.source != ThemeSource::Settings));
}

#[test]
fn same_theme_treats_empty_as_dark_and_compares_files() {
    assert!(same_theme("", "dark"));
    assert!(!same_theme("dark", "/karet-test/dark"));
    assert!(same_theme("/karet-test/a.json", "/karet-test/a.json"));
    assert!(!same_theme("/karet-test/a.json", "/karet-test/b.json"));
}

#[test]
fn the_picker_is_in_the_palette_and_opens_on_the_theme_in_effect() {
    assert!(crate::command::palette().contains(&Command::SelectColorTheme));
    let mut app = app();
    app.dispatch(Command::SelectColorTheme);
    let overlay = app.overlay.as_ref();
    assert_eq!(overlay.map(Overlay::title), Some(THEME_PICKER_TITLE));
    assert_eq!(overlay.map(Overlay::selected), Some(0));
    assert_eq!(rows(&app).first().map(String::as_str), Some("Dark"));
    assert_eq!(
        overlay
            .map(Overlay::row_hints)
            .and_then(|h| h.first().cloned()),
        Some(Some("built-in".to_string()))
    );
}

#[test]
fn moving_through_the_list_previews_each_theme() {
    let (dir, mut app) = picker_app();
    assert_eq!(rows(&app), ["Dark", "amber", "blue"]);
    assert_eq!(app.theme, Theme::dark());
    key(&mut app, KeyCode::Down);
    assert_eq!(app.theme, loaded(&dir.path().join("amber.json")));
    key(&mut app, KeyCode::Down);
    assert_eq!(app.theme, loaded(&dir.path().join("blue.json")));
    // Typing filters, and the preview follows the row now highlighted.
    for c in "amb".chars() {
        key(&mut app, KeyCode::Char(c));
    }
    assert_eq!(rows(&app), ["amber"]);
    assert_eq!(app.theme, loaded(&dir.path().join("amber.json")));
    // A query matching nothing shows the theme from before the picker.
    key(&mut app, KeyCode::Char('q'));
    assert!(rows(&app).is_empty());
    assert_eq!(app.theme, Theme::dark());
}

#[test]
fn escape_puts_back_the_theme_from_before_the_picker() {
    let (_dir, mut app) = picker_app();
    key(&mut app, KeyCode::Down);
    assert_ne!(app.theme, Theme::dark());
    key(&mut app, KeyCode::Esc);
    assert!(app.overlay.is_none());
    assert_eq!(app.theme, Theme::dark());
}

#[test]
fn enter_keeps_the_highlighted_theme_for_the_session() {
    let (dir, mut app) = picker_app();
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    assert!(app.overlay.is_none());
    let amber = loaded(&dir.path().join("amber.json"));
    assert_eq!(app.theme, amber);
    assert_eq!(
        last_message(&app).as_deref(),
        Some("color theme: amber for this session; set workbench.colorTheme to keep it")
    );
    // Reopened, the picker starts on the pick.
    app.open_theme_picker_from(&[(ThemeSource::User, dir.path().to_path_buf())]);
    assert_eq!(app.overlay.as_ref().map(Overlay::selected), Some(1));
    assert_eq!(app.theme, amber);
}

#[test]
fn a_reload_keeps_the_pick_until_the_setting_itself_changes() {
    let (dir, mut app) = picker_app();
    key(&mut app, KeyCode::Down);
    key(&mut app, KeyCode::Enter);
    let amber = loaded(&dir.path().join("amber.json"));

    // An unrelated settings change leaves the pick in place.
    let mut config = app.loaded_config.clone();
    config.settings.editor.tab_size += 1;
    app.apply_loaded_config(config.clone(), false);
    assert_eq!(app.theme, amber);

    // Setting workbench.colorTheme supersedes it.
    let blue = dir.path().join("blue.json");
    config.settings.workbench.color_theme = blue.to_string_lossy().into_owned();
    app.apply_loaded_config(config.clone(), false);
    assert_eq!(app.theme, loaded(&blue));

    // And setting it back does not resurrect the pick.
    config.settings.workbench.color_theme = "dark".to_string();
    app.apply_loaded_config(config, false);
    assert_eq!(app.theme, Theme::dark());
}

#[test]
fn a_reload_while_previewing_changes_what_escape_restores() {
    let (dir, mut app) = picker_app();
    key(&mut app, KeyCode::Down);
    let mut config = app.loaded_config.clone();
    let blue = dir.path().join("blue.json");
    config.settings.workbench.color_theme = blue.to_string_lossy().into_owned();
    app.apply_loaded_config(config, false);
    assert_eq!(
        app.theme,
        loaded(&dir.path().join("amber.json")),
        "the highlighted row still previews"
    );
    key(&mut app, KeyCode::Esc);
    assert_eq!(app.theme, loaded(&blue));
}

#[test]
fn a_theme_that_fails_to_load_is_reported_and_changes_nothing() {
    let dir = themes_dir(&[("broken.json", "{ not json")]);
    let mut app = app();
    app.open_theme_picker_from(&[(ThemeSource::User, dir.path().to_path_buf())]);
    key(&mut app, KeyCode::Down);
    assert_eq!(app.theme, Theme::dark(), "the preview keeps the old theme");
    key(&mut app, KeyCode::Enter);
    assert_eq!(app.theme, Theme::dark());
    let (severity, message) = last_report(&app).unwrap_or((Severity::Hint, String::new()));
    assert_eq!(severity, Severity::Error);
    assert!(message.contains("broken.json"), "{message}");
}
