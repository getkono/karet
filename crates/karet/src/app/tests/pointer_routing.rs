//! Pointer input goes to what is under the pointer (#339): the wheel to the
//! split it is over without moving focus, a background split's scrollbar and
//! links to that split, the popups to themselves, and a right click to the text
//! it lands on before the menu opens. The rows are the Wheel, Click and popup
//! rows of `docs/editing-behavior.md`.

use karet_core::Markup;
use karet_core::MarkupKind;

use super::harness::*;
use crate::app::*;

/// `n` numbered lines with the caret at the start.
fn numbered(n: usize) -> String {
    let body: String = (0..n).map(|i| format!("line {i}\n")).collect();
    format!("|{body}")
}

/// A rig split in two, both panes over the same `n`-line document, with the
/// right-hand pane (index 1) focused.
fn split_rig(n: usize) -> Rig {
    let mut rig = Rig::new(&numbered(n));
    rig.step(Step::Cmd(Command::SplitRight));
    assert_eq!(rig.focused_index(), Some(1), "the new split is focused");
    rig
}

/// The `n`th pane's frame, in layout order.
fn pane_frame(rig: &Rig, n: usize) -> PaneFrame {
    let order = rig.app.layout.layout(rig.app.main_rect);
    let Some((pane, _)) = order.get(n).copied() else {
        panic!("no pane {n}");
    };
    let Some(frame) = rig.app.pane_frames.iter().find(|frame| frame.pane == pane) else {
        panic!("pane {n} was not drawn");
    };
    frame.clone()
}

/// Every cell of the `n`th pane's vertical document scrollbar, top to bottom.
fn track_cells(rig: &Rig, n: usize) -> Vec<(u16, u16)> {
    let content = pane_frame(rig, n).content_rect;
    let mut cells = Vec::new();
    for y in content.top()..content.bottom() {
        for x in content.left()..content.right() {
            if rig
                .app
                .scroll_hits
                .at(x, y)
                .is_some_and(|hit| hit.surface == ScrollSurface::TabRows)
            {
                cells.push((x, y));
            }
        }
    }
    cells
}

fn mouse(rig: &mut Rig, kind: MouseEventKind, (column, row): (u16, u16), modifiers: KeyModifiers) {
    rig.app.handle_mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers,
    });
    rig.render();
}

#[test]
fn the_wheel_scrolls_the_split_under_the_pointer_and_leaves_focus() {
    let mut rig = split_rig(40);
    let history = rig.app.view_history.clone();
    rig.step(Step::WheelDown(At::Pane(0, 2, 0), KeyModifiers::NONE));
    assert_eq!(rig.focused_index(), Some(1), "focus stays where it was");
    assert_eq!(
        rig.scroll_line_in(0),
        3,
        "the split under the pointer scrolls"
    );
    assert_eq!(rig.scroll_line_in(1), 0, "the focused split does not");
    assert_eq!(rig.marked_in(0), numbered(40), "no caret moves");
    assert_eq!(rig.marked_in(1), numbered(40));
    assert_eq!(
        rig.app.view_history, history,
        "lending the focus for a notch is not an activation"
    );

    rig.step(Step::WheelUp(At::Pane(0, 3, 0), KeyModifiers::NONE));
    assert_eq!(rig.scroll_line_in(0), 0);
    // The focused split still takes its own notches.
    rig.step(Step::WheelDown(At::Pane(1, 2, 0), KeyModifiers::NONE));
    assert_eq!((rig.scroll_line_in(0), rig.scroll_line_in(1)), (0, 3));
}

#[test]
fn a_background_splits_scrollbar_takes_the_wheel_and_a_press_focuses_it() {
    let mut rig = split_rig(80);
    // The right-hand split is the background one here: the left split's bar
    // shares its column with the divider, which a press grabs first.
    rig.step(click(At::Pane(0, 0, 0)));
    assert_eq!(rig.focused_index(), Some(0));
    let cells = track_cells(&rig, 1);
    let (Some(&top), Some(&bottom)) = (cells.first(), cells.last()) else {
        panic!("the background split paints its scrollbar");
    };
    mouse(
        &mut rig,
        MouseEventKind::ScrollDown,
        top,
        KeyModifiers::NONE,
    );
    assert_eq!(
        rig.focused_index(),
        Some(0),
        "the wheel does not take focus"
    );
    assert_eq!(rig.scroll_line_in(1), 1, "one line per notch on a bar");
    assert_eq!(rig.scroll_line_in(0), 0);

    // A press below the thumb pages that split, and focuses it as a press on its
    // text would.
    mouse(
        &mut rig,
        MouseEventKind::Down(MouseButton::Left),
        bottom,
        KeyModifiers::NONE,
    );
    mouse(
        &mut rig,
        MouseEventKind::Up(MouseButton::Left),
        bottom,
        KeyModifiers::NONE,
    );
    assert_eq!(rig.focused_index(), Some(1));
    assert!(
        rig.scroll_line_in(1) > 1,
        "the press paged the split it hit"
    );
    assert_eq!(rig.scroll_line_in(0), 0);
}

