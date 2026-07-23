use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use karet_core::LineCol;
use karet_core::Range;

use super::model::*;
use super::*;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TempDir {
    path: PathBuf,
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn temp_dir() -> TempDir {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("karet-widgets-{}-{}", std::process::id(), n));
    let _ = std::fs::create_dir_all(&path);
    TempDir { path }
}

fn write(dir: &Path, rel: &str, contents: &[u8]) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, contents);
}

fn names(state: &FileTreeState) -> Vec<String> {
    state.rows().iter().map(|r| name_key(&r.path)).collect()
}

fn labels(state: &FileTreeState) -> Vec<String> {
    state.rows().iter().map(|r| r.label.clone()).collect()
}

#[test]
fn rebuild_lists_top_level_dirs_first() {
    let dir = temp_dir();
    write(&dir.path, "a.txt", b"a");
    write(&dir.path, "sub/b.txt", b"b");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    // "sub" (dir, single *file* child → not compacted) before "a.txt" (file).
    assert_eq!(names(&state), vec!["sub", "a.txt"]);
}

#[cfg(unix)]
#[test]
fn symlink_rows_keep_the_link_identity() -> std::io::Result<()> {
    use std::os::unix::fs::symlink;

    let dir = temp_dir();
    write(&dir.path, "target.txt", b"target");
    symlink("target.txt", dir.path.join("alias.txt"))?;
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);

    let alias = state
        .rows()
        .iter()
        .find(|row| row.path == dir.path.join("alias.txt"));
    assert!(alias.is_some_and(|row| row.is_symlink && !row.is_dir));
    assert!(
        state
            .rows()
            .iter()
            .any(|row| { row.path == dir.path.join("target.txt") && !row.is_symlink })
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlink_rows_render_the_link_glyph() -> std::io::Result<()> {
    use std::os::unix::fs::symlink;

    let dir = temp_dir();
    write(&dir.path, "target.txt", b"target");
    symlink("target.txt", dir.path.join("alias.txt"))?;
    let mut state = FileTreeState::new();
    let area = Rect::new(0, 0, 30, 3);
    let mut buffer = Buffer::empty(area);
    FileTree::new(&dir.path)
        .icons(IconStyle::Ascii)
        .render(area, &mut buffer, &mut state);
    let rendered = (0..area.height)
        .flat_map(|y| (0..area.width).map(move |x| (x, y)))
        .map(|point| buffer[point].symbol())
        .collect::<String>();
    assert!(rendered.contains(" @ alias.txt"));
    Ok(())
}

#[test]
fn toggle_reveals_children() {
    let dir = temp_dir();
    write(&dir.path, "sub/b.txt", b"b");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    state.toggle(&dir.path.join("sub"));
    state.ensure_built(&dir.path);
    assert_eq!(names(&state), vec!["sub", "b.txt"]);
}

#[test]
fn compacts_single_child_directory_chains() {
    let dir = temp_dir();
    // a → b → c, with the leaf file under c: the chain a/b/c collapses to one row.
    write(&dir.path, "a/b/c/leaf.txt", b"x");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    assert_eq!(labels(&state), vec!["a/b/c"]);
    // The row's path is the *deepest* directory.
    assert_eq!(state.rows()[0].path, dir.path.join("a/b/c"));
    // Toggling the chain expands the tip and reveals its child.
    state.toggle_selected();
    state.ensure_built(&dir.path);
    assert_eq!(labels(&state), vec!["a/b/c", "leaf.txt"]);
}

#[test]
fn does_not_compact_when_directory_has_a_file_sibling() {
    let dir = temp_dir();
    write(&dir.path, "a/b/c.txt", b"x");
    write(&dir.path, "a/note.txt", b"y");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    // "a" has two entries (dir b + file note.txt) → not compacted.
    assert_eq!(labels(&state), vec!["a"]);
}

#[test]
fn gitignored_files_are_dimmed_not_hidden() {
    let dir = temp_dir();
    write(&dir.path, ".gitignore", b"ignored.txt\n");
    write(&dir.path, "kept.txt", b"k");
    write(&dir.path, "ignored.txt", b"i");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    // VS Code behavior: nothing is hidden (dotfiles shown too); the gitignored
    // file is listed but flagged for dimming.
    assert_eq!(names(&state), vec![".gitignore", "ignored.txt", "kept.txt"]);
    let ignored: Vec<String> = state
        .rows()
        .iter()
        .filter(|r| r.ignored)
        .map(|r| name_key(&r.path))
        .collect();
    assert_eq!(ignored, vec!["ignored.txt"]);
}

#[test]
fn gitignore_state_is_inherited_by_descendants() {
    let dir = temp_dir();
    // `target/` is ignored by name; its children match no pattern themselves, so
    // strict inheritance is the only thing that keeps them dimmed once expanded.
    write(&dir.path, ".gitignore", b"target/\n");
    write(&dir.path, "target/debug/app", b"bin");
    write(&dir.path, "target/notes.txt", b"n");
    write(&dir.path, "src/main.rs", b"fn main() {}\n");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    // `target` itself is ignored...
    let target = dir.path.join("target");
    assert!(state.rows().iter().any(|r| r.path == target && r.ignored));
    // ...and after expanding it, every descendant row inherits the ignored flag.
    state.expand(&target);
    state.expand(&target.join("debug"));
    state.ensure_built(&dir.path);
    let under_target: Vec<&FileTreeRow> = state
        .rows()
        .iter()
        .filter(|r| r.path.starts_with(&target))
        .collect();
    assert!(under_target.len() >= 3, "expected target subtree rows");
    assert!(
        under_target.iter().all(|r| r.ignored),
        "descendants of an ignored dir must all be ignored"
    );
    // A sibling outside the ignored subtree is unaffected.
    assert!(
        state
            .rows()
            .iter()
            .any(|r| r.path == dir.path.join("src") && !r.ignored)
    );
}

#[test]
fn new_file_inserts_an_inline_editor_and_commits() {
    let dir = temp_dir();
    write(&dir.path, "existing.txt", b"x");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    // Begin a new file at the root; an editing placeholder row appears at the top.
    state.begin_new(false);
    state.ensure_built(&dir.path);
    assert!(state.is_editing());
    assert!(state.rows().iter().any(|r| r.editing));
    // Type a name; the editor row reflects it.
    for c in "new.rs".chars() {
        state.edit_push(c);
    }
    state.ensure_built(&dir.path);
    assert!(
        state
            .rows()
            .iter()
            .any(|r| r.editing && r.label == "new.rs")
    );
    let editing = state.rows().iter().find(|r| r.editing);
    assert!(
        editing.is_some_and(|r| r.path == dir.path.join("new.rs")),
        "the editing row should use the typed candidate path for icon detection"
    );
    // Commit → a Create for the joined path, and editing ends.
    let pending = state.take_edit();
    assert_eq!(
        pending,
        Some(PendingEdit::Create {
            path: dir.path.join("new.rs"),
            folder: false,
        })
    );
    assert!(!state.is_editing());
}

#[test]
fn failed_create_can_restore_the_inline_editor() {
    let dir = temp_dir();
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    state.begin_new(false);
    for c in "retry.rs".chars() {
        state.edit_push(c);
    }
    let pending = state.take_edit();
    assert!(pending.is_some(), "typed name should produce a create edit");
    let Some(pending) = pending else {
        return;
    };
    assert!(!state.is_editing());

    state.restore_edit(&pending);
    state.ensure_built(&dir.path);

    let editing = state.rows().iter().find(|r| r.editing);
    assert!(state.is_editing());
    assert!(editing.is_some_and(|r| r.label == "retry.rs" && r.path == dir.path.join("retry.rs")));
}

#[test]
fn new_folder_nests_under_the_selected_directory() {
    let dir = temp_dir();
    write(&dir.path, "sub/keep.txt", b"k");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    // Select the "sub" directory, then create a folder inside it.
    state.select_visible(0);
    assert!(state.selected().is_some_and(|r| r.is_dir));
    state.begin_new(true);
    for c in "child".chars() {
        state.edit_push(c);
    }
    state.ensure_built(&dir.path);
    // The editor row is nested one level under "sub".
    let editing = state.rows().iter().find(|r| r.editing);
    assert!(editing.is_some_and(|r| r.is_dir && r.depth == 1));
    assert_eq!(
        state.take_edit(),
        Some(PendingEdit::Create {
            path: dir.path.join("sub").join("child"),
            folder: true,
        })
    );
}

#[test]
fn edit_paste_replaces_the_selected_rename_stem() {
    let dir = temp_dir();
    write(&dir.path, "old.txt", b"o");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    state.select_visible(0);
    state.begin_rename();
    state.edit_paste("pasted");
    state.ensure_built(&dir.path);
    assert!(
        state
            .rows()
            .iter()
            .any(|r| r.editing && r.label == "pasted.txt")
    );
}

#[test]
fn inline_edit_cursor_keys_match_a_text_field() {
    let dir = temp_dir();
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    state.begin_new(false);
    state.edit_paste("abcd");
    state.edit_left();
    state.edit_left();
    state.edit_delete();
    state.edit_push('X');
    state.edit_home();
    state.edit_push('^');
    state.edit_end();
    state.edit_push('$');
    state.ensure_built(&dir.path);
    assert!(
        state
            .rows()
            .iter()
            .any(|r| r.editing && r.label == "^abXd$")
    );

    state.edit_select_all();
    state.edit_paste("final.txt");
    state.ensure_built(&dir.path);
    assert!(
        state
            .rows()
            .iter()
            .any(|r| r.editing && r.label == "final.txt")
    );
}

#[test]
fn edit_paste_is_a_no_op_when_not_editing() {
    let dir = temp_dir();
    write(&dir.path, "old.txt", b"o");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    state.edit_paste("should not appear anywhere");
    assert!(!state.is_editing());
}

#[test]
fn rename_marks_the_row_and_returns_the_new_path() {
    let dir = temp_dir();
    write(&dir.path, "old.txt", b"o");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    state.select_visible(0);
    state.begin_rename();
    // Rename begins with the stem selected, mirroring a GUI file explorer:
    // typing replaces "old" while keeping the ".txt" extension.
    for c in "renamed".chars() {
        state.edit_push(c);
    }
    state.ensure_built(&dir.path);
    assert!(
        state
            .rows()
            .iter()
            .any(|r| r.editing && r.label == "renamed.txt")
    );
    assert_eq!(
        state.take_edit(),
        Some(PendingEdit::Rename {
            from: dir.path.join("old.txt"),
            to: dir.path.join("renamed.txt"),
        })
    );
}

#[test]
fn blank_name_commit_is_a_no_op() {
    let dir = temp_dir();
    write(&dir.path, "a.txt", b"a");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    state.begin_new(false);
    assert_eq!(state.take_edit(), None); // nothing typed → no action
    assert!(!state.is_editing());
}

#[test]
fn collapse_all_closes_every_directory() {
    let dir = temp_dir();
    write(&dir.path, "a/b/c.txt", b"c");
    write(&dir.path, "a/note.txt", b"n"); // keeps "a" from compacting
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    state.expand(&dir.path.join("a"));
    state.expand(&dir.path.join("a/b"));
    state.ensure_built(&dir.path);
    assert!(state.rows().len() > 1);
    state.collapse_all();
    state.ensure_built(&dir.path);
    // Only the top-level "a" remains, collapsed.
    assert_eq!(labels(&state), vec!["a"]);
    assert!(!state.rows()[0].expanded);
}

#[test]
fn git_directory_is_always_excluded() {
    let dir = temp_dir();
    write(&dir.path, ".git/config", b"[core]\n");
    write(&dir.path, "src/main.rs", b"fn main() {}\n");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    assert!(!names(&state).contains(&".git".to_string()));
    assert!(names(&state).contains(&"src".to_string()));
}

#[test]
fn nested_repository_marks_its_directory_and_stops_compaction_there() {
    let dir = temp_dir();
    write(&dir.path, "group/project/.git/config", b"[core]\n");
    write(&dir.path, "group/project/src/main.rs", b"fn main() {}\n");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    let row = state.rows().first();
    assert_eq!(row.map(|row| row.label.as_str()), Some("group/project"));
    assert!(row.is_some_and(|row| row.is_repository));
}

#[test]
fn selection_moves_and_clamps() {
    let dir = temp_dir();
    write(&dir.path, "a.txt", b"a");
    write(&dir.path, "b.txt", b"b");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    assert_eq!(
        state.selected_path(),
        Some(dir.path.join("a.txt").as_path())
    );
    state.select_next();
    assert_eq!(
        state.selected_path(),
        Some(dir.path.join("b.txt").as_path())
    );
    state.select_next(); // clamps at the last row
    assert_eq!(
        state.selected_path(),
        Some(dir.path.join("b.txt").as_path())
    );
    state.select_prev();
    state.select_prev(); // clamps at 0
    assert_eq!(
        state.selected_path(),
        Some(dir.path.join("a.txt").as_path())
    );
}

#[test]
fn multi_select_extends_toggles_and_selects_all() {
    let dir = temp_dir();
    write(&dir.path, "a.txt", b"a");
    write(&dir.path, "b.txt", b"b");
    write(&dir.path, "c.txt", b"c");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);

    // Range: cursor 0, extend down one → rows 0 and 1 selected, 2 not.
    state.select_extend(1);
    assert!(state.is_selected(0));
    assert!(state.is_selected(1));
    assert!(!state.is_selected(2));

    // A plain move collapses the range back to a single row.
    state.select_next();
    assert!(!state.is_selected(0));
    assert!(state.is_selected(2));

    // Toggle keeps the cursor row and adds another; select_all covers everything.
    state.select_prev(); // cursor 1
    state.mark_toggle(); // {1}
    state.select_all();
    assert!((0..3).all(|i| state.is_selected(i)));
}

