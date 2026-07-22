use super::*;

/// Draw the loaded settings and provenance as a scrollable read-only report.
pub(super) fn draw_loaded_config(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    report: &LoadedConfig,
    scroll: &mut u16,
) {
    let header = Style::default()
        .fg(theme.role(ThemeRole::LineNumberActive).to_ratatui())
        .add_modifier(Modifier::BOLD);
    let fg = Style::default().fg(theme.role(ThemeRole::Foreground).to_ratatui());
    let explicit = fg.add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(theme.role(ThemeRole::Muted).to_ratatui());
    let badge = Style::default().fg(theme.role(ThemeRole::DiagnosticHint).to_ratatui());
    let warning = Style::default().fg(theme.role(ThemeRole::DiagnosticWarning).to_ratatui());

    let mut lines = Vec::new();
    lines.push(Line::styled(" Loaded Settings", header));
    lines.push(Line::raw(""));
    lines.push(Line::styled(" Layers", header));

    let mut layers = report.layers.clone();
    layers.sort_by_key(|row| std::cmp::Reverse(row.layer));
    if layers.is_empty() {
        lines.push(Line::styled("  no layer provenance captured", muted));
    }
    for row in layers {
        let (status, style) = match &row.status {
            ConfigLayerStatus::Loaded => ("loaded".to_string(), fg),
            ConfigLayerStatus::Missing => ("missing".to_string(), muted),
            ConfigLayerStatus::Invalid(_) => ("invalid".to_string(), warning),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<8}", row.layer.label()), style),
            Span::styled(format!("{status:<9}"), style),
            Span::styled(row.path.to_string_lossy().into_owned(), style),
        ]));
        if let ConfigLayerStatus::Invalid(message) = row.status {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(message, warning),
            ]));
        }
    }

    lines.push(Line::raw(""));
    lines.push(Line::styled(" Diagnostics", header));
    if report.diagnostics.is_empty() {
        lines.push(Line::styled("  none", muted));
    } else {
        for diag in &report.diagnostics {
            let style = severity_style(theme, diag.severity);
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<8}", severity_label(diag.severity)), style),
                Span::styled(format!("{}  ", diag.path.to_string_lossy()), muted),
                Span::styled(diag.message.clone(), style),
            ]));
        }
    }

    lines.push(Line::raw(""));
    lines.push(Line::styled(" Values", header));
    match serde_json::to_value(&report.settings) {
        Ok(serde_json::Value::Object(sections)) => {
            for (section, value) in sections {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(section.clone(), header),
                ]));
                flatten_setting_lines(
                    &mut lines, report, &section, "", &value, explicit, muted, badge,
                );
            }
        },
        _ => lines.push(Line::styled("  settings could not be serialized", warning)),
    }

    let height = area.height as usize;
    let max_scroll = lines.len().saturating_sub(height);
    *scroll = (*scroll).min(max_scroll as u16);
    f.render_widget(Paragraph::new(lines).scroll((*scroll, 0)), area);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn flatten_setting_lines(
    lines: &mut Vec<Line<'static>>,
    report: &LoadedConfig,
    section: &str,
    prefix: &str,
    value: &serde_json::Value,
    explicit: Style,
    muted: Style,
    badge: Style,
) {
    if let serde_json::Value::Object(obj) = value {
        for (key, child) in obj {
            let next = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            flatten_setting_lines(lines, report, section, &next, child, explicit, muted, badge);
        }
        return;
    }

    let full_path = format!("{section}.{prefix}");
    let source = report.explicit.get(&full_path);
    let style = if source.is_some() { explicit } else { muted };
    let source_label = source.map_or("default", |layer| layer.label());
    let value_text = serde_json::to_string(value).unwrap_or_else(|_| "<value>".to_string());
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(format!("{prefix:<24}"), style),
        Span::styled(format!("{value_text:<26}"), style),
        Span::styled(source_label.to_string(), source.map_or(muted, |_| badge)),
    ]));
}

pub(super) fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Information => "info",
        Severity::Hint => "hint",
        _ => "info",
    }
}

pub(super) fn severity_style(theme: &Theme, severity: Severity) -> Style {
    let role = match severity {
        Severity::Error => ThemeRole::DiagnosticError,
        Severity::Warning => ThemeRole::DiagnosticWarning,
        Severity::Information => ThemeRole::DiagnosticInfo,
        Severity::Hint => ThemeRole::DiagnosticHint,
        _ => ThemeRole::DiagnosticInfo,
    };
    Style::default().fg(theme.role(role).to_ratatui())
}

/// The breathing room between a markdown preview's rendered text and its pane edges. Prose
/// pinned against the pane border reads as cramped next to the source pane's gutter.
const MARKDOWN_PREVIEW_PADDING: Margin = Margin {
    horizontal: 2,
    vertical: 1,
};

/// The rect a markdown preview paints into: its pane, inset by
/// [`MARKDOWN_PREVIEW_PADDING`]. A pane too small to hold the padding gives up an empty
/// rect rather than painting to the edge.
pub(super) fn markdown_preview_rect(area: Rect) -> Rect {
    area.inner(MARKDOWN_PREVIEW_PADDING)
}

