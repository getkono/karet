use unicode_width::UnicodeWidthStr;

use super::*;

mod responsive;

pub(super) use responsive::draw_commit;
pub(super) use responsive::draw_compare;

#[allow(clippy::too_many_arguments)] // render inputs and two-axis view state are independent
pub(super) fn draw_commit_loading(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    rev: &str,
    loading_since: Pending,
    error: Option<&str>,
    scroll: &mut u16,
    column: &mut u16,
) {
    *scroll = 0;
    if error.is_none() && !loading_since.visible() {
        f.render_widget(
            Block::default()
                .style(Style::default().bg(theme.role(ThemeRole::Background).to_ratatui())),
            area,
        );
        return;
    }
    let title = theme
        .style(ThemeRole::Foreground)
        .add_modifier(Modifier::BOLD);
    let muted = theme.style(ThemeRole::Muted);
    let error_style = theme.style(ThemeRole::DiagnosticError);
    let hash_style = theme.style(ThemeRole::DiagnosticWarning);
    let short = rev.chars().take(12).collect::<String>();
    let lines = if let Some(error) = error {
        vec![
            Line::styled(" Could not load commit", title),
            Line::from(vec![
                Span::raw(" "),
                Span::styled(short, hash_style),
                Span::styled("  ", muted),
                Span::styled(error.to_string(), error_style),
            ]),
        ]
    } else {
        vec![
            Line::styled(" Loading commit", title),
            Line::from(vec![
                Span::raw(" "),
                Span::styled(short, hash_style),
                Span::styled(" details and file changes…", muted),
            ]),
        ]
    };
    draw_scrollable_lines(f, theme, area, lines, scroll, column);
}

/// Where the signature badge sits within the commit view's line list, so a click can
/// be hit-tested against it: its row index and horizontal column span.
#[derive(Clone, Copy)]
pub(super) struct BadgeHit {
    /// Row index into the commit view's line list (before scrolling).
    pub(super) line: u16,
    /// First column of the badge glyph/label, relative to the render area's left.
    pub(super) col: u16,
    /// The badge's width in columns (glyph + label).
    pub(super) width: u16,
}

/// A short, plain-language explanation of what the signature badge means, keyed on the
/// same four states as [`verified_badge`]. Revealed under the badge on a double-click.
pub(super) fn badge_explanation(
    verification: Option<&karet_session::GithubVerification>,
    signature: Option<&karet_vcs::CommitSignature>,
) -> &'static [&'static str] {
    match verification {
        Some(v) if v.verified => &[
            "Verified \u{2014} a key the forge trusts for this author signed the",
            "commit and the forge confirmed it, proving who wrote it.",
        ],
        Some(_) => &[
            "Unverified \u{2014} this commit is signed, but the forge could not",
            "confirm the signature (see the reason on the signature line below).",
        ],
        None if signature.is_some() => &[
            "Signed \u{2014} this commit carries a cryptographic signature, but it",
            "has not been checked with the forge, so its authenticity is unconfirmed.",
        ],
        None => &[
            "Unsigned \u{2014} no signature is attached, so the author cannot be",
            "cryptographically confirmed beyond the recorded name and email.",
        ],
    }
}

/// The commit's signature badge as `(glyph, label, role)`. Prefers the forge's verdict
/// once fetched; otherwise reports only what the local object records ("Signed" /
/// "Unsigned"), never claiming a verification result the tool did not compute.
pub(super) fn verified_badge(
    verification: Option<&karet_session::GithubVerification>,
    signature: Option<&karet_vcs::CommitSignature>,
) -> (&'static str, &'static str, ThemeRole) {
    match verification {
        Some(v) if v.verified => ("\u{2714}", "Verified", ThemeRole::VcsVerified),
        Some(_) => ("\u{26a0}", "Unverified", ThemeRole::VcsUnverified),
        None if signature.is_some() => ("\u{25cf}", "Signed", ThemeRole::Foreground),
        None => ("", "Unsigned", ThemeRole::Muted),
    }
}

