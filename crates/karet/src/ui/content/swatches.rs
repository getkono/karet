use karet_core::VirtualText;

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
            out.push(Decoration::new(
                karet_core::Range {
                    start: karet_core::LineCol::new(line, u32::try_from(range.start).unwrap_or(0)),
                    end: karet_core::LineCol::new(
                        line,
                        u32::try_from(range.end).unwrap_or(u32::MAX),
                    ),
                },
                karet_core::DecorationKind::ColorSwatch { rgba },
                None,
            ));
        }
    }
    out
}

/// The end-of-line priority of a dependency hint: above inline blame, so on a
/// short line it is the blame that is cut, never the call to act.
const MANIFEST_EOL_PRIORITY: u8 = crate::app::LiveBlame::BLAME_EOL_PRIORITY + 1;

/// The end-of-line priority of a diagnostic message: above a dependency hint
/// and inline blame, so a short line keeps the problem and cuts the rest.
const DIAGNOSTIC_EOL_PRIORITY: u8 = MANIFEST_EOL_PRIORITY + 1;

/// The diagnostics published for `doc`, or none.
pub(super) fn doc_diagnostics<'a>(ctx: &PaneCtx<'a>, doc: Option<DocumentId>) -> &'a [Diagnostic] {
    doc.and_then(|doc| ctx.diagnostics.get(&doc))
        .map_or(&[][..], Vec::as_slice)
}

/// The `diagnostics` that earn a gutter sign and a caret-line message: the
/// problems, as the Problems panel counts them. Misspellings keep their
/// underline but would otherwise fill the marker lane, and take the
/// end-of-line slot from blame, on every line of prose.
pub(super) fn problems(diagnostics: &[Diagnostic]) -> Vec<Diagnostic> {
    diagnostics
        .iter()
        .filter(|diagnostic| crate::app::is_problem(diagnostic))
        .cloned()
        .collect()
}

/// The end-of-line messages of `diagnostics` on the lines `inline` selects
/// (`mode`, with the `caret_lines`), at least as severe as its threshold
/// (`editor.inlineDiagnostics`).
pub(super) fn diagnostic_text(
    diagnostics: &[Diagnostic],
    inline: (karet_editor::InlineDiagnostics, karet_core::Severity),
    caret_lines: &[u32],
) -> Vec<VirtualText> {
    let (mode, min_severity) = inline;
    karet_editor::diagnostic_messages(
        diagnostics,
        mode,
        min_severity,
        caret_lines,
        DIAGNOSTIC_EOL_PRIORITY,
    )
}

/// The lines the carets of `cursors` are on.
fn caret_lines(cursors: &karet_core::CursorState) -> Vec<u32> {
    cursors.selections.iter().map(|s| s.head.line).collect()
}

/// The merged pane of a merge conflict's decorations and virtual text: its
/// `decorations` followed by the gutter signs of the problems among
/// `diagnostics` (after, so a marker already on a line keeps the lane, as in
/// the plain editor), and those problems' messages on the lines `inline`
/// (`editor.inlineDiagnostics`) selects.
pub(super) fn merged_pane_diagnostics(
    inline: (karet_editor::InlineDiagnostics, karet_core::Severity),
    decorations: &[Decoration],
    diagnostics: &[Diagnostic],
    cursors: &karet_core::CursorState,
) -> (Vec<Decoration>, Vec<VirtualText>) {
    let problems = problems(diagnostics);
    let mut out = decorations.to_vec();
    out.extend(karet_editor::diagnostic_signs(&problems));
    let messages = diagnostic_text(&problems, inline, &caret_lines(cursors));
    (out, messages)
}

/// End-of-line annotations for a manifest's dependency hints. Fresh
/// dependencies stay quiet; the rest carry a state glyph, the newest
/// version, and an advisory count, role-colored by severity.
pub(super) fn manifest_hint_text(hints: &[karet_session::ManifestHint]) -> Vec<VirtualText> {
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
            Some(VirtualText::end_of_line(
                hint.line,
                text,
                role,
                MANIFEST_EOL_PRIORITY,
            ))
        })
        .collect()
}

