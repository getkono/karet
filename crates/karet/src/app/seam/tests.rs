//! Navigation, narrowing, and the state's equivalence to a query string.

use karet_core::Range;

use super::*;

/// Build a node view with the given parent and children.
fn node(id: &str, parent: Option<&str>, children: &[&str], rollups: [u32; 5]) -> SeamNodeView {
    SeamNodeView {
        id: id.to_owned(),
        name: id.rsplit("::").next().unwrap_or(id).to_owned(),
        kind: "module".to_owned(),
        detail: None,
        file: PathBuf::from("src/lib.rs"),
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

/// A three-level tree:
/// ```text
/// pkg ── model ── Symbol
///     │        └─ Hidden
///     └─ net   ── connect
/// ```
fn view() -> SeamViewState {
    let nodes = vec![
        node("pkg", None, &["pkg::model", "pkg::net"], [4, 1, 0, 0, 1]),
        node(
            "pkg::model",
            Some("pkg"),
            &["pkg::model::Symbol", "pkg::model::Hidden"],
            [2, 0, 0, 0, 0],
        ),
        node(
            "pkg::model::Symbol",
            Some("pkg::model"),
            &[],
            [1, 0, 0, 0, 0],
        ),
        node(
            "pkg::model::Hidden",
            Some("pkg::model"),
            &[],
            [1, 0, 0, 0, 0],
        ),
        node(
            "pkg::net",
            Some("pkg"),
            &["pkg::net::connect"],
            [2, 1, 0, 0, 1],
        ),
        node("pkg::net::connect", Some("pkg::net"), &[], [1, 1, 0, 0, 1]),
    ];
    let mut state = SeamViewState::pending(std::path::PathBuf::new());
    state.adopt(SeamSummary::default(), nodes);
    state
}

#[test]
fn a_fresh_view_is_loading_until_a_tree_arrives() {
    let state = SeamViewState::pending(std::path::PathBuf::new());
    assert!(state.is_loading());
    assert!(state.columns()[0].is_empty());

    let ready = view();
    assert!(!ready.is_loading());
    assert_eq!(ready.roots, ["pkg"]);
}

#[test]
fn a_failure_is_recorded_rather_than_left_looking_like_an_empty_package() {
    let mut state = SeamViewState::pending(std::path::PathBuf::new());
    state.fail("no Cargo.toml".to_owned());
    assert!(!state.is_loading());
    assert_eq!(state.error.as_deref(), Some("no Cargo.toml"));
}

#[test]
fn columns_cascade_from_the_selection() {
    let mut state = view();
    assert_eq!(state.columns().len(), 1);

    state.move_row(0);
    assert_eq!(state.selected_id(), Some("pkg"));
    // Selecting the root opens a column of its children.
    assert_eq!(state.columns().len(), 2);
    assert_eq!(state.columns()[1], ["pkg::model", "pkg::net"]);

    state.move_column(1);
    state.move_row(0);
    assert_eq!(state.selected_id(), Some("pkg::model"));
    assert_eq!(
        state.columns()[2],
        ["pkg::model::Symbol", "pkg::model::Hidden"]
    );
}

#[test]
fn moving_within_a_column_invalidates_everything_to_its_right() {
    let mut state = view();
    state.move_row(0);
    state.move_column(1);
    state.move_row(0);
    state.move_column(1);
    state.move_row(0);
    assert_eq!(state.selection.len(), 3);

    // Going back and choosing a different sibling must not leave the old subtree selected.
    state.focused_column = 1;
    state.move_row(1);
    assert_eq!(state.selected_id(), Some("pkg::net"));
    assert_eq!(state.selection.len(), 2);
}

#[test]
fn moving_past_the_ends_of_a_column_clamps() {
    let mut state = view();
    state.move_row(0);
    state.move_column(1);
    state.move_row(99);
    assert_eq!(state.selected_id(), Some("pkg::net"));
    state.move_row(-99);
    assert_eq!(state.selected_id(), Some("pkg::model"));
}

#[test]
fn moving_past_the_last_column_does_nothing() {
    let mut state = view();
    state.move_row(0);
    let before = state.focused_column;
    state.move_column(50);
    assert_eq!(state.focused_column, before);
}

// --- narrowing --------------------------------------------------------------

#[test]
fn rerooting_narrows_to_a_subtree_and_widening_returns_to_it() {
    let mut state = view();
    state.move_row(0);
    state.move_column(1);
    state.move_row(0); // pkg::model

    assert_eq!(state.reroot(), Reroot::Narrowed);
    assert_eq!(state.root_set(), ["pkg::model"]);
    assert_eq!(state.narrow.len(), 1);

    assert!(state.widen());
    assert!(state.narrow.is_empty());
    // Stepping out lands back on what was rerooted, not at the top of a list.
    assert_eq!(state.selected_id(), Some("pkg::model"));
}

#[test]
fn a_leaf_cannot_be_rerooted() {
    let mut state = view();
    state.select_path("pkg::model::Symbol");
    // Rerooting onto something with nothing under it would produce an empty view.
    assert_eq!(state.reroot(), Reroot::Refused);
    assert!(state.narrow.is_empty());
}

#[test]
fn pressing_enter_twice_never_stacks_the_same_scope_twice() {
    let mut state = view();
    state.select_path("pkg::model");
    assert_eq!(state.reroot(), Reroot::Narrowed);

    // Back onto the lone root row, where the second Enter used to land the reader.
    state.focused_column = 0;
    state.move_row(0);
    assert_eq!(state.selected_id(), Some("pkg::model"));

    assert_eq!(state.reroot(), Reroot::Descended);
    // The breadcrumb is what the reader watched grow, so that is what this pins.
    let crumbs: Vec<String> = state.narrow.iter().map(Narrow::label).collect();
    assert_eq!(crumbs, ["model"]);
}

#[test]
fn rerooting_lands_focus_inside_the_new_subtree() {
    let mut state = view();
    state.select_path("pkg::model");
    assert_eq!(state.reroot(), Reroot::Narrowed);
    // Narrowing and then leaving the reader on the row they came from looks inert.
    assert_eq!(state.focused_column, 1);
    assert_eq!(state.selected_id(), Some("pkg::model::Symbol"));
}

#[test]
fn enter_on_the_current_root_steps_into_it_rather_than_narrowing() {
    let mut state = view();
    state.select_path("pkg::model");
    assert_eq!(state.reroot(), Reroot::Narrowed);
    state.focused_column = 0;
    state.move_row(0);

    assert_eq!(state.reroot(), Reroot::Descended);
    assert_eq!(state.focused_column, 1);
    assert_eq!(state.narrow.len(), 1);
}

#[test]
fn the_sole_root_of_a_single_package_index_is_never_pushed_as_a_narrow() {
    let mut state = view();
    state.move_row(0); // pkg, the only root
    // Rerooting the one thing on offer removes nothing from view; it only adds a crumb.
    assert_eq!(state.reroot(), Reroot::Descended);
    assert!(state.narrow.is_empty());
    assert_eq!(state.focused_column, 1);
}

#[test]
fn one_backspace_undoes_one_enter() {
    let mut state = view();
    state.move_row(0); // pkg
    assert_eq!(state.reroot(), Reroot::Descended); // steps into pkg, no crumb
    assert_eq!(state.reroot(), Reroot::Narrowed); // pkg::model narrows for real
    assert_eq!(state.reroot(), Reroot::Refused); // pkg::model::Symbol is a leaf

    assert_eq!(state.narrow.len(), 1);
    assert!(state.widen());
    assert!(!state.widen());
}

#[test]
fn a_pivot_onto_the_current_root_set_is_refused() {
    let mut state = view();
    state.select_path("pkg::model");
    assert_eq!(state.reroot(), Reroot::Narrowed);
    // Following an edge back to where the reader already stands moves nothing.
    assert!(!state.pivot("implements", "pkg::model", vec!["pkg::model".to_owned()]));
    assert_eq!(state.narrow.len(), 1);
}

#[test]
fn widening_from_the_top_is_a_no_op() {
    let mut state = view();
    assert!(!state.widen());
}

#[test]
fn a_pivot_pushes_onto_the_same_stack_as_a_scope_narrow() {
    let mut state = view();
    state.select_path("pkg::net::connect");

    assert!(state.pivot(
        "implements",
        "pkg::net::connect",
        vec!["pkg::model::Symbol".to_owned()]
    ));
    assert_eq!(state.root_set(), ["pkg::model::Symbol"]);
    // Reversible the same way, through the same breadcrumb.
    assert!(state.widen());
    assert!(state.narrow.is_empty());
}

#[test]
fn a_pivot_reaching_nothing_in_this_tree_does_not_narrow() {
    let mut state = view();
    // Leaving the reader in an empty view would be worse than refusing the pivot.
    assert!(!state.pivot("implements", "pkg", vec!["other::Thing".to_owned()]));
    assert!(state.narrow.is_empty());
}

#[test]
fn narrows_stack_and_unwind_one_at_a_time() {
    let mut state = view();
    state.select_path("pkg::model");
    assert_eq!(state.reroot(), Reroot::Narrowed);
    state.move_row(0);
    assert_eq!(state.narrow.len(), 1);

    assert!(state.widen());
    assert!(!state.widen());
}

#[test]
fn a_breadcrumb_entry_labels_itself_by_its_leaf() {
    assert_eq!(
        Narrow::Scope("pkg::model::Symbol".to_owned()).label(),
        "Symbol"
    );
    assert_eq!(
        Narrow::Pivot {
            edge: "implements".to_owned(),
            from: "pkg::T".to_owned(),
            targets: Vec::new()
        }
        .label(),
        "implements ▸"
    );
}

// --- filters ----------------------------------------------------------------

#[test]
fn a_lens_matches_a_node_whose_subtree_carries_it() {
    let mut state = view();
    state.toggle_lens("hazard");
    // `pkg` has no hazard facet of its own, but something under it does — hiding the
    // module would hide the thing the reader is looking for.
    assert!(state.matches("pkg"));
    assert!(state.matches("pkg::net"));
    assert!(!state.matches("pkg::model"));
}

#[test]
fn toggling_a_lens_twice_clears_it() {
    let mut state = view();
    state.toggle_lens("api");
    assert_eq!(state.lenses.len(), 1);
    state.toggle_lens("api");
    assert!(state.lenses.is_empty());
    // With no lens active everything matches.
    assert!(state.matches("pkg::model"));
}

#[test]
fn demote_mode_keeps_the_tree_shape_and_hide_mode_does_not() {
    let mut state = view();
    state.toggle_lens("hazard");
    state.select_path("pkg");

    // Demoting leaves both children in place for the renderer to dim.
    assert_eq!(state.columns()[1], ["pkg::model", "pkg::net"]);

    state.lens_filter = LensFilter::Hide;
    assert_eq!(state.columns()[1], ["pkg::net"]);
}

#[test]
fn clearing_lenses_restores_everything() {
    let mut state = view();
    state.toggle_lens("hazard");
    state.toggle_lens("boundary");
    state.clear_lenses();
    assert!(state.lenses.is_empty());
    assert!(state.matches("pkg::model"));
}

#[test]
fn query_matches_narrow_the_visible_set() {
    let mut state = view();
    state.query_matches = Some(
        ["pkg".to_owned(), "pkg::net".to_owned()]
            .into_iter()
            .collect(),
    );
    assert!(state.matches("pkg::net"));
    assert!(!state.matches("pkg::model"));
}

// --- identity-keyed restoration ---------------------------------------------

#[test]
fn a_rename_costs_that_node_its_place_and_nothing_else() {
    let mut state = view();
    state.select_path("pkg::model::Symbol");
    assert_eq!(state.selection.len(), 3);

    // Re-index with `Symbol` renamed; its ancestors are untouched.
    let renamed = vec![
        node("pkg", None, &["pkg::model"], [2, 0, 0, 0, 0]),
        node(
            "pkg::model",
            Some("pkg"),
            &["pkg::model::Renamed"],
            [1, 0, 0, 0, 0],
        ),
        node(
            "pkg::model::Renamed",
            Some("pkg::model"),
            &[],
            [1, 0, 0, 0, 0],
        ),
    ];
    state.adopt(SeamSummary::default(), renamed);

    // The reader keeps their place down to the surviving ancestor.
    assert_eq!(state.selection, ["pkg", "pkg::model"]);
    assert!(state.nodes.contains_key("pkg::model::Renamed"));
}

#[test]
fn a_narrow_whose_root_vanished_is_dropped_rather_than_left_dangling() {
    let mut state = view();
    state.select_path("pkg::model");
    assert_eq!(state.reroot(), Reroot::Narrowed);

    let without_model = vec![
        node("pkg", None, &["pkg::net"], [2, 1, 0, 0, 1]),
        node("pkg::net", Some("pkg"), &[], [2, 1, 0, 0, 1]),
    ];
    state.adopt(SeamSummary::default(), without_model);

    assert!(
        state.narrow.is_empty(),
        "a root that no longer exists cannot be the root"
    );
    assert_eq!(state.root_set(), ["pkg"]);
}

#[test]
fn selecting_a_path_walks_the_columns_down_to_it() {
    let mut state = view();
    state.select_path("pkg::net::connect");
    assert_eq!(state.selection, ["pkg", "pkg::net", "pkg::net::connect"]);
    assert_eq!(state.focused_column, 2);
}

#[test]
fn selecting_something_outside_the_current_root_is_refused() {
    let mut state = view();
    state.select_path("pkg::model");
    assert_eq!(state.reroot(), Reroot::Narrowed);
    let before = state.selection.clone();
    // `pkg::net` is not under the current root, so the walk finds no anchor.
    state.select_path("pkg::net");
    assert_eq!(state.selection, before);
}

// --- totality ---------------------------------------------------------------

#[test]
fn ui_state_serializes_to_the_query_that_would_reproduce_it() {
    let mut state = view();
    state.select_path("pkg::model");
    let _ = state.reroot();
    state.toggle_lens("api");
    state.toggle_lens("hazard");
    state.query = "Symbol".to_owned();

    let query = state.as_query();
    // Everything reached by pressing keys is expressible, which is what lets an agent
    // hand a narrowing back as something the reader can inspect and adopt.
    assert!(query.contains("in:pkg::model"), "{query}");
    assert!(query.contains("lens:api"), "{query}");
    assert!(query.contains("lens:hazard"), "{query}");
    assert!(query.contains("Symbol"), "{query}");
}

#[test]
fn a_pivot_serializes_to_its_pivot_term() {
    let mut state = view();
    state.select_path("pkg::net::connect");
    state.pivot(
        "implements",
        "pkg::net::connect",
        vec!["pkg::model::Symbol".to_owned()],
    );
    assert_eq!(state.as_query(), "pivot:implements:pkg::net::connect");
}

#[test]
fn an_unnarrowed_view_serializes_to_an_empty_query() {
    let state = view();
    assert!(state.as_query().is_empty());
}

#[test]
fn packages_land_in_discovery_order_whatever_order_they_arrive_in() {
    // Packages are read concurrently, so arrival order is completion order. The column
    // the reader sees must not depend on which core finished first.
    let mut state = SeamViewState::pending(PathBuf::new());
    state.begin_sync();

    state.adopt_package(
        2,
        "gamma",
        vec![node("gamma", None, &[], [0; 5])],
        Vec::new(),
    );
    state.adopt_package(
        0,
        "alpha",
        vec![node("alpha", None, &[], [0; 5])],
        Vec::new(),
    );
    state.adopt_package(1, "beta", vec![node("beta", None, &[], [0; 5])], Vec::new());

    assert_eq!(state.roots, ["alpha", "beta", "gamma"]);
    assert_eq!(state.summary.packages, 3);
    assert_eq!(state.summary.nodes, 3);
}

#[test]
fn a_package_is_visible_before_its_neighbours_have_arrived() {
    // The point of streaming: the first rows are usable while the rest is still parsing.
    let mut state = SeamViewState::pending(PathBuf::new());
    state.begin_sync();
    assert!(state.is_loading());

    state.adopt_package(
        0,
        "alpha",
        vec![
            node("alpha", None, &["alpha::one"], [1, 0, 0, 0, 0]),
            node("alpha::one", Some("alpha"), &[], [1, 0, 0, 0, 0]),
        ],
        Vec::new(),
    );

    assert!(!state.is_loading(), "the first package is already readable");
    assert_eq!(state.columns()[0], ["alpha"]);
}

#[test]
fn re_syncing_a_package_replaces_it_rather_than_doubling_it() {
    let mut state = SeamViewState::pending(PathBuf::new());
    state.begin_sync();
    state.adopt_package(
        0,
        "alpha",
        vec![
            node(
                "alpha",
                None,
                &["alpha::gone", "alpha::kept"],
                [2, 0, 0, 0, 0],
            ),
            node("alpha::gone", Some("alpha"), &[], [1, 0, 0, 0, 0]),
            node("alpha::kept", Some("alpha"), &[], [1, 0, 0, 0, 0]),
        ],
        Vec::new(),
    );
    state.finish_sync(SeamSummary::default());
    assert!(state.nodes.contains_key("alpha::gone"));

    // A second sync in which that node no longer exists. Merging alone never removes
    // anything, so a package's first report of a run has to clear what it held.
    state.begin_sync();
    state.adopt_package(
        0,
        "alpha",
        vec![
            node("alpha", None, &["alpha::kept"], [1, 0, 0, 0, 0]),
            node("alpha::kept", Some("alpha"), &[], [1, 0, 0, 0, 0]),
        ],
        Vec::new(),
    );

    assert!(
        !state.nodes.contains_key("alpha::gone"),
        "a removed node survived"
    );
    assert!(state.nodes.contains_key("alpha::kept"));
    assert_eq!(state.roots, ["alpha"], "the root was not duplicated");
    assert_eq!(state.summary.nodes, 2);
}

#[test]
fn a_sync_leaves_the_previous_tree_readable_until_it_is_replaced() {
    // Blanking the view for the length of a sync would trade an answer that is still
    // true for most packages against nothing.
    let mut state = view();
    state.move_row(0);
    let before = state.selected_id().map(str::to_owned);

    state.begin_sync();
    assert!(!state.is_loading());
    assert_eq!(state.selected_id().map(str::to_owned), before);
    assert_eq!(state.roots, ["pkg"]);
}