#[test]
fn selected_paths_follow_the_effective_selection() {
    let dir = temp_dir();
    write(&dir.path, "a.txt", b"a");
    write(&dir.path, "b.txt", b"b");
    write(&dir.path, "c.txt", b"c");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);

    assert_eq!(state.selected_paths(), vec![dir.path.join("a.txt")]);

    state.select_extend(1);
    assert_eq!(
        state.selected_paths(),
        vec![dir.path.join("a.txt"), dir.path.join("b.txt")]
    );

    state.toggle_visible(2);
    assert_eq!(
        state.selected_paths(),
        vec![
            dir.path.join("a.txt"),
            dir.path.join("b.txt"),
            dir.path.join("c.txt"),
        ]
    );
}

#[test]
fn selected_paths_survive_row_rebuilds() {
    let dir = temp_dir();
    write(&dir.path, "a.txt", b"a");
    write(&dir.path, "b.txt", b"b");
    write(&dir.path, "c.txt", b"c");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);

    state.select_index(1);
    state.toggle_visible(2);
    write(&dir.path, "aa.txt", b"aa");
    state.rebuild(&dir.path);

    assert_eq!(
        state.selected_paths(),
        vec![dir.path.join("b.txt"), dir.path.join("c.txt")]
    );
    assert_eq!(
        state.selected_path(),
        Some(dir.path.join("c.txt").as_path())
    );
}