#[test]
fn a_link_in_a_background_split_resolves_in_that_split() {
    let mut rig = split_rig(4);
    let area = pane_frame(&rig, 0).editor_rect;
    let rect = Rect::new(area.x + 2, area.y, 4, 1);
    rig.app.markdown_link_hits = vec![MarkdownLinkHit {
        rect,
        target: "missing.md".to_string(),
    }];
    rig.app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::CONTROL,
    });
    assert_eq!(rig.focused_index(), Some(0), "the link's split is focused");
}

fn item(label: &str) -> karet_core::CompletionItem {
    karet_core::CompletionItem {
        label: label.to_owned(),
        kind: karet_core::CompletionKind::Function,
        detail: None,
        documentation: None,
        insert_text: label.to_owned(),
        edit: None,
        sort_text: None,
        deprecated: false,
    }
}

/// Open the completion popup anchored at the caret, and draw it.
fn open_completion(rig: &mut Rig, labels: &[String]) {
    let anchor = rig.tab().editor.cursor();
    rig.app.completion = Some(crate::completion::CompletionUi {
        items: labels.iter().map(|label| item(label)).collect(),
        list: karet_widgets::CompletionState::default(),
        doc: DocumentId(9),
        anchor,
        last_filter: String::new(),
        mode: crate::completion::CompletionMode::Filtered,
    });
    rig.render();
}

fn completion_area(rig: &Rig) -> Rect {
    let Some(ui) = rig.app.completion.as_ref() else {
        panic!("the popup is open");
    };
    ui.list.area()
}

#[test]
fn clicking_a_completion_row_accepts_it() {
    let mut rig = Rig::new("|\n\n\n\n");
    open_completion(&mut rig, &["alpha".to_string(), "beta".to_string()]);
    let area = completion_area(&rig);
    assert!(area.height >= 2, "both rows are painted");
    // The second row, by sort order.
    rig.step(click(At::Cell(area.x + 2, area.y + 1)));
    assert_eq!(
        rig.marked(),
        "beta|\n\n\n\n",
        "the clicked row was inserted"
    );
    assert!(rig.app.completion.is_none(), "and the popup closed");
}

#[test]
fn the_wheel_over_the_completion_popup_scrolls_the_list_not_the_text() {
    let mut rig = Rig::new(&numbered(40));
    let labels: Vec<String> = (0..25).map(|i| format!("item_{i:02}")).collect();
    open_completion(&mut rig, &labels);
    let area = completion_area(&rig);
    rig.step(Step::WheelDown(
        At::Cell(area.x + 1, area.y + 1),
        KeyModifiers::NONE,
    ));
    assert_eq!(
        rig.scroll_line(),
        0,
        "the text under the popup did not scroll"
    );
    let Some(ui) = rig.app.completion.as_ref() else {
        panic!("the wheel keeps the popup open");
    };
    assert_eq!(ui.list.row_at(area.x, area.y), Some(3), "the list did");
}

/// Show a hover popup of `rows` lines at the caret, and draw it.
fn open_hover(rig: &mut Rig, rows: usize) -> Rect {
    let value: String = (0..rows).map(|i| format!("hover row {i}\n\n")).collect();
    rig.app.hover_ui = Some(crate::hover::HoverUi {
        markup: Markup {
            kind: MarkupKind::Markdown,
            value,
        },
        doc: DocumentId(9),
        at: rig.tab().editor.cursor(),
        scroll: 0,
        rect: Rect::default(),
    });
    rig.render();
    let Some(ui) = rig.app.hover_ui.as_ref() else {
        panic!("the popup is open");
    };
    ui.rect
}

#[test]
fn the_hover_popup_owns_the_pointer_over_it() {
    let mut rig = Rig::new(&numbered(40));
    rig.run(&[key(KeyCode::Down); 5]);
    let rect = open_hover(&mut rig, 30);
    assert!(rect.height > 0, "the popup is painted");
    let scroll = rig.scroll_line();
    let carets = rig.marked();

    rig.step(Step::WheelDown(
        At::Cell(rect.x + 1, rect.y),
        KeyModifiers::NONE,
    ));
    assert_eq!(
        rig.scroll_line(),
        scroll,
        "the text under it did not scroll"
    );
    let hover_scroll = rig.app.hover_ui.as_ref().map(|ui| ui.scroll);
    assert_eq!(hover_scroll, Some(3), "the popup did");

    rig.step(click(At::Cell(rect.x + 1, rect.y)));
    assert!(rig.app.hover_ui.is_some(), "a click inside keeps it");
    assert_eq!(rig.marked(), carets, "and places no caret under it");
}