/// Format a Unix timestamp (with its timezone `offset` in seconds) as
/// `YYYY-MM-DD HH:MM`, without pulling in a date library (civil-from-days).
pub(super) fn format_datetime(secs: i64, offset: i32) -> String {
    let t = secs + i64::from(offset);
    let days = t.div_euclid(86_400);
    let tod = t.rem_euclid(86_400);
    let (hour, minute) = (tod / 3600, (tod % 3600) / 60);
    // Howard Hinnant's civil_from_days: days since 1970-01-01 -> (y, m, d).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// Build the commit view's scrollable lines. Shared by the standalone [`TabKind::Commit`]
/// tab and the graph browser's detail pane.
/// When `reveal` is set, the signature badge's explanation is inserted under the badge
/// (a transient tooltip). The returned [`BadgeHit`], if any, locates the badge for
/// click hit-testing.
#[derive(Clone, Copy)]
pub(super) enum CommitFileStatus<'a> {
    Ready,
    Loading(Pending),
    Failed(&'a str),
}

pub(super) fn file_load_status(files: &CommitFiles) -> CommitFileStatus<'_> {
    if let Some(error) = &files.error {
        CommitFileStatus::Failed(error)
    } else if let Some(since) = files.loading_since {
        CommitFileStatus::Loading(since)
    } else {
        CommitFileStatus::Ready
    }
}

#[allow(clippy::too_many_arguments)] // commit metadata, file state, badge state, and width are independent
pub(super) fn commit_metadata_lines(
    theme: &Theme,
    detail: &karet_vcs::CommitDetail,
    verification: Option<&karet_session::GithubVerification>,
    reveal: bool,
) -> (Vec<Line<'static>>, Option<BadgeHit>) {
    let fg = theme.style(ThemeRole::Foreground);
    let subject = fg.add_modifier(Modifier::BOLD);
    let dim = theme.style(ThemeRole::LineNumber);
    let muted = theme.style(ThemeRole::Muted);
    let accent = theme.style(ThemeRole::DiffModified);
    let label = theme.style(ThemeRole::LineNumberActive);
    let hash_style = theme.style(ThemeRole::DiagnosticWarning);
    let bar = || Span::styled("\u{258c} ", accent);

    let mut lines: Vec<Line<'static>> = Vec::new();
    // Subject + body.
    lines.push(Line::styled(format!(" {}", detail.summary), subject));
    if !detail.body.is_empty() {
        lines.push(Line::raw(""));
        for l in detail.body.lines() {
            lines.push(Line::styled(format!(" {l}"), muted));
        }
    }
    lines.push(Line::raw(""));

    // Commit hash + verified badge.
    let (glyph, badge, badge_role) = verified_badge(verification, detail.signature.as_ref());
    let badge_style = theme.style(badge_role).add_modifier(Modifier::BOLD);
    let mut hash_spans = vec![
        bar(),
        Span::styled(format!("{:<10} ", "commit"), label),
        Span::styled(detail.hash.clone(), hash_style),
        Span::raw("   "),
    ];
    // The badge's row and column span, derived from the spans already on the line so
    // hit-testing can't drift from the layout. The badge starts after everything built
    // above (bar + label + hash + gap); its width is the glyph (with a space) + label.
    let badge_col: usize = hash_spans.iter().map(|s| s.content.chars().count()).sum();
    let badge_width = if glyph.is_empty() {
        0
    } else {
        glyph.chars().count() + 1
    } + badge.chars().count();
    let badge_hit = BadgeHit {
        line: u16::try_from(lines.len()).unwrap_or(u16::MAX),
        col: u16::try_from(badge_col).unwrap_or(u16::MAX),
        width: u16::try_from(badge_width).unwrap_or(u16::MAX),
    };
    if !glyph.is_empty() {
        hash_spans.push(Span::styled(format!("{glyph} "), badge_style));
    }
    hash_spans.push(Span::styled(badge, badge_style));
    lines.push(Line::from(hash_spans));

    // On a double-click of the badge, reveal its meaning right beneath it.
    if reveal {
        for text in badge_explanation(verification, detail.signature.as_ref()) {
            lines.push(Line::from(vec![
                bar(),
                Span::styled((*text).to_string(), muted),
            ]));
        }
        lines.push(Line::raw(""));
    }

    // Author, and committer only when it differs.
    let ident_line = |role_label: &str, id: &karet_vcs::Identity, verb: &str| {
        Line::from(vec![
            bar(),
            Span::styled(format!("{role_label:<10} "), label),
            Span::styled(format!("{} <{}>", id.name, id.email), fg),
            Span::styled(
                format!("   {verb} {}", format_datetime(id.time, id.offset)),
                dim,
            ),
        ])
    };
    lines.push(ident_line("author", &detail.author, "authored"));
    if detail.committer.name != detail.author.name
        || detail.committer.email != detail.author.email
        || detail.committer.time != detail.author.time
    {
        lines.push(ident_line("committer", &detail.committer, "committed"));
    }

    // Parents.
    if !detail.parents.is_empty() {
        let mut spans = vec![bar(), Span::styled(format!("{:<10} ", "parents"), label)];
        for (i, p) in detail.parents.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                p.chars().take(7).collect::<String>(),
                hash_style,
            ));
        }
        lines.push(Line::from(spans));
    }

    // Signature detail (type · key, plus the forge reason once known).
    if let Some(sig) = &detail.signature {
        let kind = match sig.kind {
            karet_vcs::SignatureKind::Ssh => "SSH",
            karet_vcs::SignatureKind::OpenPgp => "GPG",
            karet_vcs::SignatureKind::X509 => "X.509",
            _ => "signature",
        };
        let mut text = kind.to_string();
        if let Some(key) = &sig.signer_key {
            text.push_str(&format!(" \u{b7} {key}"));
        }
        if let Some(v) = verification {
            if v.reason != "valid" {
                text.push_str(&format!("  ({})", v.reason));
            }
            if let Some(s) = &v.signer {
                text.push_str(&format!("  {s}"));
            }
        }
        lines.push(Line::from(vec![
            bar(),
            Span::styled(format!("{:<10} ", "signature"), label),
            Span::styled(text, muted),
        ]));
    }

    (lines, Some(badge_hit))
}

