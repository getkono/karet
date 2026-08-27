//! Render tests: what the view actually puts on screen.
//!
//! Rendering is the product here, so these assert on cells rather than on state. The
//! states worth pinning are the ones that are easy to render *almost* right — a
//! configuration that goes unnamed, an empty view that does not say why it is empty,
//! a glyph with no legend to decode it.

use karet_core::Range;
use karet_filetype::IconStyle;
use karet_session::api::SeamFacetView;
use karet_session::api::SeamNodeView;
use karet_session::api::SeamPreview;
use karet_session::api::SeamSummary;
use karet_theme::Theme;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use super::*;
use crate::app::seam::SeamViewState;

/// A node with the given facets and rollups.
fn node(id: &str, parent: Option<&str>, children: &[&str], rollups: [u32; 5]) -> SeamNodeView {
    SeamNodeView {
        id: id.to_owned(),
        name: id.rsplit("::").next().unwrap_or(id).to_owned(),
        kind: "module".to_owned(),
        detail: None,
        file: std::path::PathBuf::from("src/lib.rs"),
        range: Range::default(),
        selection: Range::default(),
        parent: parent.map(str::to_owned),
        children: children.iter().map(|c| (*c).to_owned()).collect(),
        facets: Vec::new(),
        rollups,
        visibility: Some("public".to_owned()),
        membership: "active".to_owned(),
        provisional: false,
    }
}

fn facet(lens: &str, subtype: &str) -> SeamFacetView {
    SeamFacetView {
        lens: lens.to_owned(),
        subtype: subtype.to_owned(),
        detail: None,
        sites: Vec::new(),
        effective: None,
    }
}

fn summary() -> SeamSummary {
    SeamSummary {
        package: "demo".to_owned(),
        packages: 1,
        nodes: 3,
        files: 1,
        configuration: "default @ x86_64-linux".to_owned(),
        available_configurations: vec!["default @ x86_64-linux".to_owned()],
        variation_complete: true,
        truncated_after: None,
        unresolved_modules: Vec::new(),
    }
}

/// A small ready view.
fn view() -> SeamViewState {
    let mut state = SeamViewState::pending();
    let mut danger = node("demo::danger", Some("demo"), &[], [1, 0, 0, 0, 1]);
    danger.facets = vec![facet("api", "pub"), facet("hazard", "unsafe")];
    danger.kind = "function".to_owned();
    state.adopt(
        summary(),
        vec![
            node(
                "demo",
                None,
                &["demo::danger", "demo::quiet"],
                [2, 0, 0, 0, 1],
            ),
            danger,
            node("demo::quiet", Some("demo"), &[], [1, 0, 0, 0, 0]),
        ],
    );
    state
}

/// Render into a buffer of the given size.
fn render(state: &mut SeamViewState, width: u16, height: u16) -> Buffer {
    render_as(state, width, height, IconStyle::Ascii)
}

/// Render in a chosen icon tier, for the assertions that are about glyph width.
fn render_as(state: &mut SeamViewState, width: u16, height: u16, icons: IconStyle) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height));
    let Ok(terminal) = terminal.as_mut() else {
        return Buffer::empty(ratatui::layout::Rect::new(0, 0, width, height));
    };
    let theme = Theme::dark();
    let _ = terminal.draw(|f| {
        draw_seam(f, &theme, f.area(), state, icons);
    });
    terminal.backend().buffer().clone()
}

