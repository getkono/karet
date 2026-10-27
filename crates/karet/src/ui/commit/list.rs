//! The commit-row vocabulary shared by Source Control, the commit-graph view, and the
//! GitHub pull-request `Commits` list.
//!
//! Keeping the rail/hash/refs/summary/age composition and the lane palette in one
//! place is what stops those three screens from drifting apart. The lane layout of
//! a `karet-vcs` log, the keep-the-cursor-visible clamp, and the relative-age format
//! are app logic and live in `app/util.rs`.

use std::collections::HashMap;

use karet_graph::LaneInput;
use karet_graph::RailRow;
use karet_graph::assign_lanes;
use karet_graph::view::render_rail;

use super::*;

/// One commit row's render inputs, independent of where the commit came from.
pub(in crate::ui) struct CommitListEntry<'a> {
    /// The full hash, which identifies the node in the lane layout.
    pub(in crate::ui) hash: &'a str,
    /// The abbreviated hash shown in the hash column.
    pub(in crate::ui) short_hash: &'a str,
    /// The commit summary (first message line).
    pub(in crate::ui) summary: &'a str,
    /// Commit time, seconds since the Unix epoch.
    pub(in crate::ui) time: i64,
    /// Full parent hashes, first-parent first — the DAG edges the lane layout walks.
    pub(in crate::ui) parents: &'a [String],
    /// Whether this row is the tip (drawn with the `HEAD` glyph).
    pub(in crate::ui) head: bool,
    /// Refs decorating this commit.
    pub(in crate::ui) labels: &'a [karet_vcs::RefLabel],
}

/// The lane colour cycle, resolved through the theme so the rail obeys the loaded
/// palette like every other span on the row. Six roles that read apart from each other;
/// `karet-graph` stays theme-free by taking this as a closure.
const LANE_ROLES: [ThemeRole; 6] = [
    ThemeRole::DiagnosticInfo,
    ThemeRole::DiffAdded,
    ThemeRole::DiagnosticWarning,
    ThemeRole::DiagnosticHint,
    ThemeRole::LineNumberActive,
    ThemeRole::DiagnosticError,
];

/// The style for lane `lane`, cycling through [`LANE_ROLES`].
pub(in crate::ui) fn lane_style(theme: &Theme, lane: u8) -> Style {
    theme.style(LANE_ROLES[lane as usize % LANE_ROLES.len()])
}

/// The lane layout [`crate::tab::commit_rails`] builds from a `karet-vcs` log, from
/// already-built render entries (the `List`-based callers, whose rows do not come from
/// a log).
pub(in crate::ui) fn rails_from_entries(entries: &[CommitListEntry<'_>]) -> Vec<RailRow> {
    let inputs: Vec<LaneInput> = entries
        .iter()
        .map(|entry| LaneInput {
            id: entry.hash.to_string(),
            parents: entry.parents.to_vec(),
            head: entry.head,
        })
        .collect();
    assign_lanes(&inputs)
}

/// Build render entries for a slice of `karet-vcs` commits, decorating each with any
/// refs that point at it. `head` marks the row that carries the `HEAD` glyph.
pub(in crate::ui) fn entries_from_commits<'a>(
    commits: &'a [karet_vcs::Commit],
    labels: &'a HashMap<String, Vec<karet_vcs::RefLabel>>,
    head: Option<usize>,
) -> Vec<CommitListEntry<'a>> {
    commits
        .iter()
        .enumerate()
        .map(|(i, commit)| CommitListEntry {
            hash: &commit.hash,
            short_hash: commit.short_hash(),
            summary: &commit.summary,
            time: commit.time,
            parents: &commit.parents,
            head: head == Some(i),
            labels: labels
                .get(commit.hash.as_str())
                .map(Vec::as_slice)
                .unwrap_or_default(),
        })
        .collect()
}