#[allow(clippy::too_many_arguments)] // shared graph-browser rendering keeps the full commit vocabulary
pub(super) fn commit_detail_lines(
    theme: &Theme,
    detail: &karet_vcs::CommitDetail,
    files: &CommitFiles,
    reveal: bool,
    width: u16,
) -> (Vec<Line<'static>>, Option<BadgeHit>) {
    let (mut lines, badge) =
        commit_metadata_lines(theme, detail, files.verification.as_ref(), reveal);
    let file_status = file_load_status(files);
    let muted = theme.style(ThemeRole::Muted);
    let label = theme.style(ThemeRole::LineNumberActive);
    match file_status {
        CommitFileStatus::Ready => lines.extend(changed_files_lines(theme, &files.files, width)),
        CommitFileStatus::Loading(since) => {
            lines.push(Line::raw(""));
            if since.visible() {
                lines.push(Line::styled(" loading changed files\u{2026}", muted));
            }
        },
        CommitFileStatus::Failed(error) => {
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled(" changed files unavailable", label),
                Span::raw("   "),
                Span::styled(error.to_string(), muted),
            ]));
        },
    }
    (lines, badge)
}

/// Build the shared "changed files" block: a `N files changed +a −b` summary, a
/// changed-file table of contents, then one boxed diff card per file. Used by both the
/// commit view ([`commit_detail_lines`]) and the compare view so the two render files
/// identically. `width` is the render width, used to size the card rules.
pub(super) fn changed_files_lines(
    theme: &Theme,
    files: &[render::FileView],
    width: u16,
) -> Vec<Line<'static>> {
    let fg = theme.style(ThemeRole::Foreground);
    let label = theme.style(ThemeRole::LineNumberActive);
    let add_fg = theme.style(ThemeRole::DiagnosticHint);
    let rem_fg = theme.style(ThemeRole::DiagnosticError);

    let mut lines: Vec<Line<'static>> = Vec::new();

    // Summary: N files changed, +added −removed.
    let (mut added, mut removed) = (0usize, 0usize);
    for file in files {
        let (a, r) = file.line_stats();
        added += a;
        removed += r;
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled(
            format!(
                " {} file{} changed",
                files.len(),
                if files.len() == 1 { "" } else { "s" }
            ),
            label,
        ),
        Span::raw("   "),
        Span::styled(format!("+{added}"), add_fg),
        Span::raw(" "),
        Span::styled(format!("\u{2212}{removed}"), rem_fg),
    ]));

    // Changed-file table of contents.
    for file in files {
        let (a, r) = file.line_stats();
        let (g, role) = status_glyph(file.change.status);
        lines.push(Line::from(vec![
            Span::styled(format!(" {g}  "), theme.style(role)),
            Span::styled(file.change.path.to_string_lossy().into_owned(), fg),
            Span::styled(format!("   +{a}"), add_fg),
            Span::styled(format!(" \u{2212}{r}"), rem_fg),
        ]));
    }

    // Per-file diff cards.
    for file in files {
        lines.push(Line::raw(""));
        lines.extend(file_card(theme, file, width));
    }
    lines
}

