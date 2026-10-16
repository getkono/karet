//! The in-file find bar's refinements: the current-match style, Search in
//! Selection, preserve-case replace, and regex captures in a replacement.

use super::harness::*;
use super::support::*;
use crate::app::*;

/// Open the find bar over `query` with `replace` as the replacement and the
/// bar owning the keyboard, as Ctrl+F then typing leaves it.
fn open(rig: &mut Rig, query: &str, replace: &str) {
    rig.step(Step::Key(KeyCode::Char('f'), KeyModifiers::CONTROL));
    if let Some(find) = rig.app.active_find_mut() {
        find.query = query.to_string();
        find.replace = replace.to_string();
        find.replace_visible = true;
    }
    rig.app.run_find();
    rig.render();
}

fn alt(c: char) -> Step {
    Step::Key(KeyCode::Char(c), KeyModifiers::ALT)
}

/// The theme role of each match decoration, in document order.
fn roles(rig: &Rig) -> Vec<Option<ThemeRole>> {
    match &rig.tab().kind {
        TabKind::Code { decos, .. } => decos.iter().map(|d| d.role).collect(),
        _ => Vec::new(),
    }
}

/// The active tab's text.
fn text_of(rig: &Rig) -> String {
    match &rig.tab().kind {
        TabKind::Code { text, .. } => text.to_string(),
        _ => String::new(),
    }
}

fn find_state(rig: &Rig) -> Option<&crate::tab::FindState> {
    rig.app.active_find()
}

#[test]
fn exactly_the_current_match_wears_the_current_match_role() {
    use ThemeRole::SearchMatch as M;
    use ThemeRole::SearchMatchCurrent as C;
    let mut rig = Rig::new("|ab ab ab");
    open(&mut rig, "ab", "");
    assert_eq!(roles(&rig), vec![Some(C), Some(M), Some(M)]);
    rig.step(key(KeyCode::Down));
    assert_eq!(roles(&rig), vec![Some(M), Some(C), Some(M)]);
    // Stepping back past the first wraps to the last.
    rig.step(key(KeyCode::Up));
    rig.step(key(KeyCode::Up));
    assert_eq!(roles(&rig), vec![Some(M), Some(M), Some(C)]);
    assert_eq!(find_state(&rig).map(|f| f.current), Some(2));
}

#[test]
fn the_current_match_is_painted_apart_from_the_others() {
    let mut rig = Rig::new("|ab ab");
    open(&mut rig, "ab", "");
    let (x0, y0) = rig.cell(text(0, 0));
    let (x1, y1) = rig.cell(text(0, 3));
    let buf = frame(&mut rig.app, 60, 12);
    let theme = &rig.app.theme;
    let current = theme.role(ThemeRole::SearchMatchCurrent).to_ratatui();
    let other = theme.role(ThemeRole::SearchMatch).to_ratatui();
    assert_eq!(buf[(x1, y1)].bg, other);
    // The caret sits on the current match, so check its second cell.
    assert_eq!(buf[(x0 + 1, y0)].bg, current);
}

#[test]
fn search_in_selection_counts_only_matches_inside_the_selection() {
    let mut rig = Rig::new("ab [ab ab|] ab");
    rig.step(Step::Key(KeyCode::Char('f'), KeyModifiers::CONTROL));
    rig.step(alt('l'));
    for c in "ab".chars() {
        rig.step(key(KeyCode::Char(c)));
    }
    assert_eq!(find_state(&rig).map(|f| f.count), Some(2));
    // Turning it off searches the whole document again.
    rig.step(alt('l'));
    assert_eq!(find_state(&rig).map(|f| f.count), Some(4));
    assert!(find_state(&rig).is_some_and(|f| f.scope.is_none()));
}

#[test]
fn search_in_selection_without_a_selection_scopes_to_the_caret_line() {
    let mut rig = Rig::new("ab\nab |ab\nab");
    rig.step(Step::Key(KeyCode::Char('f'), KeyModifiers::CONTROL));
    rig.step(alt('l'));
    for c in "ab".chars() {
        rig.step(key(KeyCode::Char(c)));
    }
    assert_eq!(find_state(&rig).map(|f| f.count), Some(2));
    // The last line has no line break to include; its scope ends at the end.
    let mut rig = Rig::new("ab\nab |ab");
    rig.step(Step::Key(KeyCode::Char('f'), KeyModifiers::CONTROL));
    rig.step(alt('l'));
    for c in "ab".chars() {
        rig.step(key(KeyCode::Char(c)));
    }
    assert_eq!(find_state(&rig).map(|f| f.count), Some(2));
}