#[test]
fn select_visible_maps_viewport_rows_via_offset() {
    let dir = temp_dir();
    write(&dir.path, "a.txt", b"a");
    write(&dir.path, "b.txt", b"b");
    write(&dir.path, "c.txt", b"c");
    let mut state = FileTreeState::new();
    state.ensure_built(&dir.path);
    assert_eq!(state.offset(), 0);
    state.select_visible(2);
    assert_eq!(state.visible_index(0), Some(0));
    assert_eq!(state.visible_index(2), Some(2));
    assert!(state.is_visible_selected(2));
    assert_eq!(
        state.selected_path(),
        Some(dir.path.join("c.txt").as_path())
    );
    state.select_visible(99); // clamps to the last row
    assert_eq!(
        state.selected_path(),
        state.rows().last().map(|r| r.path.as_path())
    );
}

#[test]
fn active_file_row_is_bold() {
    let dir = temp_dir();
    write(&dir.path, "a.txt", b"a");
    let mut state = FileTreeState::new();
    let theme = Theme::dark();
    let active = dir.path.join("a.txt");
    let area = Rect::new(0, 0, 30, 4);
    let mut buf = Buffer::empty(area);
    FileTree::new(&dir.path)
        .theme(&theme)
        .active(Some(&active))
        .render(area, &mut buf, &mut state);
    // The label starts at column 4 (2 chevron + 2 icon cells) and is bold.
    assert!(buf.content()[4].modifier.contains(Modifier::BOLD));

    // The focused pane's active file also gets the dedicated row background.
    assert_eq!(
        buf.content()[0].bg,
        theme.role(ThemeRole::ActiveEditorRow).to_ratatui()
    );

    // Without an active path, the same row is not bold and has no active bg.
    let mut plain = Buffer::empty(area);
    let mut state2 = FileTreeState::new();
    FileTree::new(&dir.path)
        .theme(&theme)
        .render(area, &mut plain, &mut state2);
    assert!(!plain.content()[4].modifier.contains(Modifier::BOLD));
    assert_ne!(
        plain.content()[0].bg,
        theme.role(ThemeRole::ActiveEditorRow).to_ratatui()
    );
}

