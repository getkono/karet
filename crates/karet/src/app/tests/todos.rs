//! The Todos panel as it is painted: what survives a narrow sidebar, and how
//! the two groupings differ.

use karet_session::TodoHit;

use super::support::*;
use crate::app::*;

fn hit(path: &str, line: u32, tag: &str, message: &str) -> TodoHit {
    TodoHit {
        path: std::path::PathBuf::from(path),
        line,
        tag: tag.to_owned(),
        message: message.to_owned(),
    }
}

/// An app showing the Todos panel over three hits in three files.
fn todos_app(by_tag: bool) -> App {
    let mut app = app();
    app.root = std::path::PathBuf::from("/repo");
    app.sidebar_panel = SidebarPanel::Todos;
    app.sidebar_visible = true;
    app.todos.hits = vec![
        hit(
            "/repo/crates/karet-dap/Cargo.toml",
            24,
            "TODO",
            "DAP protocol types — hand-roll over serde, or adopt the crate",
        ),
        hit(
            "/repo/crates/karet-session/src/spell/context.rs",
            8,
            "TODO",
            "replace these textual contexts with grammar queries",
        ),
        hit("/repo/crates/karet/src/app/github.rs", 76, "FIXME", "short"),
    ];
    app.todos.scanned = true;
    app.todos.files_scanned = 467;
    app.todos.by_tag = by_tag;
    app.todos.rebuild_rows();
    app
}

#[test]
fn grouping_by_tag_keeps_the_location_visible_in_a_narrow_sidebar() {
    // The file:line is what tells two rows under one tag apart, so a long
    // message must yield to it rather than push it off the edge.
    let mut app = todos_app(true);
    let painted = screen(&mut app, 120, 12).join("\n");

    assert!(painted.contains("Cargo.toml:25"), "{painted}");
    assert!(painted.contains("context.rs:9"), "{painted}");
    assert!(painted.contains("github.rs:77"), "{painted}");
}

#[test]
fn grouping_by_tag_drops_the_tag_the_group_header_already_names() {
    let mut app = todos_app(true);
    let rows = screen(&mut app, 120, 12);
    let Some(header) = rows.iter().find(|r| r.contains("TODO (2)")) else {
        panic!("expected a TODO group header:\n{}", rows.join("\n"));
    };
    assert!(header.contains("TODO (2)"), "{header}");

    // The entries under it do not repeat the tag.
    let entry = rows
        .iter()
        .find(|r| r.contains("Cargo.toml:25"))
        .map(String::as_str)
        .unwrap_or_default();
    assert!(
        !entry.contains("TODO"),
        "the row repeats its group's tag: {entry}"
    );
}

#[test]
fn grouping_by_file_keeps_the_tag_which_distinguishes_rows_there() {
    // Under a file header the tag is the useful part — one file can hold a
    // TODO and a FIXME — and the line number is short enough to always fit.
    let mut app = todos_app(false);
    let rows = screen(&mut app, 120, 14);
    let painted = rows.join("\n");

    assert!(painted.contains("crates/karet-dap/Cargo.toml"), "{painted}");
    let Some(entry) = rows.iter().find(|r| r.contains("DAP protocol")) else {
        panic!("expected the hit's row:\n{painted}");
    };
    assert!(entry.contains("TODO"), "the tag leads the row: {entry}");
    assert!(entry.contains("25"), "the line number survives: {entry}");
}

#[test]
fn a_short_message_is_left_alone() {
    let mut app = todos_app(true);
    let painted = screen(&mut app, 120, 12).join("\n");
    assert!(
        painted.contains("short github.rs:77"),
        "a message that fits keeps its ellipsis off:\n{painted}"
    );
}