/// Gutter markers for one file's armed breakpoints (`●` verified in the
/// breakpoint red, `○` not-yet-verified in muted).
fn breakpoint_decorations(breakpoints: &std::collections::BTreeMap<u32, bool>) -> Vec<Decoration> {
    breakpoints
        .iter()
        .map(|(&line, &verified)| {
            Decoration::new(
                karet_core::Range {
                    start: karet_core::LineCol::new(line, 0),
                    end: karet_core::LineCol::new(line, 0),
                },
                karet_core::DecorationKind::GutterMarker {
                    glyph: if verified { '●' } else { '○' },
                },
                Some(if verified {
                    ThemeRole::Breakpoint
                } else {
                    ThemeRole::Muted
                }),
            )
        })
        .collect()
}

/// The debugger's per-file decorations: gutter breakpoint markers and the
/// stopped-line tint.
pub(super) fn debug_decorations(ctx: &PaneCtx, path: &std::path::Path) -> Vec<Decoration> {
    let mut out = ctx
        .breakpoints
        .get(path)
        .map(breakpoint_decorations)
        .unwrap_or_default();
    if let Some((stop_path, stop_line)) = ctx.debug_stopped
        && stop_path == path
    {
        out.push(Decoration::new(
            karet_core::Range {
                start: karet_core::LineCol::new(*stop_line, 0),
                end: karet_core::LineCol::new(*stop_line, 0),
            },
            karet_core::DecorationKind::LineBackground,
            Some(ThemeRole::DebugStoppedLine),
        ));
    }
    out
}

/// The per-frame decorations content assembles fresh each draw: color
/// swatches over the visible slice.
pub(super) fn frame_decorations(
    ctx: &PaneCtx,
    buffer: &TextBuffer,
    scroll_line: u32,
    area: Rect,
) -> Vec<Decoration> {
    if ctx.color_highlight {
        color_swatch_decorations(buffer, scroll_line, area.height)
    } else {
        Vec::new()
    }
}

/// The virtual text content assembles fresh each draw, in one slice: the
/// document's inlay hints (one text per label part, so a part can be hovered
/// and followed on its own), the messages of the `diagnostics` on the
/// `caret_lines`, the manifest's dependency hints (version-guarded), and the
/// inline blame last.
///
/// Returns the slice; which `(hint, part)` each of its first texts is, in
/// order, one per inlay-hint label part; and the blame's index in the slice.
/// The last two are how what was painted is found again for its pointer
/// targets. Beside them, the document's code lenses as the rows the editor
/// draws above their lines.
pub(super) fn frame_virtual_text(
    ctx: &PaneCtx,
    doc: Option<DocumentId>,
    buffer: &TextBuffer,
    (diagnostics, cursors): (&[Diagnostic], &karet_core::CursorState),
) -> FrameVirtual {
    let mut out: Vec<VirtualText> = Vec::new();
    let mut inlay_parts = Vec::new();
    for (index, hint) in doc
        .and_then(|doc| ctx.inlay_hints.get(&doc))
        .into_iter()
        .flatten()
        .enumerate()
    {
        for (part, text) in hint.virtual_texts().into_iter().enumerate() {
            out.push(text);
            inlay_parts.push((index, part));
        }
    }
    out.extend(diagnostic_text(
        diagnostics,
        ctx.display.inline_diagnostics(),
        &caret_lines(cursors),
    ));
    if let Some((checked, hints)) = doc.and_then(|doc| ctx.manifest_hints.get(&doc))
        && *checked == buffer.version()
    {
        out.extend(manifest_hint_text(hints));
    }
    let blame = ctx.blame.clone().map(|blame| {
        out.push(blame);
        out.len() - 1
    });
    // The lens rows are the frame's only virtual lines, in the order
    // `App::handle_code_lens_mouse` rebuilds them to find a click's lens.
    let lenses = doc.and_then(|doc| ctx.code_lenses.get(&doc));
    let rows = lenses.map(|lenses| crate::app::code_lens_rows(lenses, buffer, ctx.tab_width));
    let lines = rows.into_iter().flatten().map(|row| row.line).collect();
    (out, inlay_parts, blame, lines)
}

/// What [`frame_virtual_text`] returns: the virtual text; which `(hint,
/// part)` each of its first texts is; the blame's index in it; and the code
/// lens rows to draw as virtual lines.
pub(super) type FrameVirtual = (
    Vec<VirtualText>,
    Vec<(usize, usize)>,
    Option<usize>,
    Vec<karet_core::VirtualLine>,
);

