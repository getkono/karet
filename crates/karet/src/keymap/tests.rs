use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

use super::*;

fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, mods)
}

#[test]
fn bindings_are_authored_in_canonical_form() {
    // Every binding chord must equal its canonical form, so `Binding::chord`
    // compares equal to a `KeyChord::from_event` of the intended key press.
    for bind in BINDINGS {
        assert_eq!(
            bind.chord,
            bind.chord.canonical(),
            "non-canonical chord for {:?}",
            bind.command
        );
    }
}

/// Resolve a single key press in a focus context to its command. `is_diff` maps
/// to the diff editor tab; every other editor tab is treated as plain here.
fn res_in(focus: Focus, panel: SidebarPanel, is_diff: bool, key: KeyEvent) -> Option<Command> {
    let tab = if is_diff {
        EditorTab::Diff
    } else {
        EditorTab::Plain
    };
    let ctx = Context::focus(FocusTarget::from(focus, panel, tab));
    match resolve(ctx, &[KeyChord::from_event(key)]) {
        Resolved::Command(c) => Some(c),
        _ => None,
    }
}

/// Resolve with the Explorer panel active (the default for non-SCM tests).
fn res(focus: Focus, is_diff: bool, key: KeyEvent) -> Option<Command> {
    res_in(focus, SidebarPanel::Explorer, is_diff, key)
}

#[test]
fn ctrl_p_variants() {
    assert_eq!(
        res(
            Focus::Sidebar,
            false,
            key(KeyCode::Char('p'), KeyModifiers::CONTROL)
        ),
        Some(Command::OpenQuickOpen)
    );
    assert_eq!(
        res(
            Focus::Sidebar,
            false,
            key(
                KeyCode::Char('P'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            )
        ),
        Some(Command::OpenCommandPalette)
    );
    // F1 is a terminal-safe alternate for the command palette (some emulators
    // capture Ctrl+Shift+P). It works regardless of focus.
    assert_eq!(
        res(Focus::Editor, false, key(KeyCode::F(1), KeyModifiers::NONE)),
        Some(Command::OpenCommandPalette)
    );
}

#[test]
fn focus_routes_navigation() {
    // A bare 'j' is text in the editor (unbound → typed) but moves the
    // selection in the sidebar.
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::Char('j'), KeyModifiers::NONE)
        ),
        None
    );
    assert_eq!(
        res(
            Focus::Sidebar,
            false,
            key(KeyCode::Char('j'), KeyModifiers::NONE)
        ),
        Some(Command::SidebarDown)
    );
}

#[test]
fn diff_only_bindings() {
    // Backslash toggles layout only on a diff tab.
    assert_eq!(
        res(
            Focus::Editor,
            true,
            key(KeyCode::Char('\\'), KeyModifiers::NONE)
        ),
        Some(Command::ToggleDiffLayout)
    );
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::Char('\\'), KeyModifiers::NONE)
        ),
        None
    );
    // Enter on a focused diff drops into the underlying file ("editor mode");
    // on a plain editor tab it stays the newline key.
    assert_eq!(
        res(Focus::Editor, true, key(KeyCode::Enter, KeyModifiers::NONE)),
        Some(Command::OpenDiffFile)
    );
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::Enter, KeyModifiers::NONE)
        ),
        Some(Command::InsertNewline)
    );
    // And the sidebar still reaches the diff via the global focus toggle.
    assert_eq!(
        res(Focus::Sidebar, true, key(KeyCode::Tab, KeyModifiers::NONE)),
        Some(Command::ToggleFocus)
    );
}

#[test]
fn pager_view_scrolls_on_arrows_and_edges() {
    let res_pager = |key: KeyEvent| {
        let ctx = Context::focus(FocusTarget::Pager);
        match resolve(ctx, &[KeyChord::from_event(key)]) {
            Resolved::Command(c) => Some(c),
            _ => None,
        }
    };
    // Arrows scroll (not caret motion), Home/End jump to the edges, `q` closes.
    assert_eq!(
        res_pager(key(KeyCode::Down, KeyModifiers::NONE)),
        Some(Command::ScrollDown)
    );
    assert_eq!(
        res_pager(key(KeyCode::Up, KeyModifiers::NONE)),
        Some(Command::ScrollUp)
    );
    assert_eq!(
        res_pager(key(KeyCode::Home, KeyModifiers::NONE)),
        Some(Command::Top)
    );
    assert_eq!(
        res_pager(key(KeyCode::End, KeyModifiers::NONE)),
        Some(Command::Bottom)
    );
    assert_eq!(
        res_pager(key(KeyCode::Char('q'), KeyModifiers::NONE)),
        Some(Command::CloseTab)
    );
}

