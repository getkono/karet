//! Which keybinding context a focus, a top-level view, and a modal select, and the
//! chords that switch between views. Split out of `tests.rs` to keep that file under
//! the workspace code-line ceiling — a relocation plus the view cases, no change to
//! the tests that moved.

use super::*;

#[test]
fn focus_target_derivation() {
    assert_eq!(
        FocusTarget::from(
            Focus::Sidebar,
            SidebarPanel::SourceControl,
            EditorTab::Plain,
            View::Editor
        ),
        FocusTarget::SourceControl
    );
    assert_eq!(
        FocusTarget::from(
            Focus::Sidebar,
            SidebarPanel::Explorer,
            EditorTab::Plain,
            View::Editor
        ),
        FocusTarget::Explorer
    );
    // Opening a diff moves focus to the editor: the active layer becomes
    // DiffEditor, NOT SourceControl, even while the SCM panel is still the
    // underlying sidebar panel. This is the fact behind the "SCM keys do
    // nothing after previewing a diff" bug.
    assert_eq!(
        FocusTarget::from(
            Focus::Editor,
            SidebarPanel::SourceControl,
            EditorTab::Diff,
            View::Editor
        ),
        FocusTarget::DiffEditor
    );
    assert_eq!(
        FocusTarget::from(
            Focus::Editor,
            SidebarPanel::Explorer,
            EditorTab::Plain,
            View::Editor
        ),
        FocusTarget::Editor
    );
    // A too-large placeholder in the editor resolves to its override target.
    assert_eq!(
        FocusTarget::from(
            Focus::Editor,
            SidebarPanel::Explorer,
            EditorTab::Oversize,
            View::Editor
        ),
        FocusTarget::Oversize
    );
}

#[test]
fn a_non_editor_view_owns_the_content_area() {
    // The view outranks the active tab: whatever tab is open behind it, keys aimed
    // at the content area belong to the showing view.
    for (view, target) in [
        (View::GitHub, FocusTarget::Github),
        (View::Agents, FocusTarget::Agents),
    ] {
        assert_eq!(
            FocusTarget::from(Focus::Editor, SidebarPanel::Explorer, EditorTab::Diff, view),
            target
        );
    }
    // …but the sidebar and the outline are not the content area, so they resolve
    // the same way in every view.
    assert_eq!(
        FocusTarget::from(
            Focus::Sidebar,
            SidebarPanel::SourceControl,
            EditorTab::Plain,
            View::Agents
        ),
        FocusTarget::SourceControl
    );
    assert_eq!(
        FocusTarget::from(
            Focus::Outline,
            SidebarPanel::Explorer,
            EditorTab::Plain,
            View::GitHub
        ),
        FocusTarget::Outline
    );
    // Self-contained, like the graph and seam browsers: never the editor's keys.
    assert_eq!(
        active_layers(Context::focus(FocusTarget::Agents)),
        &[Layer::Agents, Layer::Global]
    );
}

#[test]
fn the_view_switcher_chords_resolve_from_every_focus() {
    // A `Ctrl+K <digit>` chord rather than `Ctrl+Shift+<digit>`: a terminal reports
    // a shifted digit as its shifted *character*, which varies by keyboard layout.
    let ctrl_k = KeyChord::from_event(key(KeyCode::Char('k'), KeyModifiers::CONTROL));
    for (digit, view) in [
        ('1', View::Editor),
        ('2', View::GitHub),
        ('3', View::Agents),
    ] {
        let chord = KeyChord::from_event(key(KeyCode::Char(digit), KeyModifiers::NONE));
        // Global, so the switcher works from the sidebar as well as the content area.
        for target in [FocusTarget::Editor, FocusTarget::Explorer] {
            assert_eq!(
                resolve(Context::focus(target), &[ctrl_k, chord]),
                Resolved::Command(Command::SelectView(view)),
                "{view:?} from {target:?}"
            );
        }
    }
}
