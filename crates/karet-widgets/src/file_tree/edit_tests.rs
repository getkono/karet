//! The inline name editor: motions, the clipboard, caret placement, and where
//! the renderer reports the field. None of it needs a listing — the editor is
//! a row the tree overlays on whatever it has been given — so the root is a
//! path that need not exist.

use super::*;

/// A tree over an empty, never-listed root.
fn empty_tree() -> (PathBuf, FileTreeState) {
    let root = PathBuf::from("/elsewhere/empty");
    let mut state = FileTreeState::new();
    state.ensure_built(&root);
    (root, state)
}

#[test]
fn inline_edit_motions_extend_a_selection_and_the_clipboard_reaches_it() {
    let (root, mut state) = empty_tree();
    state.begin_new(false);
    state.edit_paste("readme.md");

    // Shift with a motion extends back from the caret at the end.
    state.edit_left(true);
    state.edit_left(true);
    state.edit_left(true);
    assert_eq!(state.edit_selected_text(), Some(".md"));

    // Cutting takes exactly that run away.
    assert_eq!(state.edit_cut().as_deref(), Some(".md"));
    assert_eq!(state.edit_selected_text(), None);
    state.ensure_built(&root);
    assert!(
        state
            .rows()
            .iter()
            .any(|r| r.editing && r.label == "readme")
    );

    // Select-all then cut empties the field.
    state.edit_select_all();
    assert_eq!(state.edit_cut().as_deref(), Some("readme"));
}

#[test]
fn a_click_in_the_inline_edit_places_the_caret_at_that_cell() {
    let (_root, mut state) = empty_tree();
    state.begin_new(false);
    state.edit_paste("alphabet");

    // Anchor at cell 5, then extend to cell 8.
    state.edit_place_cursor(5, false);
    state.edit_place_cursor(8, true);
    assert_eq!(state.edit_selected_text(), Some("bet"));
}

#[test]
fn the_renderer_records_where_the_inline_edit_field_landed() {
    let (root, mut state) = empty_tree();
    assert_eq!(state.edit_rect(), None, "nothing is being edited yet");

    state.begin_new(false);
    state.edit_paste("name");
    let area = Rect::new(3, 0, 40, 10);
    let mut buf = Buffer::empty(area);
    FileTree::new(&root).render(area, &mut buf, &mut state);

    let rect = state.edit_rect();
    assert!(rect.is_some(), "the editing row reports its field");
    let Some(rect) = rect else { return };
    assert!(
        rect.x > area.x,
        "the field starts after the row's indent, chevron and icon"
    );
    assert_eq!(rect.height, 1);
    assert!(rect.right() <= area.right());
    // The recorded rect is where the name actually is: the cell at its left edge
    // holds the first character of the buffer.
    assert_eq!(
        buf.cell((rect.x, rect.y)).map(|cell| cell.symbol()),
        Some("n")
    );
}