#[test]
fn home_end_move_to_line_edges_ctrl_to_document() {
    assert_eq!(
        res(Focus::Editor, false, key(KeyCode::Home, KeyModifiers::NONE)),
        Some(Command::CaretLineStart)
    );
    assert_eq!(
        res(Focus::Editor, false, key(KeyCode::End, KeyModifiers::NONE)),
        Some(Command::CaretLineEnd)
    );
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::Home, KeyModifiers::CONTROL)
        ),
        Some(Command::CaretDocStart)
    );
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::End, KeyModifiers::CONTROL)
        ),
        Some(Command::CaretDocEnd)
    );
}

#[test]
fn word_motion_and_select_all_bind_in_editor() {
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::Left, KeyModifiers::CONTROL)
        ),
        Some(Command::CaretWordLeft)
    );
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::Right, KeyModifiers::CONTROL | KeyModifiers::SHIFT)
        ),
        Some(Command::SelectWordRight)
    );
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::Char('a'), KeyModifiers::CONTROL)
        ),
        Some(Command::EditorSelectAll)
    );
}

#[test]
fn multi_cursor_chords_bind_in_editor() {
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::Up, KeyModifiers::CONTROL | KeyModifiers::ALT)
        ),
        Some(Command::AddCursorAbove)
    );
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::Down, KeyModifiers::CONTROL | KeyModifiers::ALT)
        ),
        Some(Command::AddCursorBelow)
    );
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::Char('d'), KeyModifiers::CONTROL)
        ),
        Some(Command::AddCursorNextOccurrence)
    );
    assert_eq!(
        res(Focus::Editor, false, key(KeyCode::Esc, KeyModifiers::NONE)),
        Some(Command::CollapseCarets)
    );
}

#[test]
fn panel_selection_and_quit() {
    assert_eq!(
        res(
            Focus::Sidebar,
            false,
            key(KeyCode::Char('2'), KeyModifiers::CONTROL)
        ),
        Some(Command::SelectPanel(SidebarPanel::Search))
    );
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::Char('q'), KeyModifiers::CONTROL)
        ),
        Some(Command::Quit)
    );
}

#[test]
fn focus_target_derivation() {
    assert_eq!(
        FocusTarget::from(
            Focus::Sidebar,
            SidebarPanel::SourceControl,
            EditorTab::Plain
        ),
        FocusTarget::SourceControl
    );
    assert_eq!(
        FocusTarget::from(Focus::Sidebar, SidebarPanel::Explorer, EditorTab::Plain),
        FocusTarget::Explorer
    );
    // Opening a diff moves focus to the editor: the active layer becomes
    // DiffEditor, NOT SourceControl, even while the SCM panel is still the
    // underlying sidebar panel. This is the fact behind the "SCM keys do
    // nothing after previewing a diff" bug.
    assert_eq!(
        FocusTarget::from(Focus::Editor, SidebarPanel::SourceControl, EditorTab::Diff),
        FocusTarget::DiffEditor
    );
    assert_eq!(
        FocusTarget::from(Focus::Editor, SidebarPanel::Explorer, EditorTab::Plain),
        FocusTarget::Editor
    );
    // A too-large placeholder in the editor resolves to its override target.
    assert_eq!(
        FocusTarget::from(Focus::Editor, SidebarPanel::Explorer, EditorTab::Oversize),
        FocusTarget::Oversize
    );
}

#[test]
fn oversize_placeholder_binds_open_anyway() {
    // Enter over a too-large placeholder loads it anyway; Esc is unbound so
    // repeated Esc does not leave the view. Editor editing keys must not leak in
    // (the layer is not stacked).
    let ctx = Context::focus(FocusTarget::Oversize);
    assert_eq!(
        resolve(
            ctx,
            &[KeyChord::from_event(key(
                KeyCode::Enter,
                KeyModifiers::NONE
            ))]
        ),
        Resolved::Command(Command::OpenAnyway)
    );
    assert_eq!(
        resolve(
            ctx,
            &[KeyChord::from_event(key(KeyCode::Esc, KeyModifiers::NONE))]
        ),
        Resolved::None
    );
    // A Ctrl-chord still resolves globally (close tab), but Save (Editor layer)
    // does not reach a placeholder.
    assert_eq!(
        resolve(
            ctx,
            &[KeyChord::from_event(key(
                KeyCode::Char('w'),
                KeyModifiers::CONTROL
            ))]
        ),
        Resolved::Command(Command::CloseTab)
    );
    assert_eq!(
        resolve(
            ctx,
            &[KeyChord::from_event(key(
                KeyCode::Char('s'),
                KeyModifiers::CONTROL
            ))]
        ),
        Resolved::None
    );
}

