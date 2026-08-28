//! What the Search panel actually paints: grouped rows, the highlighted match
//! span, the right-aligned line-number column, and the status line.

use ratatui::Terminal;

use super::*;
use crate::app::SearchPanel;

fn search_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("karet-search-ui-{}-{tag}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// A hit whose match covers `needle` inside `line_text`.
fn hit(path: &Path, line: u32, line_text: &str) -> karet_session::SearchHit {
    let start = line_text.find("needle").unwrap_or(0);
    karet_session::SearchHit {
        path: path.to_path_buf(),
        matches: vec![karet_session::SearchMatch {
            range: Range {
                start: LineCol::new(line, start as u32),
                end: LineCol::new(line, (start + 6) as u32),
            },
            line_text: line_text.to_owned(),
            preview_start: start as u32,
            preview_end: (start + 6) as u32,
        }],
    }
}

/// Render the panel and hand back the terminal for cell-level assertions.
fn render(
    app: &mut App,
    width: u16,
    height: u16,
) -> Result<Terminal<ratatui::backend::TestBackend>, std::convert::Infallible> {
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(width, height))?;
    let theme = app.theme.clone();
    let _ = terminal.draw(|frame| {
        let area = frame.area();
        super::sidebar::search::draw_search_panel(
            frame,
            app,
            &theme,
            area,
            &mut ScrollHits::default(),
        );
    });
    Ok(terminal)
}

fn painted(terminal: &Terminal<ratatui::backend::TestBackend>) -> String {
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect()
}