/// Presentation-neutral input for the shared commit-list renderer.
pub(super) struct CommitListEntry<'a> {
    pub(super) hash: &'a str,
    pub(super) short_hash: &'a str,
    pub(super) summary: &'a str,
    pub(super) time: i64,
    pub(super) parents: &'a [String],
    pub(super) head: bool,
}

/// Render the commit rows shared by Source Control, the graph browser, and GitHub
/// pull-request `Commits`. Keeping the rail/hash/summary/time vocabulary here prevents
/// those three screens from drifting apart.
pub(super) fn commit_list_items(
    theme: &Theme,
    entries: &[CommitListEntry<'_>],
    selected: Option<usize>,
    include_header: bool,
) -> Vec<ListItem<'static>> {
    const LANE_COLORS: [Color; 6] = [
        Color::Cyan,
        Color::Green,
        Color::Yellow,
        Color::Magenta,
        Color::Blue,
        Color::Red,
    ];
    let lane_style = |lane: u8| Style::default().fg(LANE_COLORS[lane as usize % LANE_COLORS.len()]);
    let header_style = theme
        .style(ThemeRole::LineNumberActive)
        .add_modifier(Modifier::BOLD);
    let hash_style = theme.style(ThemeRole::DiagnosticWarning);
    let dim = theme.style(ThemeRole::LineNumber);
    let sel_bg = theme.role(ThemeRole::Selection).to_ratatui();
    let inputs: Vec<LaneInput> = entries
        .iter()
        .map(|entry| LaneInput {
            id: entry.hash.to_string(),
            parents: entry.parents.to_vec(),
            head: entry.head,
        })
        .collect();
    let rails = assign_lanes(&inputs);
    let mut items = Vec::with_capacity(entries.len() + usize::from(include_header));
    if include_header {
        items.push(ListItem::new(Line::styled(" COMMITS", header_style)));
    }
    for (index, (entry, rail)) in entries.iter().zip(rails.iter()).enumerate() {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(render_rail(rail, lane_style).spans);
        spans.push(Span::styled(format!(" {} ", entry.short_hash), hash_style));
        spans.push(Span::raw(entry.summary.to_string()));
        spans.push(Span::styled(
            format!("  {}", relative_time(entry.time)),
            dim,
        ));
        let mut line = Line::from(spans);
        if selected == Some(index) {
            line = line.style(Style::default().bg(sel_bg));
        }
        items.push(ListItem::new(line));
    }
    items
}

/// Render one file's diff as a boxed "card": a top rule carrying the status glyph, the
/// path (and the old path for renames), and the `+a −b` stats; each diff line prefixed
/// with a left rail; then a bottom rule. `width` sizes the rules (a small floor keeps a
/// narrow pane from producing a degenerate box).
pub(super) const FILE_CARD_MIN_WIDTH: u16 = 13;

pub(super) fn file_card(theme: &Theme, file: &render::FileView, width: u16) -> Vec<Line<'static>> {
    let mut out = vec![file_card_header(theme, file, width, false)];
    if width < FILE_CARD_MIN_WIDTH {
        return out;
    }
    // Body: each diff line behind a left rail.
    out.extend(file_card_body(theme, file, 0, usize::MAX, width));
    out.push(file_card_footer(theme, width));
    out
}