#[test]
fn source_control_bindings_are_panel_scoped() {
    // In the SCM panel, bare 's' stages and Space toggles staging.
    assert_eq!(
        res_in(
            Focus::Sidebar,
            SidebarPanel::SourceControl,
            false,
            key(KeyCode::Char('s'), KeyModifiers::NONE)
        ),
        Some(Command::ScmStage)
    );
    assert_eq!(
        res_in(
            Focus::Sidebar,
            SidebarPanel::SourceControl,
            false,
            key(KeyCode::Char(' '), KeyModifiers::NONE)
        ),
        Some(Command::ScmToggleStage)
    );
    // Shift+Down extends the selection — a shared Sidebar-layer verb that is
    // active in the SCM panel too (it is not shadowed by an SCM binding).
    assert_eq!(
        res_in(
            Focus::Sidebar,
            SidebarPanel::SourceControl,
            false,
            key(KeyCode::Down, KeyModifiers::SHIFT)
        ),
        Some(Command::SelectExtendDown)
    );
    // `x` toggles the cursor row into the selection in both list panels.
    assert_eq!(
        res_in(
            Focus::Sidebar,
            SidebarPanel::SourceControl,
            false,
            key(KeyCode::Char('x'), KeyModifiers::NONE)
        ),
        Some(Command::SelectToggle)
    );
    assert_eq!(
        res_in(
            Focus::Sidebar,
            SidebarPanel::Explorer,
            false,
            key(KeyCode::Down, KeyModifiers::SHIFT)
        ),
        Some(Command::SelectExtendDown)
    );
    // The same 's' in the Explorer panel is not an SCM command.
    assert_eq!(
        res_in(
            Focus::Sidebar,
            SidebarPanel::Explorer,
            false,
            key(KeyCode::Char('s'), KeyModifiers::NONE)
        ),
        None
    );
    // Space in the Explorer toggles expansion, not staging.
    assert_eq!(
        res_in(
            Focus::Sidebar,
            SidebarPanel::Explorer,
            false,
            key(KeyCode::Char(' '), KeyModifiers::NONE)
        ),
        Some(Command::SidebarToggleExpand)
    );
    assert_eq!(
        res_in(
            Focus::Sidebar,
            SidebarPanel::Explorer,
            false,
            key(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ),
        Some(Command::ExplorerCopy)
    );
    assert_eq!(
        res_in(
            Focus::Sidebar,
            SidebarPanel::Explorer,
            false,
            key(KeyCode::Char('x'), KeyModifiers::CONTROL)
        ),
        Some(Command::ExplorerCut)
    );
    assert_eq!(
        res_in(
            Focus::Sidebar,
            SidebarPanel::Explorer,
            false,
            key(KeyCode::Char('v'), KeyModifiers::CONTROL)
        ),
        Some(Command::ExplorerPaste)
    );
    assert_eq!(
        res_in(
            Focus::Sidebar,
            SidebarPanel::Explorer,
            false,
            key(KeyCode::Char('d'), KeyModifiers::CONTROL)
        ),
        Some(Command::ExplorerDuplicate)
    );
    assert_eq!(
        res_in(
            Focus::Sidebar,
            SidebarPanel::Explorer,
            false,
            key(KeyCode::Delete, KeyModifiers::NONE)
        ),
        Some(Command::ExplorerDelete)
    );
    assert_eq!(
        res_in(
            Focus::Sidebar,
            SidebarPanel::Explorer,
            false,
            key(KeyCode::F(10), KeyModifiers::SHIFT)
        ),
        Some(Command::ExplorerOpenContextMenu)
    );
}

#[test]
fn explorer_copy_path_bindings_are_scoped_and_advertised() {
    let explorer = Context::focus(FocusTarget::Explorer);
    let source_control = Context::focus(FocusTarget::SourceControl);
    let absolute = KeyChord::from_event(key(
        KeyCode::Char('C'),
        KeyModifiers::SHIFT | KeyModifiers::ALT,
    ));
    let ctrl_k = KeyChord::from_event(key(KeyCode::Char('k'), KeyModifiers::CONTROL));
    let ctrl_shift_c = KeyChord::from_event(key(
        KeyCode::Char('C'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ));

    assert_eq!(
        resolve(explorer, &[absolute]),
        Resolved::Command(Command::ExplorerCopyPath)
    );
    assert_eq!(resolve(explorer, &[ctrl_k]), Resolved::Pending);
    assert_eq!(
        resolve(explorer, &[ctrl_k, ctrl_shift_c]),
        Resolved::Command(Command::ExplorerCopyRelativePath)
    );
    assert_eq!(resolve(source_control, &[absolute]), Resolved::None);
    assert_eq!(
        resolve(source_control, &[ctrl_k, ctrl_shift_c]),
        Resolved::None
    );

    assert_eq!(
        hint_for(Command::ExplorerCopyPath, ChordStyle::Verbose).as_deref(),
        Some("Alt+Shift+C")
    );
    assert_eq!(
        hint_for(Command::ExplorerCopyRelativePath, ChordStyle::Verbose).as_deref(),
        Some("Ctrl+K Ctrl+Shift+C")
    );
    let hints = hints_for(explorer, ChordStyle::Caret);
    let commands: Vec<Command> = hints.iter().map(|hint| hint.command).collect();
    assert_eq!(
        commands.get(..2),
        Some([Command::ExplorerCopyPath, Command::ExplorerCopyRelativePath].as_slice())
    );
}

#[test]
fn search_modal_still_resolves_global_chords() {
    // The Search modals layer their own keys over Global, so Ctrl+B still toggles
    // the sidebar while a bare 'j' navigates the results rather than typing.
    let list = Context::modal(Modal::SearchList, FocusTarget::Search);
    assert_eq!(
        resolve(
            list,
            &[KeyChord::from_event(key(
                KeyCode::Char('b'),
                KeyModifiers::CONTROL
            ))]
        ),
        Resolved::Command(Command::ToggleSidebar)
    );
    // A plain overlay is exclusive: Ctrl+B does not leak through to Global.
    let overlay = Context::modal(Modal::Overlay, FocusTarget::Editor);
    assert_eq!(
        resolve(
            overlay,
            &[KeyChord::from_event(key(
                KeyCode::Char('b'),
                KeyModifiers::CONTROL
            ))]
        ),
        Resolved::None
    );
}

#[test]
fn editor_editing_chords() {
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ),
        Some(Command::Copy)
    );
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::Char('s'), KeyModifiers::CONTROL)
        ),
        Some(Command::Save)
    );
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::Char('z'), KeyModifiers::CONTROL)
        ),
        Some(Command::Undo)
    );
    // A bare letter is no longer a command in the editor (it is typed instead).
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::Char('y'), KeyModifiers::NONE)
        ),
        None
    );
    // Quit is Ctrl+Q.
    assert_eq!(
        res(
            Focus::Editor,
            false,
            key(KeyCode::Char('q'), KeyModifiers::CONTROL)
        ),
        Some(Command::Quit)
    );
}