#[test]
fn replace_all_in_selection_leaves_the_rest_alone_and_the_scope_follows() {
    let mut rig = Rig::new("ab [ab ab|] ab");
    rig.step(Step::Key(KeyCode::Char('f'), KeyModifiers::CONTROL));
    rig.step(alt('l'));
    open(&mut rig, "ab", "wide");
    rig.step(Step::Cmd(Command::FindReplaceAll));
    assert_eq!(text_of(&rig), "ab wide wide ab");
    // The scope grew with its replacements, so it still holds exactly them.
    open(&mut rig, "wide", "");
    assert_eq!(find_state(&rig).map(|f| f.count), Some(2));
    open(&mut rig, "ab", "");
    assert_eq!(find_state(&rig).map(|f| f.count), Some(0));
}

#[test]
fn replace_one_in_selection_keeps_the_scope_on_its_text() {
    let mut rig = Rig::new("ab [ab ab|] ab");
    rig.step(Step::Key(KeyCode::Char('f'), KeyModifiers::CONTROL));
    rig.step(alt('l'));
    open(&mut rig, "ab", "longer");
    rig.app.find_replace_current();
    assert_eq!(text_of(&rig), "ab longer ab ab");
    // One match is left in scope, and the one outside it still is not counted.
    assert_eq!(find_state(&rig).map(|f| f.count), Some(1));
}

#[test]
fn closing_the_bar_drops_the_selection_scope() {
    let mut rig = Rig::new("ab [ab|] ab");
    rig.step(Step::Key(KeyCode::Char('f'), KeyModifiers::CONTROL));
    rig.step(alt('l'));
    assert!(find_state(&rig).is_some_and(|f| f.scope.is_some()));
    rig.step(key(KeyCode::Esc));
    assert!(find_state(&rig).is_some_and(|f| f.scope.is_none()));
}

#[test]
fn preserve_case_follows_each_matchs_case_in_replace_all() {
    let mut rig = Rig::new("|foo Foo FOO");
    open(&mut rig, "foo", "bar");
    rig.step(alt('p'));
    assert!(find_state(&rig).is_some_and(|f| f.preserve_case));
    rig.step(Step::Cmd(Command::FindReplaceAll));
    assert_eq!(text_of(&rig), "bar Bar BAR");
}

#[test]
fn preserve_case_applies_to_a_single_replace() {
    let mut rig = Rig::new("|Foo foo");
    open(&mut rig, "foo", "bar");
    rig.step(alt('p'));
    rig.app.find_replace_current();
    assert_eq!(text_of(&rig), "Bar foo");
}

#[test]
fn without_preserve_case_the_replacement_is_inserted_as_typed() {
    let mut rig = Rig::new("|foo Foo FOO");
    open(&mut rig, "foo", "bar");
    rig.step(Step::Cmd(Command::FindReplaceAll));
    assert_eq!(text_of(&rig), "bar bar bar");
}

#[test]
fn toggling_preserve_case_does_not_move_the_caret() {
    let mut rig = Rig::new("foo foo");
    open(&mut rig, "foo", "bar");
    rig.step(key(KeyCode::Down));
    let before = rig.marked();
    rig.step(alt('p'));
    assert_eq!(rig.marked(), before);
}

#[test]
fn regex_captures_expand_in_replace_all_and_in_a_single_replace() {
    let mut rig = Rig::new("|a=1 b=2");
    open(&mut rig, "", "$2=$1");
    if let Some(find) = rig.app.active_find_mut() {
        find.regex = true;
        find.query = r"(\w)=(\d)".to_string();
    }
    rig.app.run_find();
    rig.app.find_replace_current();
    assert_eq!(text_of(&rig), "1=a b=2");
    rig.step(Step::Cmd(Command::FindReplaceAll));
    assert_eq!(text_of(&rig), "1=a 2=b");
}

#[test]
fn a_literal_query_inserts_a_dollar_replacement_verbatim() {
    let mut rig = Rig::new("|ab");
    open(&mut rig, "ab", "$1");
    rig.app.find_replace_current();
    assert_eq!(text_of(&rig), "$1");
}

#[test]
fn the_bar_shows_the_in_selection_and_preserve_case_toggles() {
    let mut rig = Rig::new("|ab");
    open(&mut rig, "ab", "");
    let buf = frame(&mut rig.app, 80, 12);
    let rows: Vec<String> = (0..12)
        .map(|y| (0..80).map(|x| buf[(x, y)].symbol()).collect())
        .collect();
    assert!(rows.iter().any(|r| r.contains("Sel")), "{rows:#?}");
    assert!(rows.iter().any(|r| r.contains("AB")), "{rows:#?}");
}