#[test]
fn a_file_heading_sits_over_its_indented_match_rows() -> Result<(), std::convert::Infallible> {
    let dir = search_dir("rows");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.search.query = "needle".into();
    app.search.hits = vec![hit(&dir.join("src/a.rs"), 98, "let needle = 1;")];
    app.search.rebuild_rows();
    app.search.matches_found = 1;
    app.search.searched = true;

    let terminal = render(&mut app, 46, 8)?;
    let painted = painted(&terminal);

    assert!(painted.contains("a.rs"), "{painted}");
    assert!(
        painted.contains("src"),
        "the parent dir is shown: {painted}"
    );
    assert!(painted.contains("let needle = 1;"), "{painted}");
    // 1-based in the UI though the model is 0-based.
    assert!(painted.contains("99"), "{painted}");
    assert!(painted.contains("1 matches in 1 files"), "{painted}");

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// The point of the whole change: the matched span is what the eye should land
/// on, so it carries the same role the editor paints matches with.
#[test]
fn the_matched_span_is_painted_with_the_search_match_role() -> Result<(), std::convert::Infallible>
{
    let dir = search_dir("highlight");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.search.query = "needle".into();
    app.search.hits = vec![hit(&dir.join("a.rs"), 0, "let needle = 1;")];
    app.search.rebuild_rows();
    app.search.searched = true;
    app.search.matches_found = 1;

    let terminal = render(&mut app, 46, 8)?;
    let buffer = terminal.backend().buffer();
    let want = app.theme.role(ThemeRole::SearchMatch).to_ratatui();

    // Collect every cell painted with the match background, in row order.
    let mut highlighted = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let cell = &buffer[(x, y)];
            if cell.bg == want {
                highlighted.push_str(cell.symbol());
            }
        }
    }
    assert_eq!(
        highlighted, "needle",
        "exactly the match is highlighted, no more and no less"
    );

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Numbers of differing width must still line up, or the column cannot be
/// scanned — that alignment is the reason it is a column at all.
#[test]
fn line_numbers_right_align_into_one_column() -> Result<(), std::convert::Infallible> {
    let dir = search_dir("align");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.search.query = "needle".into();
    let path = dir.join("a.rs");
    let mut file = hit(&path, 8, "let needle = 1;");
    let wide = hit(&path, 1233, "let needle = 2;");
    file.matches.extend(wide.matches);
    app.search.hits = vec![file];
    app.search.rebuild_rows();
    app.search.searched = true;
    app.search.matches_found = 2;

    let width = 46;
    let terminal = render(&mut app, width, 8)?;
    let buffer = terminal.backend().buffer();

    // Find the rows carrying each number and check both end at the same column.
    let row_text = |y: u16| -> String { (0..width).map(|x| buffer[(x, y)].symbol()).collect() };
    let mut ends = Vec::new();
    for y in 0..buffer.area.height {
        let text = row_text(y);
        if text.contains("let needle") {
            ends.push(text.trim_end().len());
        }
    }
    assert_eq!(ends.len(), 2, "both match rows rendered");
    assert_eq!(
        ends[0],
        ends[1],
        "9 and 1234 end at the same column: {:?}",
        (row_text(2), row_text(3))
    );

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn a_collapsed_group_hides_its_matches_but_keeps_its_count() -> Result<(), std::convert::Infallible>
{
    let dir = search_dir("collapsed");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.search.query = "needle".into();
    app.search.hits = vec![hit(&dir.join("a.rs"), 0, "let needle = 1;")];
    app.search.rebuild_rows();
    app.search.toggle_file(&dir.join("a.rs"));
    app.search.searched = true;
    app.search.matches_found = 1;

    let terminal = render(&mut app, 46, 8)?;
    let painted = painted(&terminal);

    assert!(painted.contains("a.rs"), "{painted}");
    assert!(painted.contains("(1)"), "the count survives: {painted}");
    assert!(
        !painted.contains("let needle = 1;"),
        "matches are hidden: {painted}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// A bad regex used to be indistinguishable from "no matches".
#[test]
fn an_invalid_pattern_says_so_instead_of_showing_no_results() -> Result<(), std::convert::Infallible>
{
    let dir = search_dir("error");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.search.query = "(".into();
    app.search.regex = true;
    app.search.error = Some("invalid search pattern".into());
    app.search.searched = true;

    let terminal = render(&mut app, 46, 6)?;
    let painted = painted(&terminal);

    assert!(painted.contains("invalid search pattern"), "{painted}");
    assert!(!painted.contains("no results"), "{painted}");

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn the_glob_fields_appear_only_when_the_filters_are_shown() -> Result<(), std::convert::Infallible>
{
    let dir = search_dir("globs");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);

    let painted_hidden = painted(&render(&mut app, 50, 6)?);
    assert!(
        !painted_hidden.contains("files to include"),
        "{painted_hidden}"
    );

    app.search.filters_visible = true;
    let painted_shown = painted(&render(&mut app, 50, 8)?);
    assert!(
        painted_shown.contains("files to include"),
        "{painted_shown}"
    );
    assert!(
        painted_shown.contains("files to exclude"),
        "{painted_shown}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn a_truncated_search_says_the_limit_was_reached() -> Result<(), std::convert::Infallible> {
    let dir = search_dir("truncated");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.search.query = "needle".into();
    app.search.hits = vec![hit(&dir.join("a.rs"), 0, "let needle = 1;")];
    app.search.rebuild_rows();
    app.search.searched = true;
    app.search.truncated = true;
    app.search.matches_found = 5_000;

    let painted = painted(&render(&mut app, 46, 8)?);
    assert!(painted.contains("limit reached"), "{painted}");

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// Sanity-check the split the panel relies on rather than only its callers.
#[test]
fn globs_ignore_empty_segments() {
    assert!(SearchPanel::globs("").is_empty());
    assert_eq!(SearchPanel::globs("  *.rs  "), vec!["*.rs"]);
}

/// The highlight is the point of the row, so it must survive a narrow pane. A
/// left-to-right budget spends the whole width on leading context and leaves the
/// match nothing — which is worst exactly where the backend windowed a long line
/// to put the match 48 bytes in.
#[test]
fn a_narrow_pane_still_shows_the_matched_span() -> Result<(), std::convert::Infallible> {
    let dir = search_dir("narrow");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.search.query = "needle".into();
    app.search.hits = vec![hit(
        &dir.join("a.rs"),
        0,
        "let very_long_result_name = compute_the_thing(needle);",
    )];
    app.search.rebuild_rows();
    app.search.searched = true;
    app.search.matches_found = 1;

    // The default sidebar is 30 columns.
    let terminal = render(&mut app, 30, 8)?;
    let buffer = terminal.backend().buffer();
    let want = app.theme.role(ThemeRole::SearchMatch).to_ratatui();
    let highlighted: String = (0..buffer.area.height)
        .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
        .filter(|&(x, y)| buffer[(x, y)].bg == want)
        .map(|(x, y)| buffer[(x, y)].symbol().to_owned())
        .collect();

    assert!(
        !highlighted.is_empty(),
        "the match must still be visible at 30 columns"
    );
    assert!(
        "needle".contains(&highlighted),
        "what is highlighted is the match (or a truncation of it), got {highlighted:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// A row wider than the list gets its tail clipped by ratatui — and the tail is
/// the directory's most specific part, the half `fit_start` kept on purpose.
#[test]
fn a_file_heading_never_overruns_the_list_width() -> Result<(), std::convert::Infallible> {
    let dir = search_dir("heading-width");
    let mut app = App::new(dir.clone(), Vec::new(), Vec::new(), false);
    app.search.query = "needle".into();
    // Enough rows to overflow the pane, so the list really does reserve its
    // scrollbar column — that reserved cell is what an over-wide heading collides
    // with, and without overflow the bug is invisible.
    app.search.hits = (0..12)
        .map(|i| {
            hit(
                &dir.join(format!("src/ui/sidebar/search{i}.rs")),
                0,
                "let needle = 1;",
            )
        })
        .collect();
    app.search.rebuild_rows();
    app.search.searched = true;
    app.search.matches_found = 12;

    let width = 30;
    let terminal = render(&mut app, width, 8)?;
    let buffer = terminal.backend().buffer();
    let row_of =
        |y: u16, cols: u16| -> String { (0..cols).map(|x| buffer[(x, y)].symbol()).collect() };
    let y = (0..buffer.area.height)
        .find(|&y| row_of(y, width).contains("search0.rs"))
        .unwrap_or(0);

    // The last column belongs to the scrollbar track. The heading must end before
    // it rather than being clipped into it, so measure only the list's columns.
    let heading = row_of(y, width - 1);
    assert_eq!(
        buffer[(width - 1, y)].symbol().trim(),
        "█",
        "the scrollbar column is painted, so an over-wide heading would collide"
    );
    assert!(
        heading.chars().count() < usize::from(width),
        "heading overruns the list: {heading:?}"
    );
    assert!(
        heading.contains("sidebar"),
        "the directory's specific tail survives: {heading:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
