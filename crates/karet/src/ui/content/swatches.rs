use super::*;

/// Swatch decorations for the color literals on the lines a viewport of
/// `height` rows starting at `first_line` can show (doubled for wrap slack).
pub(super) fn color_swatch_decorations(
    buffer: &TextBuffer,
    first_line: u32,
    height: u16,
) -> Vec<Decoration> {
    let mut out = Vec::new();
    let end = first_line.saturating_add(u32::from(height) * 2);
    for line in first_line..=end {
        let Some(text) = buffer.line(line as usize) else {
            break;
        };
        for (range, rgba) in karet_syntax::color::detect(&text) {
            out.push(Decoration {
                range: karet_core::Range {
                    start: karet_core::LineCol::new(line, u32::try_from(range.start).unwrap_or(0)),
                    end: karet_core::LineCol::new(
                        line,
                        u32::try_from(range.end).unwrap_or(u32::MAX),
                    ),
                },
                kind: karet_core::DecorationKind::ColorSwatch { rgba },
                role: None,
            });
        }
    }
    out
}

/// End-of-line annotations for a manifest's dependency hints. Fresh
/// dependencies stay quiet; the rest carry a state glyph, the newest
/// version, and an advisory count, role-colored by severity.
pub(super) fn manifest_hint_decorations(hints: &[karet_session::ManifestHint]) -> Vec<Decoration> {
    use karet_session::ManifestHintState;
    hints
        .iter()
        .filter_map(|hint| {
            let (glyph, role) = match hint.state {
                ManifestHintState::UpToDate => return None,
                ManifestHintState::Patch => ("↑", ThemeRole::DiagnosticInfo),
                ManifestHintState::Outdated => ("↑", ThemeRole::DiagnosticWarning),
                ManifestHintState::Vulnerable => ("✗", ThemeRole::DiagnosticError),
                ManifestHintState::Error => ("!", ThemeRole::Muted),
            };
            let mut text = format!("  {glyph}");
            if let Some(latest) = &hint.latest {
                text.push(' ');
                text.push_str(latest);
            }
            if !hint.vulnerabilities.is_empty() {
                text.push_str(&format!("  {} advisories", hint.vulnerabilities.len()));
            }
            Some(Decoration {
                range: karet_core::Range {
                    start: karet_core::LineCol::new(hint.line, 0),
                    end: karet_core::LineCol::new(hint.line, 0),
                },
                kind: karet_core::DecorationKind::InlineText {
                    text,
                    before: false,
                },
                role: Some(role),
            })
        })
        .collect()
}

/// The per-frame decorations content assembles fresh each draw: color
/// swatches over the visible slice, and the manifest's dependency hints
/// (version-guarded).
pub(super) fn frame_decorations(
    ctx: &PaneCtx,
    doc: Option<DocumentId>,
    buffer: &TextBuffer,
    scroll_line: u32,
    area: Rect,
) -> Vec<Decoration> {
    let mut out = if ctx.color_highlight {
        color_swatch_decorations(buffer, scroll_line, area.height)
    } else {
        Vec::new()
    };
    if let Some((checked, hints)) = doc.and_then(|doc| ctx.manifest_hints.get(&doc))
        && *checked == buffer.version()
    {
        out.extend(manifest_hint_decorations(hints));
    }
    out
}