/// Where the editor painted each inlay-hint label part of `doc`, given which
/// `(hint, part)` each of the first virtual texts was.
pub(super) fn inlay_hits(
    inlay_parts: &[(usize, usize)],
    doc: Option<DocumentId>,
    editor: &karet_editor::EditorState,
) -> Vec<crate::app::InlayHintHit> {
    let Some(doc) = doc else {
        return Vec::new();
    };
    inlay_parts
        .iter()
        .enumerate()
        .filter_map(|(index, &(hint, part))| {
            Some(crate::app::InlayHintHit {
                rect: editor.virtual_text_rect(index)?,
                doc,
                hint,
                part,
            })
        })
        .collect()
}

#[cfg(test)]
mod diagnostic_text_tests {
    use karet_core::EolAlign;
    use karet_core::LineCol;
    use karet_core::Severity;
    use karet_core::VirtualTextPlacement;

    use super::*;

    fn diagnostic(line: u32, severity: Severity, message: &str) -> Diagnostic {
        Diagnostic {
            range: karet_core::Range {
                start: LineCol::new(line, 0),
                end: LineCol::new(line, 1),
            },
            severity,
            message: message.to_owned(),
            source: None,
            code: None,
            code_href: None,
            tags: Vec::new(),
            related: Vec::new(),
        }
    }

    /// The default `editor.inlineDiagnostics`: the caret lines, every severity.
    const DEFAULT: (karet_editor::InlineDiagnostics, Severity) =
        (karet_editor::InlineDiagnostics::CurrentLine, Severity::Hint);

    #[test]
    fn by_default_only_the_caret_lines_show_messages_of_every_severity() {
        let all = [
            diagnostic(0, Severity::Error, "off the caret"),
            diagnostic(3, Severity::Hint, "a hint"),
            diagnostic(3, Severity::Error, "an error"),
        ];
        let texts: Vec<(u32, String)> = diagnostic_text(&all, DEFAULT, &[3])
            .into_iter()
            .map(|text| (text.line, text.text))
            .collect();
        assert_eq!(
            texts,
            [(3, "  an error".to_owned()), (3, "  a hint".to_owned())]
        );
    }

    #[test]
    fn the_setting_chooses_the_lines_and_the_threshold() {
        let all = [
            diagnostic(0, Severity::Error, "off the caret"),
            diagnostic(3, Severity::Hint, "a hint"),
        ];
        let lines = |inline| -> Vec<u32> {
            diagnostic_text(&all, inline, &[3])
                .into_iter()
                .map(|text| text.line)
                .collect()
        };
        use karet_editor::InlineDiagnostics;
        assert_eq!(lines((InlineDiagnostics::All, Severity::Hint)), [0, 3]);
        assert_eq!(lines((InlineDiagnostics::All, Severity::Warning)), [0]);
        assert!(lines((InlineDiagnostics::Off, Severity::Hint)).is_empty());
    }

    #[test]
    fn misspellings_get_no_sign_and_no_caret_line_message() {
        let mut misspelled = diagnostic(2, Severity::Warning, "Unknown word");
        misspelled.source = Some("karet-spell".to_owned());
        let all = [misspelled, diagnostic(2, Severity::Hint, "a hint")];
        let kept = problems(&all);
        let messages: Vec<String> = diagnostic_text(&kept, DEFAULT, &[2])
            .into_iter()
            .map(|text| text.text)
            .collect();
        assert_eq!(messages, ["  a hint".to_owned()]);
        let signs = karet_editor::diagnostic_signs(&kept);
        assert_eq!(signs.len(), 1);
        assert_eq!(
            signs.first().and_then(|sign| sign.role),
            Some(ThemeRole::DiagnosticHint)
        );
    }