#[test]
fn active_file_uses_deepest_visible_directory_ancestor() {
    let dir = temp_dir();
    write(&dir.path, "a/b/foo.rs", b"fn main() {}\n");
    write(&dir.path, "a/note.txt", b"note\n");
    let mut state = FileTreeState::new();
    let theme = Theme::dark();
    let active = dir.path.join("a/b/foo.rs");
    let a = dir.path.join("a");
    let b = dir.path.join("a/b");
    let area = Rect::new(0, 0, 30, 6);
    let width = area.width as usize;
    let active_bg = theme.role(ThemeRole::ActiveEditorRow).to_ratatui();

    state.expand(&a);
    let mut collapsed = Buffer::empty(area);
    FileTree::new(&dir.path)
        .theme(&theme)
        .active(Some(&active))
        .render(area, &mut collapsed, &mut state);

    let a_index = state.rows().iter().position(|row| row.path == a);
    let b_index = state.rows().iter().position(|row| row.path == b);
    assert!(a_index.is_some());
    assert!(b_index.is_some());
    let (Some(a_index), Some(b_index)) = (a_index, b_index) else {
        return;
    };
    assert_ne!(collapsed.content()[a_index * width].bg, active_bg);
    assert_eq!(collapsed.content()[b_index * width].bg, active_bg);
    assert!(
        collapsed.content()[b_index * width..(b_index + 1) * width]
            .iter()
            .any(|cell| cell.modifier.contains(Modifier::BOLD))
    );

    state.expand(&b);
    let mut expanded = Buffer::empty(area);
    FileTree::new(&dir.path)
        .theme(&theme)
        .active(Some(&active))
        .render(area, &mut expanded, &mut state);

    let b_index = state.rows().iter().position(|row| row.path == b);
    let file_index = state.rows().iter().position(|row| row.path == active);
    assert!(b_index.is_some());
    assert!(file_index.is_some());
    let (Some(b_index), Some(file_index)) = (b_index, file_index) else {
        return;
    };
    assert_ne!(expanded.content()[b_index * width].bg, active_bg);
    assert_eq!(expanded.content()[file_index * width].bg, active_bg);
}

