//! The whole-document fold commands — fold / unfold all, fold level N, fold
//! and unfold recursively, fold every comment or import block — run through
//! the conformance harness, through their key bindings where they have one.

use std::collections::BTreeSet;

use karet_core::FoldKind;
use karet_core::FoldRegion;
use karet_core::FoldRegions;

use super::harness::*;
use crate::app::*;

const CTRL: KeyModifiers = KeyModifiers::CONTROL;

/// A rig over `marked` with the given fold `regions`, none collapsed.
fn rig_with(marked: &str, regions: Vec<FoldRegion>) -> Rig {
    let mut rig = Rig::new(marked);
    let active = rig.app.active;
    if let TabKind::Code { folds, .. } = &mut rig.app.tabs[active].kind {
        *folds = FoldRegions::from_sorted_regions(regions);
    }
    rig.render();
    rig
}

fn collapsed(rig: &Rig) -> BTreeSet<u32> {
    match &rig.tab().kind {
        TabKind::Code { folded, .. } => folded.clone(),
        _ => BTreeSet::new(),
    }
}

/// `Ctrl+K` then `Ctrl+<c>`.
fn chord(c: char) -> [Step; 2] {
    [
        Step::Key(KeyCode::Char('k'), CTRL),
        Step::Key(KeyCode::Char(c), CTRL),
    ]
}

/// Lines 0..=9: `a` (0..=8) holds `b` (1..=4, holding `c` 2..=3) and `d` (5..=7).
const NESTED: &str = "a {\n  b {\n    c {\n    }\n  }\n  d {\n    x\n  }\n}\ntail";

fn nested() -> Vec<FoldRegion> {
    vec![
        FoldRegion::new(0, 8),
        FoldRegion::new(1, 4),
        FoldRegion::new(2, 3),
        FoldRegion::new(5, 7),
    ]
}

fn set(lines: &[u32]) -> BTreeSet<u32> {
    lines.iter().copied().collect()
}

#[test]
fn fold_all_collapses_every_region_and_lifts_the_caret_to_the_outermost_header() {
    let mut rig = rig_with(&NESTED.replacen("x", "|x", 1), nested());
    rig.run(&chord('0'));
    assert_eq!(collapsed(&rig), set(&[0, 1, 2, 5]));
    assert_eq!(
        rig.tab().editor.cursor().line,
        0,
        "the caret is never hidden"
    );
    rig.run(&chord('j'));
    assert!(collapsed(&rig).is_empty(), "unfold all expands everything");
}

#[test]
fn fold_all_lifts_a_caret_out_of_a_region_headed_where_another_ends() {
    // `if a {…} else {…}` folds as (0, 2) and (2, 4): the `else` header on
    // line 2 is hidden by the collapsed `if` block, so the caret goes on up.
    let src = "if a {\n    x\n} else {\n    |y\n}";
    let mut rig = rig_with(src, vec![FoldRegion::new(0, 2), FoldRegion::new(2, 4)]);
    rig.run(&chord('0'));
    assert_eq!(
        collapsed(&rig),
        set(&[0, 2]),
        "both stay collapsed after the frame"
    );
    assert_eq!(rig.tab().editor.cursor().line, 0);
}

#[test]
fn fold_level_collapses_one_depth_but_not_the_region_holding_a_caret() {
    let mut rig = rig_with(&NESTED.replacen("x", "|x", 1), nested());
    rig.run(&chord('2'));
    // `d` (5..=7) holds the caret, so only `b` folds at depth 2.
    assert_eq!(collapsed(&rig), set(&[1]));
    assert_eq!(rig.tab().editor.cursor().line, 6, "the caret did not move");
    rig.run(&chord('3'));
    assert_eq!(
        collapsed(&rig),
        set(&[1, 2]),
        "other levels are left as they are"
    );
    rig.run(&chord('1'));
    assert_eq!(collapsed(&rig), set(&[1, 2]), "`a` holds the caret too");
}

#[test]
fn every_fold_level_is_bound() {
    for level in 1..=7u8 {
        let digit = char::from(b'0' + level);
        let mut rig = rig_with("|x\na {\n  b\n}", vec![FoldRegion::new(1, 3)]);
        rig.run(&chord(digit));
        let expected = if level == 1 { set(&[1]) } else { set(&[]) };
        assert_eq!(collapsed(&rig), expected, "level {level}");
    }
}

#[test]
fn fold_and_unfold_recursively_act_on_the_region_at_the_cursor_and_inside_it() {
    let mut rig = rig_with(&NESTED.replacen("b {", "b| {", 1), nested());
    rig.run(&chord('['));
    assert_eq!(collapsed(&rig), set(&[1, 2]), "`b` and `c`, not `a` or `d`");
    rig.run(&chord('0'));
    assert_eq!(collapsed(&rig), set(&[0, 1, 2, 5]));
    // The caret sits on `a`'s header now, so unfolding recursively opens all.
    rig.run(&chord(']'));
    assert!(collapsed(&rig).is_empty());
}

#[test]
fn unfold_recursively_leaves_regions_outside_the_target_collapsed() {
    let mut rig = rig_with(&NESTED.replacen("b {", "b| {", 1), nested());
    rig.run(&[
        Step::Cmd(Command::FoldLevel(2)),
        Step::Cmd(Command::FoldLevel(3)),
    ]);
    // The caret is on `b`'s header, so `b` stays open while `c` and `d` fold.
    assert_eq!(collapsed(&rig), set(&[2, 5]));
    rig.run(&[Step::Cmd(Command::UnfoldRecursively)]);
    assert_eq!(collapsed(&rig), set(&[5]), "`d` is outside `b`");
}

#[test]
fn fold_all_comments_and_imports_fold_only_their_kind() {
    let src = "|use a;\nuse b;\n// one\n// two\nfn f() {\n    x\n}";
    let regions = vec![
        FoldRegion::new(0, 1).with_kind(FoldKind::Imports),
        FoldRegion::new(2, 3).with_kind(FoldKind::Comment),
        FoldRegion::new(4, 6),
    ];
    let mut rig = rig_with(src, regions.clone());
    rig.run(&chord('/'));
    assert_eq!(collapsed(&rig), set(&[2]));
    let mut rig = rig_with(src, regions);
    rig.run(&chord('g'));
    assert_eq!(collapsed(&rig), set(&[0]));
    assert_eq!(
        rig.tab().editor.cursor().line,
        0,
        "the header holds the caret"
    );
}

#[test]
fn fold_commands_are_in_the_palette() {
    let palette = crate::command::palette();
    for command in [
        Command::FoldAll,
        Command::UnfoldAll,
        Command::FoldLevel(1),
        Command::FoldLevel(7),
        Command::FoldRecursively,
        Command::UnfoldRecursively,
        Command::FoldAllComments,
        Command::FoldAllImports,
    ] {
        assert!(palette.contains(&command), "{command:?}");
        assert!(command.id().is_some(), "{command:?}");
    }
    assert_eq!(Command::FoldLevel(3).label(), "Fold: Fold Level 3");
    assert_eq!(Command::from_id("foldLevel.7"), Some(Command::FoldLevel(7)));
    assert_eq!(Command::FoldLevel(8).id(), None);
}
