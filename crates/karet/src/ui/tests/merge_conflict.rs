//! The merge-conflict view's three panes scroll as one.

use crossterm::event::KeyCode;
use crossterm::event::KeyModifiers;

use crate::app::tests::support::app;
use crate::app::tests::support::screen;
use crate::app::tests::support::send_key;
use crate::app::tests::support::text_tab;
use crate::keymap::Focus;
use crate::tab::MergeConflictState;

/// A line far wider than any of the three panes, ending in `END`.
fn wide_line(tag: char) -> String {
    format!("{tag}{}END\n", "abcdefghijklmnopqrstuvwxyz".repeat(5))
}

#[test]
fn side_panes_scroll_with_the_merged_caret_in_the_same_frame() {
    let mut app = app();
    app.tabs.push(text_tab("a.rs", &wide_line('M')));
    app.active = app.tabs.len() - 1;
    let mut conflict = MergeConflictState::loading();
    conflict.finish(wide_line('C'), wide_line('I'));
    app.tabs[app.active].merge_conflict = Some(conflict);
    app.focus = Focus::Editor;
    // The first frame measures the panes the reveal is resolved against.
    let _ = screen(&mut app, 120, 12);

    // End puts the caret past the merged pane's right margin (wrap is off for
    // `.rs`). The sides copy the merged offset before the merged editor
    // paints, so it has to be right as soon as the motion returns.
    send_key(&mut app, KeyCode::End, KeyModifiers::NONE);
    let painted = screen(&mut app, 120, 12);

    let tab = &app.tabs[app.active];
    let merged = tab.editor.scroll_col;
    assert!(merged > 0, "End scrolled the merged pane");
    let Some(conflict) = tab.merge_conflict.as_ref() else {
        unreachable!("the tab is still a merge conflict");
    };
    assert_eq!(conflict.current_editor.scroll_col, merged);
    assert_eq!(conflict.incoming_editor.scroll_col, merged);
    // All three panes show the same columns: the `END` tail of each line.
    assert!(
        painted.iter().any(|row| row.matches("END").count() == 3),
        "the panes are not aligned on the line's tail:\n{}",
        painted.join("\n")
    );
}
