use unicode_width::UnicodeWidthStr;

use super::*;

mod graph;
pub(crate) mod list;
mod rail;
pub(in crate::ui) mod responsive;

pub(in crate::ui) use graph::CommitGraphInput;
pub(in crate::ui) use graph::CommitGraphScroll;
pub(in crate::ui) use graph::draw_commit_graph;
pub(in crate::ui) use list::CommitListEntry;
pub(in crate::ui) use list::commit_list_items;
pub(crate) use list::relative_time;
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
    hits: &mut ScrollHits,
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
    hits.record_both(
        draw_scrollable_lines(f, theme, area, lines, scroll, column),
        ScrollSurface::TabRows,
        ScrollSurface::TabColumns,
    );
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

/// Render one file's diff as a boxed "card": a top rule carrying the status glyph, the
/// path (and the old path for renames), and the `+a −b` stats; each diff line prefixed
/// with a left rail; then a bottom rule. `width` sizes the rules (a small floor keeps a
/// narrow pane from producing a degenerate box).
pub(super) const FILE_CARD_MIN_WIDTH: u16 = 13;

/// Why this file's diff is folded away by default, when it is machine-maintained.
///
/// A lockfile or a minified bundle is real content nobody reads line by line, so
/// the card starts collapsed and names the reason. The classification is
/// `karet-filetype`'s, so the commit view, the file rail, and any future consumer
/// agree on what counts as generated.
pub(super) fn auto_collapse_reason(file: &render::FileView) -> Option<karet_filetype::Generated> {
    karet_filetype::generated_for_path(&file.change.path)
}

/// The parenthesized label shown beside a machine-maintained file, e.g.
/// `"(lockfile)"`. Matches the lowercase, parenthesized phrasing `karet-diff`
/// already uses for its own placeholders.
pub(super) fn auto_collapse_label(file: &render::FileView) -> Option<String> {
    auto_collapse_reason(file).map(|kind| format!("({})", kind.reason()))
}

/// The narrowest path a card header will show before the generated reason is
/// dropped. The path identifies the file; the reason only explains it, so the
/// reason yields first.
const REASON_MIN_PATH: usize = 12;

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

    // The generated reason sits between the path and the dash filler, and is the
    // first thing dropped when the pane narrows — it explains the card, while the
    // path identifies it.
    let reason = auto_collapse_label(file).map(|label| format!(" {label}"));
    let reason_width = reason.as_deref().map_or(0, UnicodeWidthStr::width);
    let show_reason =
        reason_width > 0 && w >= prefix_width + suffix_width + 2 + reason_width + REASON_MIN_PATH;
    let reason_width = if show_reason { reason_width } else { 0 };

    let path_budget = w
        .saturating_sub(prefix_width + suffix_width + reason_width + 2)
        .max(1);
    path = truncate_start(&path, path_budget);
    let path_width = UnicodeWidthStr::width(path.as_str());
    let dashes = w
        .saturating_sub(prefix_width + path_width + reason_width + suffix_width + 1)
        .max(1);

    let mut top: Vec<Span<'static>> = vec![
        Span::styled("\u{256d}\u{2500} ", border),
        Span::styled(format!("{toggle} "), toggle_style),
        Span::styled(format!("{glyph} "), glyph_style),
        Span::styled(path, fg.add_modifier(Modifier::BOLD)),
        // A reviewed file wears its check right after the path.
        Span::styled(
            if file.reviewed { " \u{2713}" } else { "" },
            theme
                .style(ThemeRole::DiagnosticHint)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(reason) = reason.filter(|_| show_reason) {
        top.push(Span::styled(reason, theme.style(ThemeRole::Muted)));
    }
    top.push(Span::styled(
        format!(" {}", "\u{2500}".repeat(dashes)),
        border,
    ));
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
/// Draw a code-visualization graph: flatten via the karet-graph tree renderer
/// (theme mapped onto its plain style slots), then paint scrollably.
#[allow(clippy::too_many_arguments)] // graph model, scroll offsets and the track sink are independent
pub(super) fn draw_graph(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    title: &str,
    view: &karet_core::GraphView,
    scroll: &mut u16,
    column: &mut u16,
    hits: &mut ScrollHits,
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
    // The dependency lens is the only `GraphView` producer today; the flattener names
    // its lens and edge kind so a future usage/call lens reuses it as-is.
    let rows = karet_graph::view::graph_tree_lines(
        &format!("{title} \u{2014} dependency graph"),
        view,
        karet_core::GraphEdgeKind::Dependency,
        &styles,
    );
    hits.record_both(
        draw_scrollable_lines(f, theme, area, rows, scroll, column),
        ScrollSurface::TabRows,
        ScrollSurface::TabColumns,
    );
}
