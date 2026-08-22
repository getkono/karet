use super::*;

/// Swatch decorations for the color literals on the lines a viewport of
/// `height` rows starting at `first_line` can show (doubled for wrap slack).
fn color_swatch_decorations(buffer: &TextBuffer, first_line: u32, height: u16) -> Vec<Decoration> {
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
            let advisories = hint.vulnerabilities.len();
            if advisories > 0 {
                let noun = if advisories == 1 {
                    "advisory"
                } else {
                    "advisories"
                };
                text.push_str(&format!("  {advisories} {noun}"));
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

/// Gutter markers for one file's armed breakpoints (`●` verified in the
/// breakpoint red, `○` not-yet-verified in muted).
pub(super) fn breakpoint_decorations(
    breakpoints: &std::collections::BTreeMap<u32, bool>,
) -> Vec<Decoration> {
    breakpoints
        .iter()
        .map(|(&line, &verified)| Decoration {
            range: karet_core::Range {
                start: karet_core::LineCol::new(line, 0),
                end: karet_core::LineCol::new(line, 0),
            },
            kind: karet_core::DecorationKind::GutterMarker {
                glyph: if verified { '●' } else { '○' },
            },
            role: Some(if verified {
                ThemeRole::Breakpoint
            } else {
                ThemeRole::Muted
            }),
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

#[cfg(test)]
mod manifest_hint_tests {
    use karet_core::DecorationKind;
    use karet_session::ManifestHint;
    use karet_session::ManifestHintState;

    use super::*;

    fn hint(state: ManifestHintState, latest: Option<&str>, advisories: &[&str]) -> ManifestHint {
        ManifestHint {
            name: "time".to_owned(),
            line: 7,
            col_start: 8,
            col_end: 14,
            current: "0.1.44".to_owned(),
            latest: latest.map(str::to_owned),
            state,
            vulnerabilities: advisories.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    /// The ghost text of the first decoration, if there is one.
    fn ghost(decorations: &[Decoration]) -> Option<String> {
        match decorations.first().map(|d| &d.kind) {
            Some(DecorationKind::InlineText { text, before }) => {
                assert!(!before, "a manifest hint trails its line");
                Some(text.clone())
            },
            _ => None,
        }
    }

    #[test]
    fn an_up_to_date_dependency_stays_quiet() {
        let decorations =
            manifest_hint_decorations(&[hint(ManifestHintState::UpToDate, None, &[])]);
        assert!(decorations.is_empty());
    }

    #[test]
    fn an_outdated_dependency_names_the_newest_release() {
        let decorations =
            manifest_hint_decorations(&[hint(ManifestHintState::Outdated, Some("0.3.55"), &[])]);
        assert_eq!(ghost(&decorations).as_deref(), Some("  ↑ 0.3.55"));
        assert_eq!(
            decorations[0].role,
            Some(ThemeRole::DiagnosticWarning),
            "outdated reads as a warning"
        );
    }

    #[test]
    fn one_advisory_is_singular_and_two_are_not() {
        let one = manifest_hint_decorations(&[hint(
            ManifestHintState::Vulnerable,
            Some("0.3.55"),
            &["RUSTSEC-2020-0071"],
        )]);
        assert_eq!(ghost(&one).as_deref(), Some("  ✗ 0.3.55  1 advisory"));

        let two = manifest_hint_decorations(&[hint(
            ManifestHintState::Vulnerable,
            Some("0.3.55"),
            &["RUSTSEC-2020-0071", "RUSTSEC-2020-0159"],
        )]);
        assert_eq!(ghost(&two).as_deref(), Some("  ✗ 0.3.55  2 advisories"));
        assert_eq!(two[0].role, Some(ThemeRole::DiagnosticError));
    }

    #[test]
    fn a_hint_with_no_known_latest_still_marks_the_line() {
        let decorations = manifest_hint_decorations(&[hint(ManifestHintState::Error, None, &[])]);
        assert_eq!(ghost(&decorations).as_deref(), Some("  !"));
        assert_eq!(decorations[0].role, Some(ThemeRole::Muted));
    }

    #[test]
    fn the_decoration_anchors_to_the_hint_s_line() {
        let decorations =
            manifest_hint_decorations(&[hint(ManifestHintState::Patch, Some("0.1.45"), &[])]);
        assert_eq!(decorations[0].range.start.line, 7);
        assert_eq!(decorations[0].role, Some(ThemeRole::DiagnosticInfo));
    }
}