#[test]
fn hint_for_formats_chords() {
    assert_eq!(
        hint_for(Command::CloseTab, ChordStyle::Verbose).as_deref(),
        Some("Ctrl+W")
    );
    assert_eq!(
        hint_for(Command::OpenCommandPalette, ChordStyle::Verbose).as_deref(),
        Some("Ctrl+Shift+P")
    );
    assert_eq!(
        hint_for(Command::ToggleFocus, ChordStyle::Verbose).as_deref(),
        Some("Tab")
    );
    // The same binding renders compactly for the status bar.
    assert_eq!(
        hint_for(Command::CloseTab, ChordStyle::Caret).as_deref(),
        Some("^W")
    );
}

#[test]
fn hint_renders_multi_key_sequence() {
    assert_eq!(
        hint_for(Command::CloseAllTabs, ChordStyle::Verbose).as_deref(),
        Some("Ctrl+K Ctrl+W")
    );
}

#[test]
fn hints_for_is_context_aware_and_deduped() {
    let editor = hints_for(Context::focus(FocusTarget::Editor), ChordStyle::Caret);
    let cmds: Vec<Command> = editor.iter().map(|h| h.command).collect();
    // Editor-context commands are advertised…
    assert!(cmds.contains(&Command::Save));
    assert!(cmds.contains(&Command::Undo));
    assert!(cmds.contains(&Command::Cut));
    assert!(cmds.contains(&Command::Copy));
    assert!(cmds.contains(&Command::OpenFind));
    // …while self-evident motion and text-editing keys are omitted.
    assert!(!cmds.contains(&Command::CaretDown));
    assert!(!cmds.contains(&Command::PageDown));
    assert!(!cmds.contains(&Command::DeleteBackward));
    // Every command appears at most once (dedup across layers).
    for &cmd in &cmds {
        assert_eq!(cmds.iter().filter(|c| **c == cmd).count(), 1);
    }
    // The most-specific layer wins ordering: an Editor-layer verb (Save) precedes
    // a Global one (Quit).
    let save = cmds.iter().position(|c| *c == Command::Save);
    let quit = cmds.iter().position(|c| *c == Command::Quit);
    assert!(save < quit);
}