/// Render one commit row: rail gutter, hash, ref chips, summary, relative age.
///
/// `rail` is the row's lane gutter, or `None` for a list with no DAG (the GitHub
/// pull-request commits, which have no local parent graph).
pub(in crate::ui) fn commit_line(
    theme: &Theme,
    entry: &CommitListEntry<'_>,
    rail: Option<&RailRow>,
    selected: bool,
) -> Line<'static> {
    let hash_style = theme.style(ThemeRole::DiagnosticWarning);
    let dim = theme.style(ThemeRole::LineNumber);
    let mut spans = vec![Span::raw(" ")];
    if let Some(rail) = rail {
        spans.extend(render_rail(rail, |lane| lane_style(theme, lane)).spans);
    }
    spans.push(Span::styled(format!(" {} ", entry.short_hash), hash_style));
    for label in entry.labels {
        let (glyph, role) = match label.kind {
            karet_vcs::RefKind::Local => ("\u{e0a0} ", ThemeRole::DiffAdded),
            karet_vcs::RefKind::Remote => ("\u{f408} ", ThemeRole::DiagnosticInfo),
            karet_vcs::RefKind::Tag => ("\u{2302} ", ThemeRole::DiagnosticWarning),
            karet_vcs::RefKind::Head => ("\u{f5d2} ", ThemeRole::LineNumberActive),
            _ => ("", ThemeRole::Muted),
        };
        spans.push(Span::styled(
            format!("[{glyph}{}] ", label.name),
            theme.style(role).add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::raw(entry.summary.to_string()));
    spans.push(Span::styled(
        format!("  {}", relative_time(entry.time)),
        dim,
    ));
    let line = Line::from(spans);
    if selected {
        line.style(Style::default().bg(theme.role(ThemeRole::Selection).to_ratatui()))
    } else {
        line
    }
}

/// Render every entry as a [`Line`], laying out the lane rails across the whole slice.
pub(in crate::ui) fn commit_list_lines(
    theme: &Theme,
    entries: &[CommitListEntry<'_>],
    rails: &[RailRow],
    selected: Option<usize>,
) -> Vec<Line<'static>> {
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| commit_line(theme, entry, rails.get(index), selected == Some(index)))
        .collect()
}

/// The [`ListItem`] form of [`commit_list_lines`], for the `List`-based callers. Lanes
/// are laid out from the entries themselves.
pub(in crate::ui) fn commit_list_items(
    theme: &Theme,
    entries: &[CommitListEntry<'_>],
    selected: Option<usize>,
) -> Vec<ListItem<'static>> {
    let rails = rails_from_entries(entries);
    commit_list_lines(theme, entries, &rails, selected)
        .into_iter()
        .map(ListItem::new)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tab::commit_rails;

    fn commit(hash: &str, parents: Vec<String>) -> karet_vcs::Commit {
        karet_vcs::Commit {
            hash: hash.to_string(),
            summary: format!("summary of {hash}"),
            author: "Tester".to_string(),
            time: 0,
            parents,
        }
    }

    /// Lane colours resolve through the theme rather than a fixed ANSI palette, and the
    /// cycle wraps rather than panicking on a high lane index.
    #[test]
    fn lane_style_cycles_through_theme_roles() {
        let theme = Theme::default();
        assert_eq!(lane_style(&theme, 0), theme.style(LANE_ROLES[0]));
        // Wraps around the cycle.
        let wrapped = u8::try_from(LANE_ROLES.len()).unwrap_or(u8::MAX);
        assert_eq!(lane_style(&theme, wrapped), lane_style(&theme, 0));
        assert_eq!(lane_style(&theme, 255), theme.style(LANE_ROLES[255 % 6]));
    }

    /// Both rail builders describe the same DAG, so a log and the entries built from it
    /// must lay out identically — that equivalence is what lets the graph view cache.
    #[test]
    fn both_rail_builders_agree() {
        let commits = vec![
            commit("dddd", vec!["cccc".to_string(), "bbbb".to_string()]),
            commit("cccc", vec!["aaaa".to_string()]),
            commit("bbbb", vec!["aaaa".to_string()]),
            commit("aaaa", Vec::new()),
        ];
        let labels = HashMap::new();
        let entries = entries_from_commits(&commits, &labels, Some(0));
        assert_eq!(commit_rails(&commits), rails_from_entries(&entries));
    }

    /// A commit carrying refs renders them as chips between the hash and the summary.
    #[test]
    fn rows_carry_rail_hash_refs_summary_and_age() {
        let commits = vec![commit("aaaa", Vec::new())];
        let mut labels = HashMap::new();
        labels.insert(
            "aaaa".to_string(),
            vec![karet_vcs::RefLabel {
                name: "main".to_string(),
                kind: karet_vcs::RefKind::Local,
            }],
        );
        let entries = entries_from_commits(&commits, &labels, Some(0));
        let rails = commit_rails(&commits);
        let lines = commit_list_lines(&Theme::default(), &entries, &rails, Some(0));
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("aaaa"), "short hash: {text}");
        assert!(text.contains("[\u{e0a0} main]"), "ref chip: {text}");
        assert!(text.contains("summary of aaaa"), "summary: {text}");
        assert!(text.contains("ago"), "relative age: {text}");
    }
}