    #[test]
    fn the_merged_pane_signs_its_problems_after_its_markers_and_shows_caret_messages() {
        let marker = Decoration::new(
            karet_core::Range {
                start: LineCol::new(1, 0),
                end: LineCol::new(1, 0),
            },
            karet_core::DecorationKind::GutterMarker { glyph: '│' },
            Some(ThemeRole::Muted),
        );
        let mut misspelled = diagnostic(1, Severity::Warning, "Unknown word");
        misspelled.source = Some("karet-spell".to_owned());
        let all = [
            diagnostic(1, Severity::Error, "on the caret"),
            diagnostic(4, Severity::Warning, "off it"),
            misspelled,
        ];
        let cursors =
            karet_core::CursorState::single(karet_core::Selection::caret(LineCol::new(1, 2)));
        let (decorations, messages) =
            merged_pane_diagnostics(DEFAULT, std::slice::from_ref(&marker), &all, &cursors);
        let lanes: Vec<(u32, Option<ThemeRole>)> = decorations
            .iter()
            .map(|decoration| (decoration.range.start.line, decoration.role))
            .collect();
        assert_eq!(
            lanes,
            [
                (1, Some(ThemeRole::Muted)),
                (1, Some(ThemeRole::DiagnosticError)),
                (4, Some(ThemeRole::DiagnosticWarning)),
            ],
            "the existing marker first; the misspelling earns no sign"
        );
        let messages: Vec<(u32, String)> = messages
            .into_iter()
            .map(|text| (text.line, text.text))
            .collect();
        assert_eq!(messages, [(1, "  on the caret".to_owned())]);
    }

    #[test]
    fn a_message_outranks_a_dependency_hint_and_the_blame() {
        let texts = diagnostic_text(&[diagnostic(0, Severity::Warning, "w")], DEFAULT, &[0]);
        let Some(text) = texts.first() else {
            panic!("the caret line's warning is shown");
        };
        assert_eq!(
            text.placement,
            VirtualTextPlacement::EndOfLine {
                align: EolAlign::Left,
                priority: DIAGNOSTIC_EOL_PRIORITY,
            }
        );
        const {
            assert!(DIAGNOSTIC_EOL_PRIORITY > MANIFEST_EOL_PRIORITY);
            assert!(DIAGNOSTIC_EOL_PRIORITY > crate::app::LiveBlame::BLAME_EOL_PRIORITY);
        }
    }
}

#[cfg(test)]
mod manifest_hint_tests {
    use karet_core::EolAlign;
    use karet_core::VirtualTextPlacement;
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

    /// The text of the first hint, if there is one.
    fn ghost(texts: &[VirtualText]) -> Option<String> {
        let text = texts.first()?;
        assert_eq!(
            text.placement,
            VirtualTextPlacement::EndOfLine {
                align: EolAlign::Left,
                priority: MANIFEST_EOL_PRIORITY,
            },
            "a manifest hint trails its line"
        );
        Some(text.text.clone())
    }

    #[test]
    fn an_up_to_date_dependency_stays_quiet() {
        let decorations = manifest_hint_text(&[hint(ManifestHintState::UpToDate, None, &[])]);
        assert!(decorations.is_empty());
    }

    #[test]
    fn an_outdated_dependency_names_the_newest_release() {
        let decorations =
            manifest_hint_text(&[hint(ManifestHintState::Outdated, Some("0.3.55"), &[])]);
        assert_eq!(ghost(&decorations).as_deref(), Some("  ↑ 0.3.55"));
        assert_eq!(
            decorations[0].role,
            ThemeRole::DiagnosticWarning,
            "outdated reads as a warning"
        );
    }

    #[test]
    fn one_advisory_is_singular_and_two_are_not() {
        let one = manifest_hint_text(&[hint(
            ManifestHintState::Vulnerable,
            Some("0.3.55"),
            &["RUSTSEC-2020-0071"],
        )]);
        assert_eq!(ghost(&one).as_deref(), Some("  ✗ 0.3.55  1 advisory"));

        let two = manifest_hint_text(&[hint(
            ManifestHintState::Vulnerable,
            Some("0.3.55"),
            &["RUSTSEC-2020-0071", "RUSTSEC-2020-0159"],
        )]);
        assert_eq!(ghost(&two).as_deref(), Some("  ✗ 0.3.55  2 advisories"));
        assert_eq!(two[0].role, ThemeRole::DiagnosticError);
    }

    #[test]
    fn a_hint_with_no_known_latest_still_marks_the_line() {
        let decorations = manifest_hint_text(&[hint(ManifestHintState::Error, None, &[])]);
        assert_eq!(ghost(&decorations).as_deref(), Some("  !"));
        assert_eq!(decorations[0].role, ThemeRole::Muted);
    }

    #[test]
    fn the_text_anchors_to_the_hint_s_line() {
        let decorations =
            manifest_hint_text(&[hint(ManifestHintState::Patch, Some("0.1.45"), &[])]);
        assert_eq!(decorations[0].line, 7);
        assert_eq!(decorations[0].role, ThemeRole::DiagnosticInfo);
    }
}