#[test]
fn hints_for_reflects_the_active_modal() {
    let find = hints_for(
        Context::modal(Modal::Find, FocusTarget::Editor),
        ChordStyle::Caret,
    );
    let cmds: Vec<Command> = find.iter().map(|h| h.command).collect();
    assert!(cmds.contains(&Command::FindNext));
    assert!(cmds.contains(&Command::FindPrev));
    assert!(cmds.contains(&Command::FindCancel));
    // The Find modal is exclusive, so editor commands don't leak into the bar.
    assert!(!cmds.contains(&Command::Save));
}

#[test]
fn completions_for_renders_only_the_remaining_chord() {
    let ctrl_k = KeyChord::from_event(key(KeyCode::Char('k'), KeyModifiers::CONTROL));
    let comps = completions_for(
        Context::focus(FocusTarget::Editor),
        &[ctrl_k],
        ChordStyle::Caret,
    );
    let by_cmd = |cmd| comps.iter().find(|h| h.command == cmd);
    // Only the chord AFTER the pending `^K` prefix is rendered.
    assert_eq!(
        by_cmd(Command::CloseAllTabs).map(|h| h.chord.as_str()),
        Some("^W")
    );
    assert_eq!(
        by_cmd(Command::CloseOtherTabs).map(|h| h.chord.as_str()),
        Some("W")
    );
    // Each completion carries a verb so the pending-chord bar is self-explanatory.
    assert!(by_cmd(Command::CloseAllTabs).is_some_and(|h| !h.verb.is_empty()));
}

#[test]
fn chord_sequences_resolve_through_a_pending_prefix() {
    let ctrl_k = KeyChord::from_event(key(KeyCode::Char('k'), KeyModifiers::CONTROL));
    let ctrl_w = KeyChord::from_event(key(KeyCode::Char('w'), KeyModifiers::CONTROL));
    let w = KeyChord::from_event(key(KeyCode::Char('w'), KeyModifiers::NONE));
    let esc = KeyChord::from_event(key(KeyCode::Esc, KeyModifiers::NONE));
    let ed = Context::focus(FocusTarget::Editor);
    // Ctrl+K alone is a prefix of a longer binding — the resolver waits.
    assert_eq!(resolve(ed, &[ctrl_k]), Resolved::Pending);
    // Completing the sequence fires the command; the second chord disambiguates.
    assert_eq!(
        resolve(ed, &[ctrl_k, ctrl_w]),
        Resolved::Command(Command::CloseAllTabs)
    );
    assert_eq!(
        resolve(ed, &[ctrl_k, w]),
        Resolved::Command(Command::CloseOtherTabs)
    );
    // A key that continues nothing breaks the sequence.
    assert_eq!(resolve(ed, &[ctrl_k, esc]), Resolved::None);
}

#[test]
fn no_terminal_binding_is_a_prefix_of_another() {
    // The resolver commits on the first full match, so a binding must never also
    // be a strict prefix of a longer one (which it would shadow). This invariant
    // is what lets resolution stay deterministic without a timeout.
    for a in BINDINGS {
        for b in BINDINGS {
            if a.seq_len() >= b.seq_len() {
                continue;
            }
            let a_prefixes_b = (0..a.seq_len()).all(|i| a.chord_at(i) == b.chord_at(i));
            assert!(
                !a_prefixes_b,
                "{:?} is a prefix of {:?}",
                a.command, b.command
            );
        }
    }
}

#[test]
fn ctrl_k_then_v_opens_the_markdown_preview() {
    let ctx = Context::focus(FocusTarget::Editor);
    let ctrl_k = KeyChord::from_event(key(KeyCode::Char('k'), KeyModifiers::CONTROL));
    // The prefix alone is incomplete, not unbound.
    assert_eq!(resolve(ctx, &[ctrl_k]), Resolved::Pending);

    let v = KeyChord::from_event(key(KeyCode::Char('v'), KeyModifiers::NONE));
    assert_eq!(
        resolve(ctx, &[ctrl_k, v]),
        Resolved::Command(Command::MarkdownPreviewSide)
    );
    // `Ctrl+V` remains Paste: the preview binding must not shadow it.
    let ctrl_v = KeyChord::from_event(key(KeyCode::Char('v'), KeyModifiers::CONTROL));
    assert_eq!(resolve(ctx, &[ctrl_v]), Resolved::Command(Command::Paste));
}
