//! Render tests for the full-screen commit-graph view.

use super::super::*;

/// The graph view's header names the branch, its divergence, and the tip commit; the
/// graph itself then gets the entire pane below it, with no detail column reserved.
#[test]
fn commit_graph_header_reports_branch_divergence_and_tip() -> Result<(), std::convert::Infallible> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::ui::commit::CommitGraphInput;
    use crate::ui::commit::CommitGraphScroll;
    use crate::ui::commit::draw_commit_graph;

    let commits = vec![karet_vcs::Commit {
        hash: "aaaa1111".to_string(),
        short_hash: "aaaa111".to_string(),
        summary: "feat: land the graph view".to_string(),
        author: "Ada".to_string(),
        time: 0,
        parents: Vec::new(),
    }];
    let rails = crate::ui::commit::list::commit_rails(&commits);
    let labels = std::collections::HashMap::new();
    let state = karet_vcs::RepositoryState {
        branch: Some("feat/graph".to_string()),
        upstream: Some("origin/feat/graph".to_string()),
        ahead: 2,
        behind: 1,
        ..karet_vcs::RepositoryState::default()
    };

    let backend = TestBackend::new(90, 10);
    let mut terminal = Terminal::new(backend)?;
    let (mut list_offset, mut column) = (0u16, 0u16);
    let mut list_rect = Rect::ZERO;
    let mut hits = ScrollHits::default();
    terminal.draw(|frame| {
        draw_commit_graph(
            frame,
            &Theme::dark(),
            frame.area(),
            &CommitGraphInput {
                history_path: None,
                commits: &commits,
                rails: &rails,
                labels: &labels,
                repo_state: Some(&state),
                has_more: false,
                loading: false,
                loading_since: None,
                selected: 0,
            },
            CommitGraphScroll {
                list_offset: &mut list_offset,
                column: &mut column,
                list_rect: &mut list_rect,
            },
            &mut hits,
        );
    })?;

    let row = |y: u16| -> String {
        let buffer = terminal.backend().buffer();
        (0..90)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect()
    };
    let header = row(0);
    assert!(header.contains("feat/graph"), "branch: {header:?}");
    assert!(header.contains("origin/feat/graph"), "upstream: {header:?}");
    assert!(header.contains("\u{2191}2"), "ahead: {header:?}");
    assert!(header.contains("\u{2193}1"), "behind: {header:?}");

    let tip = row(1);
    assert!(tip.contains("aaaa111"), "tip hash: {tip:?}");
    assert!(tip.contains("Ada"), "tip author: {tip:?}");
    assert!(tip.contains("1 commits loaded"), "loaded count: {tip:?}");

    // The commit row sits below the header rule and runs the full width — the pane is
    // not split with a detail column.
    let first = row(3);
    assert!(first.contains("aaaa111"), "commit row: {first:?}");
    assert!(
        first.contains("feat: land the graph view"),
        "summary reaches across the pane: {first:?}"
    );
    // The painted viewport height is reported back so history can be prefetched.
    assert!(list_rect.height > 0, "the painted rows rect is recorded");
    Ok(())
}