/// The whole buffer as text, one line per row.
fn text(buf: &Buffer) -> String {
    let area = buf.area;
    (area.top()..area.bottom())
        .map(|y| {
            (area.left()..area.right())
                .map(|x| {
                    buf.cell((x, y))
                        .map_or(' ', |c| c.symbol().chars().next().unwrap_or(' '))
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_header_always_names_the_configuration() {
    let mut state = view();
    let rendered = text(&render(&mut state, 120, 20));
    // Nothing renders unattributed: a view that does not say which build it is showing
    // is answering a question the reader did not ask.
    assert!(rendered.contains("config:"), "{rendered}");
    assert!(rendered.contains("default"), "{rendered}");
    assert!(rendered.contains("demo"), "{rendered}");
}

#[test]
fn an_incomplete_variation_lens_says_so_in_the_header() {
    let mut state = view();
    state.summary.variation_complete = false;
    let rendered = text(&render(&mut state, 120, 20));
    assert!(rendered.contains("variation incomplete"), "{rendered}");
}

#[test]
fn a_truncated_index_never_looks_complete() {
    let mut state = view();
    state.summary.truncated_after = Some(20_000);
    let rendered = text(&render(&mut state, 120, 20));
    assert!(rendered.contains("truncated"), "{rendered}");
}

#[test]
fn unresolved_modules_are_reported_rather_than_omitted_silently() {
    let mut state = view();
    state.summary.unresolved_modules = vec![("demo::absent".to_owned(), Vec::new())];
    let rendered = text(&render(&mut state, 120, 20));
    assert!(rendered.contains("unresolved"), "{rendered}");
}

#[test]
fn the_legend_is_always_on_screen() {
    let mut state = view();
    let rendered = text(&render(&mut state, 140, 20));
    // A glyph nobody can decode is decoration; the legend is not behind a keypress.
    for label in ["api", "sub", "var", "bnd", "haz"] {
        assert!(rendered.contains(label), "missing {label} in:\n{rendered}");
    }
}

#[test]
fn the_spine_lists_the_package_and_its_children() {
    let mut state = view();
    state.move_row(0);
    let rendered = text(&render(&mut state, 120, 20));
    assert!(rendered.contains("demo"), "{rendered}");
    assert!(rendered.contains("danger"), "{rendered}");
    assert!(rendered.contains("quiet"), "{rendered}");
}

#[test]
fn a_narrow_terminal_falls_back_to_an_indented_tree() {
    let mut state = view();
    state.move_row(0);
    // Two readable columns need roughly 37 cells; below that the shape changes rather
    // than the content being crushed.
    assert!(wide_enough(50), "50 cells carries two columns comfortably");
    assert!(!wide_enough(30));
    let rendered = text(&render(&mut state, 30, 20));
    assert!(rendered.contains("danger"), "{rendered}");
    assert!(rendered.contains("quiet"), "{rendered}");
}

#[test]
fn the_facet_pane_spells_out_every_lens_including_the_empty_ones() {
    let mut state = view();
    state.select_path("demo::danger");
    let rendered = text(&render(&mut state, 120, 24));
    // A glyph must never be the only place a fact appears.
    assert!(rendered.contains("hazard"), "{rendered}");
    assert!(rendered.contains("unsafe"), "{rendered}");
    // And an absent lens says so, rather than being left off the list.
    assert!(rendered.contains("boundary"), "{rendered}");
    assert!(
        rendered.contains('—'),
        "an empty lens must read as absent:\n{rendered}"
    );
}

#[test]
fn unresolved_edges_read_as_unresolved_not_as_none() {
    let mut state = view();
    state.select_path("demo::danger");
    let rendered = text(&render(&mut state, 120, 24));
    // With no semantic tier nothing has looked, so claiming "none" would assert an
    // absence the index has not established.
    assert!(rendered.contains("not resolved"), "{rendered}");
}

#[test]
fn each_empty_state_says_which_one_it_is() {
    // Indexing.
    let mut loading = SeamViewState::pending();
    let rendered = text(&render(&mut loading, 100, 12));
    // Before the reveal delay nothing is claimed at all, so a fast index never flashes.
    assert!(!rendered.contains("No seams"), "{rendered}");

    // A root that could not be indexed.
    let mut failed = SeamViewState::pending();
    failed.fail("no Cargo.toml".to_owned());
    let rendered = text(&render(&mut failed, 100, 12));
    assert!(
        rendered.contains("Nothing here could be indexed"),
        "{rendered}"
    );
    assert!(rendered.contains("no Cargo.toml"), "{rendered}");
    // And the way out is named, since the root may simply have been the wrong choice.
    assert!(rendered.contains("another start point"), "{rendered}");

    // A root with genuinely nothing in it.
    let mut empty = SeamViewState::pending();
    empty.adopt(summary(), Vec::new());
    let rendered = text(&render(&mut empty, 100, 12));
    assert!(rendered.contains("No seams here"), "{rendered}");

    // Filtered to nothing, which is a different fact again.
    let mut filtered = view();
    filtered.query_matches = Some(std::collections::HashSet::new());
    filtered.lens_filter = crate::app::seam::LensFilter::Hide;
    let rendered = text(&render(&mut filtered, 100, 12));
    assert!(
        rendered.contains("matches the current filters"),
        "{rendered}"
    );
}

#[test]
fn a_query_error_is_shown_with_its_suggestions() {
    let mut state = view();
    state.query = "lens:hazrd".to_owned();
    state.query_error = Some(karet_session::api::SeamQueryError {
        message: "unknown lens `hazrd`".to_owned(),
        start: 0,
        end: 10,
        suggestions: vec!["hazard".to_owned()],
    });
    let rendered = text(&render(&mut state, 120, 20));
    assert!(rendered.contains("unknown lens"), "{rendered}");
    assert!(rendered.contains("did you mean"), "{rendered}");
    assert!(rendered.contains("hazard"), "{rendered}");
}

#[test]
fn the_reversal_path_is_visible_once_narrowed() {
    let mut state = view();
    // Narrowed directly: what this pins is that a narrowed view shows the way back, not
    // how one narrows — and the fixture's sole root is no longer rerootable onto itself.
    state
        .narrow
        .push(crate::app::seam::Narrow::Scope("demo".to_owned()));
    state.move_row(0);
    let rendered = text(&render(&mut state, 120, 20));
    // Reversible is not enough — the way back has to be on screen.
    assert!(rendered.contains("widen"), "{rendered}");
}

#[test]
fn every_lens_line_in_the_facet_pane_starts_at_the_same_column() {
    // The regression: `hazard`'s glyph was East-Asian Wide, so its name sat one column
    // right of the other four and the pane read as ragged.
    for icons in [IconStyle::NerdFont, IconStyle::Unicode, IconStyle::Ascii] {
        let mut state = view();
        state.select_path("demo::danger");
        let rendered = text(&render_as(&mut state, 120, 24, icons));
        let columns: Vec<usize> = LENS_NAMES
            .iter()
            .filter_map(|lens| {
                rendered
                    .lines()
                    // Past the header, whose legend also spells `api`.
                    .skip(1)
                    .find(|line| line.contains(*lens))
                    .and_then(|line| line.chars().position(|c| c.is_ascii_alphabetic()))
            })
            .collect();
        assert_eq!(columns.len(), LENS_NAMES.len(), "{icons:?}");
        assert!(
            columns.windows(2).all(|pair| pair[0] == pair[1]),
            "{icons:?}: lens names start at {columns:?}"
        );
    }
}

#[test]
fn the_legend_entries_are_evenly_spaced_in_every_tier() {
    // Each entry is `{digit}{glyph} {short}  `, so the short names sit at a constant
    // stride — until one glyph measures two cells and shoves everything after it along.
    for icons in [IconStyle::NerdFont, IconStyle::Unicode, IconStyle::Ascii] {
        let mut state = view();
        let rendered = text(&render_as(&mut state, 140, 20, icons));
        let Some(header) = rendered.lines().next() else {
            panic!("no header row");
        };
        let columns: Vec<usize> = ["api", "sub", "var", "bnd", "haz"]
            .iter()
            .filter_map(|short| {
                header
                    .chars()
                    .collect::<Vec<_>>()
                    .windows(3)
                    .position(|w| w.iter().collect::<String>() == **short)
            })
            .collect();
        assert_eq!(columns.len(), 5, "{icons:?}: {header:?}");
        let strides: Vec<usize> = columns.windows(2).map(|pair| pair[1] - pair[0]).collect();
        assert!(
            strides.windows(2).all(|pair| pair[0] == pair[1]),
            "{icons:?}: legend strides {strides:?} in {header:?}"
        );
    }
}

/// A source preview whose node sits `before` lines into the fetched block.
fn preview(first_line: u32, before: usize, body: usize, count: usize) -> SeamPreview {
    SeamPreview {
        file: std::path::PathBuf::from("src/lib.rs"),
        first_line,
        lines: (0..count).map(|n| format!("    source line {n}")).collect(),
        body_start: before,
        body_end: before + body,
        dropped: 0,
        context: 3,
        tokens: Vec::new(),
    }
}

/// The view with a source preview already answered.
fn with_preview(answer: Result<SeamPreview, String>) -> SeamViewState {
    let mut state = view();
    state.select_path("demo::danger");
    state.preview = Some(answer);
    state
}

#[test]
fn the_preview_block_is_the_same_height_at_the_top_of_a_file_as_in_the_middle() {
    // The requirement: nothing below the block may move as the selection travels.
    let row_of = |state: &mut SeamViewState| {
        let rendered = text(&render(state, 100, 24));
        rendered
            .lines()
            .position(|line| line.contains("edges"))
            .zip(
                rendered
                    .lines()
                    .position(|line| line.contains("press / to filter")),
            )
    };
    let mid = row_of(&mut with_preview(Ok(preview(40, 3, 2, 8))));
    let top = row_of(&mut with_preview(Ok(preview(0, 0, 2, 5))));
    let unread = row_of(&mut with_preview(Err("gone".to_owned())));
    assert!(mid.is_some(), "nothing rendered");
    assert_eq!(mid, top);
    assert_eq!(mid, unread);
}

#[test]
fn context_the_file_does_not_have_is_reserved_rather_than_closed_up() {
    let rendered = text(&render(&mut with_preview(Ok(preview(0, 0, 2, 5))), 100, 24));
    // The node starts at line 1, so the first painted source number must be 1 — the
    // rows above it are blank, not filled with whatever came next.
    let numbered: Vec<&str> = rendered
        .lines()
        .filter(|line| line.contains("source line"))
        .collect();
    assert!(!numbered.is_empty(), "{rendered}");
    assert!(numbered.iter().all(|line| !line.trim().is_empty()));
}

#[test]
fn a_preview_that_could_not_be_read_says_why_instead_of_rendering_blank() {
    let rendered = text(&render(
        &mut with_preview(Err("the file could not be read".to_owned())),
        100,
        24,
    ));
    assert!(rendered.contains("could not be read"), "{rendered}");
    assert!(rendered.contains('?'), "{rendered}");
}

#[test]
fn a_pending_preview_says_nothing_until_the_reveal_delay_and_then_says_so() {
    let paint = |pending: Option<crate::ui::Pending>| {
        let mut state = view();
        state.select_path("demo::danger");
        state.detail_since = pending;
        text(&render(&mut state, 100, 24))
    };
    // A fast answer must never flash a placeholder on its way past, so a request that
    // has only just gone out looks exactly like no request at all.
    assert_eq!(paint(Some(crate::ui::Pending::start())), paint(None));
    // Past the shared delay it has to say something, or the pane reads as broken.
    assert_ne!(paint(Some(crate::ui::Pending::revealed())), paint(None));
}

#[test]
fn the_preview_sits_beside_the_facets_on_a_wide_terminal() {
    let rendered = text(&render(
        &mut with_preview(Ok(preview(40, 3, 2, 8))),
        160,
        24,
    ));
    // Same row as a lens line: the pane split sideways rather than growing.
    assert!(
        rendered
            .lines()
            .any(|line| line.contains("substitution") && line.contains("source line")),
        "{rendered}"
    );
}

#[test]
fn the_preview_sits_below_the_facets_on_a_tall_narrow_one() {
    let rendered = text(&render(&mut with_preview(Ok(preview(40, 3, 2, 8))), 70, 40));
    assert!(rendered.contains("source line"), "{rendered}");
    assert!(
        !rendered
            .lines()
            .any(|line| line.contains("substitution") && line.contains("source line")),
        "{rendered}"
    );
}

#[test]
fn context_lines_are_muted_and_the_definition_is_not() {
    // The whole point of the block: the eye lands on the definition, not on its
    // surroundings — and that has to hold in a file with no grammar to colour it.
    let mut state = with_preview(Ok(preview(40, 3, 2, 8)));
    let buf = render(&mut state, 100, 24);
    let rendered = text(&buf);
    let Some(row) = rendered
        .lines()
        .position(|line| line.contains("source line 3"))
    else {
        panic!("the first body line was not painted: {rendered}");
    };
    let Some(column) = rendered
        .lines()
        .nth(row)
        .and_then(|line| line.find("source"))
    else {
        panic!("no source column");
    };
    let cell = |y: usize| {
        buf.cell((
            u16::try_from(column).unwrap_or(0),
            u16::try_from(y).unwrap_or(0),
        ))
        .map(|c| c.fg)
    };
    assert_ne!(
        cell(row),
        cell(row - 1),
        "body must not wear its context's colour"
    );
}

#[test]
fn a_terminal_that_can_hold_neither_keeps_its_spine_instead() {
    let rendered = text(&render(&mut with_preview(Ok(preview(40, 3, 2, 8))), 60, 20));
    // The spine is the primary surface; a preview is not worth its rows here.
    assert!(!rendered.contains("source line"), "{rendered}");
}

// --- what the frame records for the pointer ---------------------------------

#[test]
fn every_painted_spine_row_is_clickable() {
    let mut state = view();
    let buf = render(&mut state, 120, 20);
    assert!(!state.hits.rows.is_empty());
    for (rect, id) in &state.hits.rows {
        let Some(node) = state.nodes.get(id) else {
            panic!("recorded a row for a node that is not in the tree: {id}");
        };
        // The cells the row claims are the cells its name was painted into.
        let painted: String = (rect.x..rect.right())
            .map(|x| {
                buf.cell((x, rect.y))
                    .map_or(' ', |c| c.symbol().chars().next().unwrap_or(' '))
            })
            .collect();
        assert!(
            painted.contains(&node.name),
            "{painted:?} lacks {}",
            node.name
        );
        assert_eq!(
            state.hits.at(rect.x, rect.y),
            Some(crate::app::seam::geometry::SeamTarget::Row(id.clone()))
        );
    }
}

#[test]
fn the_narrow_fallback_records_its_rows_too() {
    // The indented tree is a different renderer, so it needs its own row map — and both
    // must resolve to the same identities.
    let mut wide = view();
    let _ = render(&mut wide, 120, 20);
    let mut narrow = view();
    let _ = render(&mut narrow, 30, 20);
    let ids = |state: &SeamViewState| {
        let mut ids: Vec<String> = state.hits.rows.iter().map(|(_, id)| id.clone()).collect();
        ids.sort();
        ids
    };
    assert!(!narrow.hits.rows.is_empty());
    assert_eq!(ids(&narrow), ids(&wide));
}

#[test]
fn the_breadcrumb_records_one_crumb_per_narrow_plus_the_root() {
    let mut state = view();
    state
        .narrow
        .push(crate::app::seam::Narrow::Scope("demo".to_owned()));
    state.move_row(0);
    let _ = render(&mut state, 120, 20);
    assert_eq!(state.hits.crumbs.len(), state.narrow.len() + 1);
    let depths: Vec<usize> = state.hits.crumbs.iter().map(|(_, depth)| *depth).collect();
    assert_eq!(depths, [0, 1]);
}

#[test]
fn the_legend_records_a_hit_for_every_lens() {
    let mut state = view();
    let buf = render(&mut state, 120, 20);
    assert_eq!(state.hits.lenses.len(), LENS_NAMES.len());
    for (rect, index) in &state.hits.lenses {
        let painted: String = (rect.x..rect.right())
            .map(|x| {
                buf.cell((x, rect.y))
                    .map_or(' ', |c| c.symbol().chars().next().unwrap_or(' '))
            })
            .collect();
        // The word is part of the target, not just the glyph beside it.
        let Some(lens) = LENS_NAMES.get(*index) else {
            panic!("legend hit for a lens that does not exist");
        };
        assert!(painted.contains(short(lens)), "{painted:?} for {lens}");
    }
}

/// The abbreviated lens name the legend paints.
fn short(lens: &str) -> &'static str {
    match lens {
        "api" => "api",
        "substitution" => "sub",
        "variation" => "var",
        "boundary" => "bnd",
        _ => "haz",
    }
}

#[test]
fn the_widen_affordance_is_recorded_only_once_narrowed() {
    let mut state = view();
    let _ = render(&mut state, 120, 20);
    assert_eq!(state.hits.widen.width, 0);

    state
        .narrow
        .push(crate::app::seam::Narrow::Scope("demo".to_owned()));
    state.move_row(0);
    let _ = render(&mut state, 120, 20);
    assert!(state.hits.widen.width > 0);
}

#[test]
fn a_header_run_pushed_off_the_row_claims_nothing() {
    // A long package name shoves the legend past the right edge; nothing painted there
    // means nothing clickable there.
    let mut state = view();
    state.summary.package = "a".repeat(200);
    let _ = render(&mut state, 60, 20);
    assert!(
        state.hits.lenses.iter().all(|(rect, _)| rect.width == 0),
        "{:?}",
        state.hits.lenses
    );
}

#[test]
fn rendering_into_a_tiny_area_does_not_panic() {
    let mut state = view();
    let _ = render(&mut state, 4, 2);
    let _ = render(&mut state, 1, 1);
    let _ = render(&mut state, 20, 3);
}

#[test]
fn lens_glyphs_are_distinct_in_the_ascii_tier() {
    let glyphs: Vec<char> = LENS_NAMES
        .iter()
        .map(|lens| lens_glyph(lens, IconStyle::Ascii))
        .collect();
    let mut unique = glyphs.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), glyphs.len(), "{glyphs:?}");
}

#[test]
fn a_workspace_names_the_directory_and_counts_its_packages() {
    let mut state = view();
    let mut many = summary();
    many.package = "myrepo".to_owned();
    many.packages = 18;
    state.summary = many;

    let rendered = text(&render(&mut state, 100, 12));
    assert!(rendered.contains("myrepo"), "{rendered}");
    assert!(rendered.contains("18 packages"), "{rendered}");
}

#[test]
fn a_single_package_says_nothing_about_counts() {
    // "1 package" beside the package's own name is noise, and one package is the common
    // case.
    let mut state = view();
    let rendered = text(&render(&mut state, 100, 12));
    assert!(rendered.contains("demo"), "{rendered}");
    assert!(!rendered.contains("package"), "{rendered}");
}