/// Paint a markdown preview, re-parsing and re-wrapping only when the document version or
/// the pane width has moved since the last frame.
///
/// Caching here rather than on every snapshot is what keeps typing cheap: a burst of
/// keystrokes lands many snapshots but only one draw, so it costs one re-parse.
pub(super) fn draw_markdown_preview(
    f: &mut Frame,
    theme: &Theme,
    area: Rect,
    buffer: &TextBuffer,
    wrapped: &mut WrappedDocument,
    rendered: &mut Option<(u64, u16)>,
    scroll: &mut u16,
) {
    // Wrap to the padded width, not the pane's: the cache key follows, so a resize that
    // only moves the padding away still re-wraps exactly once.
    let area = markdown_preview_rect(area);
    let key = (buffer.version(), area.width);
    if *rendered != Some(key) {
        *wrapped = karet_markdown::parse(&buffer.text()).wrap(area.width);
        *rendered = Some(key);
    }
    let mut state = MarkdownViewState { scroll: *scroll };
    f.render_stateful_widget(MarkdownView::new(wrapped, theme), area, &mut state);
    // The widget clamps the scroll to the document; keep the clamped value so a
    // shrinking document doesn't leave the tab scrolled past the end.
    *scroll = state.scroll;
}

/// The commands shown on the empty-editor welcome screen, with descriptions. As in
/// the footer, only this selection and the prose are presentation — each chord is
/// derived from the keymap so the cheat-sheet can't drift from a rebinding.
pub(super) const WELCOME_HINTS: &[(Command, &str)] = &[
    (Command::OpenQuickOpen, "go to file"),
    (Command::OpenCommandPalette, "command palette"),
    (Command::ToggleSidebar, "toggle sidebar"),
    (Command::OpenGlobalSearch, "search the workspace"),
    (Command::Copy, "copy selection"),
    (Command::ToggleFocus, "switch focus"),
    (Command::Quit, "quit"),
];

pub(super) fn draw_welcome(f: &mut Frame, theme: &Theme, area: Rect) {
    let dim = Style::default().fg(theme.role(ThemeRole::LineNumber).to_ratatui());
    let title = Style::default()
        .fg(theme.role(ThemeRole::Foreground).to_ratatui())
        .add_modifier(Modifier::BOLD);
    let mut text = vec![Line::raw(""), Line::styled("  karet", title), Line::raw("")];
    for &(cmd, desc) in WELCOME_HINTS {
        let chord = keymap::hint_for(cmd, ChordStyle::Verbose).unwrap_or_default();
        text.push(Line::styled(format!("  {chord:<14}{desc}"), dim));
    }
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), area);
}

/// The separator drawn between adjacent hints in the status bar.
const HINT_SEP: &str = " · ";

/// A single hint's segment text (`"^S save"`), used for both measuring and drawing.
pub(super) fn hint_segment(hint: &keymap::Hint) -> String {
    format!("{} {}", hint.chord, hint.verb)
}

/// The terminal-cell width of `s` (display width, wide/combining aware — unlike a
/// raw `chars().count()`), via ratatui's own measurement so no extra dependency is
/// pulled in.
pub(super) fn cell_width(s: &str) -> u16 {
    u16::try_from(Span::raw(s).width()).unwrap_or(u16::MAX)
}

/// How many leading `hints` fit in `avail` columns when joined by [`HINT_SEP`].
/// When some don't fit, room is reserved for a trailing ` +N` overflow marker (a
/// hint is dropped if the marker wouldn't otherwise fit). Pure, so it is unit-tested.
pub(super) fn pack_hints(hints: &[keymap::Hint], avail: u16) -> usize {
    let sep = cell_width(HINT_SEP);
    let mut used = 0u16;
    let mut shown = 0usize;
    for (i, hint) in hints.iter().enumerate() {
        let seg = cell_width(&hint_segment(hint)) + if i == 0 { 0 } else { sep };
        if used + seg > avail {
            break;
        }
        used += seg;
        shown += 1;
    }
    // Reserve room for the ` +N` marker by dropping trailing hints until it fits.
    while shown < hints.len() && shown > 0 {
        let marker = cell_width(&format!(" +{}", hints.len() - shown));
        if used + marker <= avail {
            break;
        }
        shown -= 1;
        let seg = cell_width(&hint_segment(&hints[shown]));
        used -= seg + if shown == 0 { 0 } else { sep };
    }
    shown
}

/// Render `hints` into `spans` starting at column `*x`, packing what fits in `avail`
/// and appending a clickable ` +N` marker (opens the palette) for the rest. Each
/// shown hint records a clickable `(start, end, command)` region in `hits`.
pub(super) fn render_hints(
    hints: &[keymap::Hint],
    spans: &mut Vec<Span<'static>>,
    hits: &mut Vec<(u16, u16, Command)>,
    x: &mut u16,
    avail: u16,
    bar: Style,
    key: Style,
) {
    let shown = pack_hints(hints, avail);
    for (i, hint) in hints.iter().take(shown).enumerate() {
        if i > 0 {
            spans.push(Span::styled(HINT_SEP.to_string(), bar));
            *x += cell_width(HINT_SEP);
        }
        let start = *x;
        spans.push(Span::styled(hint.chord.clone(), key));
        spans.push(Span::styled(format!(" {}", hint.verb), bar));
        *x += cell_width(&hint.chord) + 1 + cell_width(hint.verb);
        hits.push((start, *x, hint.command));
    }
    if shown < hints.len() {
        let marker = format!(" +{}", hints.len() - shown);
        let start = *x;
        *x += cell_width(&marker);
        spans.push(Span::styled(marker, bar));
        // The overflow marker opens the palette, so the hidden commands stay reachable.
        hits.push((start, *x, Command::OpenCommandPalette));
    }
}