#[test]
fn active_file_highlights_collapsed_compact_directory_chain() {
    let dir = temp_dir();
    write(&dir.path, "a/b/c/leaf.txt", b"leaf\n");
    let mut state = FileTreeState::new();
    let theme = Theme::dark();
    let active = dir.path.join("a/b/c/leaf.txt");
    let area = Rect::new(0, 0, 30, 2);
    let mut buf = Buffer::empty(area);

    FileTree::new(&dir.path)
        .theme(&theme)
        .active(Some(&active))
        .render(area, &mut buf, &mut state);

    assert_eq!(state.rows()[0].path, dir.path.join("a/b/c"));
    assert_eq!(
        buf.content()[0].bg,
        theme.role(ThemeRole::ActiveEditorRow).to_ratatui()
    );
}

#[test]
fn visible_file_row_is_accent_not_bold() {
    let dir = temp_dir();
    write(&dir.path, "a.txt", b"a");
    let mut state = FileTreeState::new();
    let theme = Theme::dark();
    let visible = vec![dir.path.join("a.txt")];
    let area = Rect::new(0, 0, 30, 4);
    let mut buf = Buffer::empty(area);
    FileTree::new(&dir.path)
        .theme(&theme)
        .visible(&visible)
        .render(area, &mut buf, &mut state);
    // A file visible in another (non-focused) pane: accent foreground, not bold,
    // and none of the stronger active-file background.
    assert_eq!(
        buf.content()[4].fg,
        theme.role(ThemeRole::LineNumberActive).to_ratatui()
    );
    assert!(!buf.content()[4].modifier.contains(Modifier::BOLD));
    assert_ne!(
        buf.content()[0].bg,
        theme.role(ThemeRole::ActiveEditorRow).to_ratatui()
    );
}