pub(super) fn file_card_header(
    theme: &Theme,
    file: &render::FileView,
    width: u16,
    collapsed: bool,
) -> Line<'static> {
    let border = theme.style(ThemeRole::LineNumber);
    let fg = theme.style(ThemeRole::Foreground);
    let add_fg = theme.style(ThemeRole::DiagnosticHint);
    let rem_fg = theme.style(ThemeRole::DiagnosticError);
    let (glyph, role) = status_glyph(file.change.status);
    let glyph_style = theme.style(role);
    let toggle_style = theme
        .style(ThemeRole::LineNumberActive)
        .add_modifier(Modifier::BOLD);
    let toggle = if collapsed { "\u{25b8}" } else { "\u{25be}" };
    let (a, r) = file.line_stats();

    let w = usize::from(width);
    let mut path = file.change.path.to_string_lossy().into_owned();
    if let Some(old) = &file.change.old_path {
        path.push_str(&format!(" \u{2190} {}", old.to_string_lossy()));
    }
    let stats = format!("+{a} \u{2212}{r}");

    if w < usize::from(FILE_CARD_MIN_WIDTH) {
        let mut spans = Vec::new();
        if w > 0 {
            spans.push(Span::styled(toggle, toggle_style));
        }
        if w > 1 {
            spans.push(Span::raw(" "));
        }
        if w > 2 {
            spans.push(Span::styled(
                truncate_start(&path, w - 2),
                fg.add_modifier(Modifier::BOLD),
            ));
        }
        return Line::from(spans);
    }

    let prefix_width = 7usize; // "╭─ {toggle} {g} "
    let stats_suffix = format!(" {stats} ─╮");
    let plain_suffix = " ─╮";
    let show_stats = prefix_width + 4 + 2 + UnicodeWidthStr::width(stats_suffix.as_str()) <= w;
    let suffix = if show_stats {
        stats_suffix.as_str()
    } else {
        plain_suffix
    };
    let suffix_width = UnicodeWidthStr::width(suffix);
    let path_budget = w.saturating_sub(prefix_width + suffix_width + 2).max(1);
    path = truncate_start(&path, path_budget);
    let path_width = UnicodeWidthStr::width(path.as_str());
    let dashes = w
        .saturating_sub(prefix_width + path_width + suffix_width + 1)
        .max(1);

    let mut top: Vec<Span<'static>> = vec![
        Span::styled("\u{256d}\u{2500} ", border),
        Span::styled(format!("{toggle} "), toggle_style),
        Span::styled(format!("{glyph} "), glyph_style),
        Span::styled(path, fg.add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {}", "\u{2500}".repeat(dashes)), border),
    ];
    if show_stats {
        top.push(Span::raw(" "));
        top.push(Span::styled(format!("+{a}"), add_fg));
        top.push(Span::raw(" "));
        top.push(Span::styled(format!("\u{2212}{r}"), rem_fg));
        top.push(Span::styled(" \u{2500}\u{256e}", border));
    } else {
        top.push(Span::styled(plain_suffix, border));
    }

    Line::from(top)
}

pub(super) fn file_card_body(
    theme: &Theme,
    file: &render::FileView,
    start: usize,
    count: usize,
    width: u16,
) -> Vec<Line<'static>> {
    let border = theme.style(ThemeRole::LineNumber);
    let mut out = Vec::new();
    for line in render::unified_lines_window(file, theme, start, count) {
        let style = line.style;
        let mut spans = vec![Span::styled("\u{2502} ", border)];
        spans.extend(line.spans);
        let mut line = Line::from(spans);
        line.style = style;
        out.push(line);
    }
    render::pad_diff_lines(&mut out, width);
    out
}

pub(super) fn file_card_footer(theme: &Theme, width: u16) -> Line<'static> {
    let border = theme.style(ThemeRole::LineNumber);
    let width = usize::from(width);
    Line::styled(
        format!(
            "\u{2570}{}\u{256f}",
            "\u{2500}".repeat(width.saturating_sub(2))
        ),
        border,
    )
}

/// Keep the right-most, most-specific part of `text` within `max` terminal cells.
pub(super) fn truncate_start(text: &str, max: usize) -> String {
    karet_widgets::text::fit_start(text, max)
}