#[test]
fn a_click_outside_the_hover_popup_dismisses_it() {
    // Even a click that leaves the caret where it was: the popup is anchored to
    // the caret, so only the press itself can tell the user meant to move on.
    let mut rig = Rig::new("ab|c\n\n\n\n\n\n");
    let rect = open_hover(&mut rig, 1);
    assert!(rect.height > 0, "the popup is painted");
    rig.step(click(text(0, 2)));
    assert!(rig.app.hover_ui.is_none());
    assert_eq!(rig.marked(), "ab|c\n\n\n\n\n\n");

    // A click that moves the caret dismisses it too, and moves the caret. The
    // popup sits on the line below the caret, so the click goes two lines down.
    open_hover(&mut rig, 1);
    rig.step(PAUSE);
    rig.step(click(text(3, 0)));
    assert!(rig.app.hover_ui.is_none());
    assert_eq!(rig.marked(), "abc\n\n\n|\n\n\n");
}

/// Open the diagnostic detail view over `paragraphs` paragraphs, and draw it.
fn open_diagnostic_view(rig: &mut Rig, paragraphs: usize) -> Rect {
    let markdown: String = (0..paragraphs)
        .map(|i| format!("problem {i}\n\n"))
        .collect();
    rig.app.diagnostic_view = Some(crate::hover::DiagnosticView {
        markdown,
        ..Default::default()
    });
    rig.render();
    let Some(view) = rig.app.diagnostic_view.as_ref() else {
        panic!("the view is open");
    };
    view.rect
}

#[test]
fn the_diagnostic_view_is_modal_for_the_pointer() {
    let mut rig = Rig::new(&numbered(40));
    let rect = open_diagnostic_view(&mut rig, 60);

    // The wheel scrolls the view wherever the pointer is, never the text.
    rig.step(Step::WheelDown(text(0, 0), KeyModifiers::NONE));
    assert_eq!(rig.scroll_line(), 0);
    let scroll = rig.app.diagnostic_view.as_ref().map(|view| view.scroll);
    assert_eq!(scroll, Some(3));

    // A click inside keeps it.
    rig.step(click(At::Cell(rect.x + 2, rect.y + 2)));
    assert!(rig.app.diagnostic_view.is_some());

    // A click outside closes it, and does nothing else: the caret stays put.
    rig.step(PAUSE);
    let outside = At::Cell(rect.x.saturating_sub(1), rect.y);
    rig.step(click(outside));
    assert!(rig.app.diagnostic_view.is_none());
    assert_eq!(rig.marked(), numbered(40));
}

#[test]
fn right_click_conformance() {
    check(vec![
        case(
            "a right click outside the selection moves the caret there",
            "a[bc|]d efg",
            vec![Step::RightClick(text(0, 6), KeyModifiers::NONE)],
            "abcd e|fg",
        ),
        case(
            "a right click inside the selection keeps it",
            "a[bc|]d efg",
            vec![Step::RightClick(text(0, 2), KeyModifiers::NONE)],
            "a[bc|]d efg",
        ),
        case(
            "a right click on a secondary selection keeps every selection",
            "[ab|] [#cd]",
            vec![Step::RightClick(text(0, 0), KeyModifiers::NONE)],
            "[ab|] [#cd]",
        ),
        case(
            "a right click past the line end lands at the line end",
            "|ab\ncd",
            vec![Step::RightClick(At::Offset(1, 2, 3, 0), KeyModifiers::NONE)],
            "ab\ncd|",
        ),
    ]);
}

#[test]
fn a_right_click_opens_the_menu_with_cut_copy_and_paste_first() {
    let mut rig = Rig::new("|abc def");
    rig.step(Step::RightClick(text(0, 5), KeyModifiers::NONE));
    assert_eq!(rig.marked(), "abc d|ef");
    let Some(menu) = rig.app.context_menu.as_ref() else {
        panic!("a right click on text opens the menu");
    };
    let commands: Vec<Command> = menu
        .entries
        .iter()
        .take(3)
        .filter_map(ContextMenuEntry::command)
        .collect();
    assert_eq!(commands, vec![Command::Cut, Command::Copy, Command::Paste]);
}

#[test]
fn a_right_click_in_a_background_split_focuses_it_and_moves_only_its_caret() {
    let mut rig = split_rig(4);
    rig.step(Step::RightClick(At::Pane(0, 2, 3), KeyModifiers::NONE));
    assert_eq!(rig.focused_index(), Some(0));
    assert_eq!(rig.marked_in(0), "line 0\nline 1\nlin|e 2\nline 3\n");
    assert_eq!(
        rig.marked_in(1),
        numbered(4),
        "the other split's caret stays"
    );
    assert!(rig.app.context_menu.is_some());
}