#[test]
fn selection_background_requires_explorer_focus() {
    let dir = temp_dir();
    write(&dir.path, "a.txt", b"a");
    let theme = Theme::dark();
    let area = Rect::new(0, 0, 30, 4);
    let sel = theme.role(ThemeRole::Selection).to_ratatui();

    // Cursor on row 0 but the explorer is not focused → no selection background,
    // so the last click doesn't linger once focus is in the editor.
    let mut unfocused = FileTreeState::new();
    let mut buf = Buffer::empty(area);
    FileTree::new(&dir.path)
        .theme(&theme)
        .render(area, &mut buf, &mut unfocused);
    assert_ne!(buf.content()[0].bg, sel);

    // Explorer focused → the cursor row gets the selection background.
    let mut focused = FileTreeState::new();
    let mut buf2 = Buffer::empty(area);
    FileTree::new(&dir.path)
        .theme(&theme)
        .explorer_focused(true)
        .render(area, &mut buf2, &mut focused);
    assert_eq!(buf2.content()[0].bg, sel);
}

#[test]
fn render_draws_status_glyph() {
    let dir = temp_dir();
    write(&dir.path, "a.txt", b"a");
    let mut state = FileTreeState::new();
    let theme = Theme::dark();
    let status = vec![(
        dir.path.join("a.txt"),
        Decoration {
            range: Range {
                start: LineCol::new(0, 0),
                end: LineCol::new(0, 0),
            },
            kind: DecorationKind::GutterMarker { glyph: 'M' },
            role: Some(ThemeRole::DiffModified),
        },
    )];
    let area = Rect::new(0, 0, 30, 4);
    let mut buf = Buffer::empty(area);
    FileTree::new(&dir.path)
        .theme(&theme)
        .status(&status)
        .render(area, &mut buf, &mut state);
    let rendered: String = buf
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect();
    assert!(rendered.contains("a.txt"));
    assert!(rendered.contains('M'));
}

#[test]
fn repository_badge_is_right_aligned_and_survives_a_long_label() {
    let dir = temp_dir();
    write(&dir.path, "long-project-name/.git/config", b"[core]\n");
    write(&dir.path, "long-project-name/a.txt", b"a\n");
    let badges = vec![(dir.path.join("long-project-name"), "↑2 +3 -1".to_string())];
    let mut state = FileTreeState::new();
    let theme = Theme::dark();
    let area = Rect::new(0, 0, 18, 1);
    let mut buf = Buffer::empty(area);
    FileTree::new(&dir.path)
        .theme(&theme)
        .badges(&badges)
        .render(area, &mut buf, &mut state);
    let rendered: String = buf.content().iter().map(|cell| cell.symbol()).collect();
    assert!(rendered.ends_with("↑2 +3 -1"), "{rendered:?}");
}

#[test]
fn nested_rows_draw_indent_guides() {
    let dir = temp_dir();
    write(&dir.path, "sub/b.txt", b"b");
    let mut state = FileTreeState::new();
    let theme = Theme::dark();
    state.ensure_built(&dir.path);
    state.toggle(&dir.path.join("sub"));
    let area = Rect::new(0, 0, 30, 4);
    let mut buf = Buffer::empty(area);
    FileTree::new(&dir.path)
        .theme(&theme)
        .render(area, &mut buf, &mut state);
    // Row 0 is the expanded `sub` (depth 0, no guide, ▼ chevron); row 1 is the
    // nested `b.txt` (depth 1), whose first cell is the box-drawing indent rule.
    let width = area.width as usize;
    assert_eq!(buf.content()[0].symbol(), "\u{25bc}"); // ▼ expanded directory
    assert_eq!(buf.content()[width].symbol(), "\u{2502}"); // │ indent guide
}

#[test]
fn file_icons_are_tinted_by_category() {
    let dir = temp_dir();
    write(&dir.path, "main.rs", b"fn main() {}");
    let mut state = FileTreeState::new();
    let theme = Theme::dark();
    let area = Rect::new(0, 0, 30, 2);
    let mut buf = Buffer::empty(area);
    FileTree::new(&dir.path)
        .theme(&theme)
        .render(area, &mut buf, &mut state);
    // A code file's icon (column 2, after the blank chevron cells) is tinted with
    // the text-file role, not the neutral foreground.
    assert_eq!(
        buf.content()[2].fg,
        theme.role(ThemeRole::FileIconText).to_ratatui()
    );
}