/// Draw the full-screen commit graph browser: a DAG commit list on the left and the
/// selected commit's detail on the right.
#[allow(clippy::too_many_arguments)] // a browser pane genuinely has many independent inputs
pub(super) fn draw_commit_graph(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    commits: &[karet_vcs::Commit],
    has_more: bool,
    loading: bool,
    loading_since: Option<Pending>,
    selected: usize,
    detail_loading_since: Option<Pending>,
    detail: Option<&karet_vcs::CommitDetail>,
    files: &CommitFiles,
    list_offset: &mut u16,
    detail_column: &mut u16,
) {
    let cols = Layout::horizontal([
        Constraint::Percentage(42),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(area);
    let (list_area, detail_area) = (cols[0], cols[2]);
    f.render_widget(Block::new().borders(Borders::LEFT), cols[1]);

    let dim = theme.style(ThemeRole::LineNumber);
    let entries: Vec<CommitListEntry<'_>> = commits
        .iter()
        .enumerate()
        .map(|(i, commit)| CommitListEntry {
            hash: &commit.hash,
            short_hash: &commit.short_hash,
            summary: &commit.summary,
            time: commit.time,
            parents: &commit.parents,
            head: i == 0,
        })
        .collect();
    let mut items = commit_list_items(theme, &entries, Some(selected), true);
    if loading && commits.is_empty() && loading_since.is_some_and(Pending::visible) {
        items.push(ListItem::new(Line::styled(" loading\u{2026}", dim)));
    } else if has_more {
        items.push(ListItem::new(Line::styled(" \u{22ef} more", dim)));
    }

    // Keep the selected row (offset by the header) visible.
    let total = items.len();
    let (list_area, tracks) = reserve_tracks(list_area, ScrollAxes::VERTICAL);
    let height = list_area.height as usize;
    let sel_row = selected + 1;
    let mut off = *list_offset as usize;
    if sel_row < off {
        off = sel_row;
    } else if height > 0 && sel_row >= off + height {
        off = sel_row + 1 - height;
    }
    *list_offset = u16::try_from(off).unwrap_or(u16::MAX);
    let mut state = ListState::default();
    *state.offset_mut() = off;
    f.render_stateful_widget(List::new(items), list_area, &mut state);
    tracks.paint(
        f.buffer_mut(),
        ScrollbarStyles::from_theme(theme),
        ScrollExtent::new(total, state.offset(), height),
        ScrollExtent::default(),
    );

    // Right: the selected commit's detail (once its fetch answers).
    let sel_hash = commits.get(selected).map(|c| c.hash.as_str());
    match detail {
        Some(d) if Some(d.hash.as_str()) == sel_hash => {
            draw_horizontally_scrollable_lines(
                f,
                theme,
                detail_area,
                commit_detail_lines(theme, d, files, false, detail_area.width).0,
                detail_column,
            );
        },
        _ => {
            let pending_since = if commits.is_empty() {
                loading_since
            } else {
                detail_loading_since
            };
            if pending_since.is_some_and(Pending::visible) {
                let msg = if commits.is_empty() {
                    "loading commits\u{2026}"
                } else {
                    "loading commit details\u{2026}"
                };
                f.render_widget(
                    Paragraph::new(Line::styled(format!("  {msg}"), dim)),
                    detail_area,
                );
            } else {
                f.render_widget(
                    Block::default()
                        .style(Style::default().bg(theme.role(ThemeRole::Background).to_ratatui())),
                    detail_area,
                );
            }
        },
    }
}

/// Draw a code-visualization graph: flatten via the karet-graph tree renderer
/// (theme mapped onto its plain style slots), then paint scrollably.
pub(super) fn draw_graph(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    title: &str,
    view: &karet_core::GraphView,
    scroll: &mut u16,
    column: &mut u16,
) {
    let styles = karet_graph::view::TreeStyles {
        header: theme
            .style(ThemeRole::LineNumberActive)
            .add_modifier(Modifier::BOLD),
        guide: theme.style(ThemeRole::LineNumber),
        name: theme.style(ThemeRole::Foreground),
        badge: theme.style(ThemeRole::LineNumber),
        revisit: theme.style(ThemeRole::DiagnosticWarning),
    };
    let rows = karet_graph::view::graph_tree_lines(title, view, &styles);
    draw_scrollable_lines(f, theme, area, rows, scroll, column);
}
