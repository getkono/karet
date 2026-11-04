//! The Seam package picker: discovery landing while the picker is open, after
//! it closed, and on reopening. The view's navigation model is covered beside
//! it, in `tab::seam`.

use std::path::PathBuf;

use super::roots;

/// A Cargo workspace with members `alpha` and `beta`, an app rooted on it, and the
/// receiver its package discoveries arrive on.
fn picker_app() -> (
    tempfile::TempDir,
    crate::app::App,
    tokio::sync::mpsc::UnboundedReceiver<roots::Discovery>,
) {
    let dir = tempfile::Builder::new()
        .prefix("karet-seam-picker")
        .tempdir()
        .expect("temp dir");
    let _ = std::fs::write(
        dir.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"alpha\", \"beta\"]\n",
    );
    for name in ["alpha", "beta"] {
        let _ = std::fs::create_dir_all(dir.path().join(name).join("src"));
        let _ = std::fs::write(
            dir.path().join(name).join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\n"),
        );
    }
    let mut app = crate::app::App::new(dir.path().to_path_buf(), Vec::new(), Vec::new(), false);
    let rx = app.seam_discovery.take_receiver().expect("receiver");
    (dir, app, rx)
}

/// Wait for the discovery in flight and hand it to the app, as the event loop does.
fn land(
    app: &mut crate::app::App,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<roots::Discovery>,
) {
    let discovery = rx.blocking_recv().expect("a discovery lands");
    app.on_seam_discovery(discovery);
}

fn picker(app: &mut crate::app::App) -> &mut crate::overlay::Picker<crate::overlay::OverlayEvent> {
    match app.keyboard.overlay_mut() {
        Some(crate::overlay::Overlay::Picker { picker, .. }) => picker,
        _ => panic!("a picker is open"),
    }
}

fn picker_rows(app: &mut crate::app::App) -> Vec<String> {
    picker(app)
        .rows()
        .iter()
        .map(|row| (*row).to_owned())
        .collect()
}

fn highlighted(app: &mut crate::app::App) -> Option<PathBuf> {
    match picker(app).accepted() {
        Some(crate::overlay::OverlayEvent::AcceptSeamRoot(root)) => Some(root.clone()),
        _ => None,
    }
}

fn package_rows(app: &mut crate::app::App) -> usize {
    picker_rows(app)
        .iter()
        .filter(|row| row.ends_with("· package"))
        .count()
}

#[test]
fn the_seam_picker_opens_before_discovery_and_fills_in_when_it_lands() {
    let (_dir, mut app, mut rx) = picker_app();
    app.open_seam_view_picker();
    // Open at once, workspace root first, with no package discovered yet.
    assert!(picker_rows(&mut app)[0].contains("workspace root"));
    assert_eq!(package_rows(&mut app), 0);

    land(&mut app, &mut rx);
    let rows = picker_rows(&mut app);
    assert!(rows[0].contains("workspace root"), "got {rows:?}");
    assert!(
        rows.iter().any(|row| row.starts_with("beta")),
        "got {rows:?}"
    );
}

#[test]
fn a_reopened_seam_picker_offers_the_known_packages_and_keeps_the_highlight_when_they_refresh() {
    let (_dir, mut app, mut rx) = picker_app();
    app.open_seam_view_picker();
    land(&mut app, &mut rx);
    app.close_overlay();

    app.open_seam_view_picker();
    assert!(
        package_rows(&mut app) > 0,
        "known packages are offered at once"
    );
    for _ in 1..picker_rows(&mut app).len() {
        picker(&mut app).select_down();
    }
    let chosen = highlighted(&mut app);
    assert!(picker(&mut app).selected() > 0);

    // The refresh replaces every row; the reader's highlight must not jump to the top.
    land(&mut app, &mut rx);
    assert_eq!(highlighted(&mut app), chosen);
}

#[test]
fn a_discovery_landing_after_the_picker_closed_leaves_the_overlay_in_its_place_alone() {
    let (_dir, mut app, mut rx) = picker_app();
    app.open_seam_view_picker();
    app.open_overlay(crate::overlay::Overlay::command_palette());
    let before = picker_rows(&mut app);

    land(&mut app, &mut rx);
    assert_eq!(picker(&mut app).title(), "Command Palette");
    assert_eq!(picker_rows(&mut app), before);
}
