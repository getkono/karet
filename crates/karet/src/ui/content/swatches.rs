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
        for (range, rgba) in super::color::detect(&text) {
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
/// swatches over the visible slice -- the language server's colours for
/// `doc`, then the literals the built-in detector finds that no server colour
/// already covers, so a colour both know is swatched once.
pub(super) fn frame_decorations(
    ctx: &PaneCtx,
    doc: Option<DocumentId>,
    buffer: &TextBuffer,
    scroll_line: u32,
    area: Rect,
) -> Vec<Decoration> {
    if !ctx.color_highlight {
        return Vec::new();
    }
    let served = doc.map_or(&[][..], |doc| {
        crate::app::current_items(ctx.document_colors, doc, buffer.version())
    });
    let local = color_swatch_decorations(buffer, scroll_line, area.height);
    merge_swatches(served, scroll_line, area.height, local)
}

/// The swatches of `served` on the lines a viewport of `height` rows from
/// `first_line` can show (doubled for wrap slack, as the detector's), then
/// each of `local` that overlaps none of them.
fn merge_swatches(
    served: &[karet_core::DocumentColor],
    first_line: u32,
    height: u16,
    local: Vec<Decoration>,
) -> Vec<Decoration> {
    let last = first_line.saturating_add(u32::from(height) * 2);
    let mut out: Vec<Decoration> = served
        .iter()
        .filter(|colour| colour.range.end.line >= first_line && colour.range.start.line <= last)
        .map(karet_core::DocumentColor::swatch)
        .collect();
    out.extend(
        local
            .into_iter()
            .filter(|swatch| !served.iter().any(|colour| colour.overlaps(swatch.range))),
    );
    out
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
    let mut lines: Vec<_> = rows.into_iter().flatten().map(|row| row.line).collect();
    // Ghost text last of both, so every index above stays what it was: its
    // first line after the caret, its further lines as rows below it.
    if let Some((text, rows)) = ctx.inline_ghost.clone() {
        out.extend(text);
        lines.extend(rows);
    }
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

/// The source text an editor viewport shows: its buffer, the file it is
/// (Markdown or not, and what relative link targets resolve against), and the
/// workspace root that bounds them.
pub(super) struct LinkSource<'a> {
    /// The document's text.
    pub(super) buffer: &'a TextBuffer,
    /// The document's file.
    pub(super) path: &'a std::path::Path,
    /// The workspace root.
    pub(super) root: &'a std::path::Path,
}

/// Paint the links written in the source that the editor just drew in `area`
/// as OSC 8 hyperlinks, so the terminal's own link handling (Cmd+click in
/// most macOS terminals) opens them too.
///
/// Only a target [`crate::links::LinkTarget::osc8_uri`] admits is painted: a
/// link that resolves outside the workspace stays plain, so the confirmation
/// karet's own Ctrl/Cmd+click asks for cannot be bypassed. The scan covers the
/// lines the viewport can show, skipping collapsed folds; each link's cells
/// come from the editor's own geometry, so a link scrolled half out of view,
/// or wrapped across rows, links exactly the cells it occupies.
///
/// Each line's geometry is worked out once ([`EditorState::line_cells`]), and
/// a link with no character on screen is passed over before it is resolved,
/// so a minified line with thousands of links costs what its visible part
/// does.
pub(super) fn source_link_osc8(
    buf: &mut ratatui::buffer::Buffer,
    source: &LinkSource,
    editor: &EditorState,
    folds: &[karet_editor::Fold],
    area: Rect,
) {
    let markdown = crate::app::is_markdown(source.path);
    let mut line = editor.scroll_line();
    let mut shown = 0;
    while shown < area.height {
        if let Some(fold) = folds
            .iter()
            .find(|fold| fold.collapsed && fold.start < line && line <= fold.end)
        {
            line = fold.end.saturating_add(1);
            continue;
        }
        let Some(text) = source.buffer.line(line as usize) else {
            break;
        };
        let links = crate::autolink::source_links(&text, markdown);
        let cells = if links.is_empty() {
            Vec::new()
        } else {
            editor.line_cells(area, source.buffer, folds, line)
        };
        for (slice, first, target) in visible_links(&text, links, &cells) {
            let Ok(target) = crate::links::resolve(&target, source.path, source.root) else {
                continue;
            };
            let Some(uri) = target.osc8_uri().and_then(crate::links::safe_external) else {
                continue;
            };
            for (y, start, end) in link_runs(&cells, slice, first, area.right()) {
                super::super::osc8::link_row(buf, y, start, end, uri);
            }
        }
        shown += 1;
        line = line.saturating_add(1);
    }
}

/// The links of `text` with a character among `cells` (the line's drawn
/// characters, as [`EditorState::line_cells`] reports them), each as its
/// source slice, the column of its first character, and its target.
///
/// Columns are counted in one pass over the line, so the cost is the line's
/// length however many links it holds.
fn visible_links<'t>(
    text: &'t str,
    mut links: Vec<crate::autolink::SourceLink>,
    cells: &[(u32, u16, u16)],
) -> Vec<(&'t str, u32, String)> {
    let (Some(&(lo, ..)), Some(&(hi, ..))) = (cells.first(), cells.last()) else {
        return Vec::new();
    };
    links.sort_by_key(|link| link.range.start);
    let (mut byte, mut col) = (0_usize, 0_u32);
    let mut visible = Vec::new();
    for link in links {
        let (Some(prefix), Some(slice)) = (text.get(byte..link.range.start), text.get(link.range))
        else {
            continue;
        };
        col = col.saturating_add(u32::try_from(prefix.chars().count()).unwrap_or(u32::MAX));
        byte += prefix.len();
        if col > hi {
            break;
        }
        let end = col.saturating_add(u32::try_from(slice.chars().count()).unwrap_or(u32::MAX));
        if end > lo {
            visible.push((slice, col, link.target));
        }
    }
    visible
}

/// The screen runs `(row, first column, end column)` the characters of
/// `slice`, whose first character is column `first`, occupy among `cells`
/// (the line's drawn characters, in column order): one run normally, one per
/// row where the link wraps, and none for what is scrolled out of view. No
/// run reaches past `right`, the editor area's right edge.
fn link_runs(
    cells: &[(u32, u16, u16)],
    slice: &str,
    first: u32,
    right: u16,
) -> Vec<(u16, u16, u16)> {
    use unicode_width::UnicodeWidthChar;
    let width = |ch: char| u16::try_from(ch.width().unwrap_or(1).max(1)).unwrap_or(1);
    let from = cells.partition_point(|&(col, ..)| col < first);
    let mut chars = slice.chars().zip(first..);
    let mut runs: Vec<(u16, u16, u16)> = Vec::new();
    for &(col, x, y) in cells.get(from..).unwrap_or_default() {
        let Some((ch, _)) = chars.find(|&(_, at)| at == col) else {
            break;
        };
        let end = x.saturating_add(width(ch)).min(right);
        match runs.last_mut() {
            Some((row, start, stop)) if *row == y && x >= *start => *stop = (*stop).max(end),
            _ => runs.push((y, x, end)),
        }
    }
    runs
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
mod swatch_tests {
    use karet_core::DocumentColor;
    use karet_core::LineCol;

    use super::*;

    fn span(line: u32, start: u32, end: u32) -> karet_core::Range {
        karet_core::Range {
            start: LineCol::new(line, start),
            end: LineCol::new(line, end),
        }
    }

    fn colour(range: karet_core::Range, rgba: [u8; 4]) -> DocumentColor {
        DocumentColor { range, rgba }
    }

    /// The swatches' spans and colours, in order.
    fn swatches(decorations: &[Decoration]) -> Vec<(karet_core::Range, [u8; 4])> {
        decorations
            .iter()
            .filter_map(|decoration| match decoration.kind {
                karet_core::DecorationKind::ColorSwatch { rgba } => Some((decoration.range, rgba)),
                _ => None,
            })
            .collect()
    }

    /// A literal both the server and the detector know is swatched once, in
    /// the server's colour; the detector's other finds and the server's
    /// colours the detector cannot see (a Tailwind class) are each kept.
    #[test]
    fn a_server_colour_replaces_the_detector_s_swatch_it_overlaps() {
        let buffer = TextBuffer::from_text("a: #ff0000;\nb: #00ff00;\nbg-red-500\n");
        let served = [
            colour(span(0, 3, 10), [250, 0, 0, 255]),
            colour(span(2, 0, 10), [239, 68, 68, 255]),
        ];
        let local = color_swatch_decorations(&buffer, 0, 10);
        let merged = merge_swatches(&served, 0, 10, local);
        assert_eq!(
            swatches(&merged),
            [
                (span(0, 3, 10), [250, 0, 0, 255]),
                (span(2, 0, 10), [239, 68, 68, 255]),
                (span(1, 3, 10), [0, 255, 0, 255]),
            ]
        );
    }

    /// Only the server colours a viewport can show are drawn.
    #[test]
    fn server_colours_off_the_viewport_are_not_drawn() {
        let served = [
            colour(span(0, 0, 1), [1, 2, 3, 255]),
            colour(span(5, 0, 1), [4, 5, 6, 255]),
            colour(span(50, 0, 1), [7, 8, 9, 255]),
        ];
        let merged = merge_swatches(&served, 4, 3, Vec::new());
        assert_eq!(swatches(&merged), [(span(5, 0, 1), [4, 5, 6, 255])]);
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

#[cfg(test)]
mod source_link_tests {
    use super::*;

    #[test]
    fn only_links_with_a_drawn_character_are_kept() {
        let text = "é https://a.io/1 x https://b.io/2 y https://c.io/3";
        assert_eq!(crate::autolink::source_links(text, false).len(), 3);
        // Columns 20..26 are on screen: inside the second URL (19..33) only.
        let cells: Vec<(u32, u16, u16)> = (20..26_u16).map(|x| (u32::from(x), x, 0)).collect();
        let visible = visible_links(text, crate::autolink::source_links(text, false), &cells);
        assert_eq!(
            visible
                .iter()
                .map(|(slice, first, _)| (*slice, *first))
                .collect::<Vec<_>>(),
            [("https://b.io/2", 19)],
            "columns count chars, not bytes"
        );
        assert!(visible_links(text, crate::autolink::source_links(text, false), &[]).is_empty());
    }

    #[test]
    fn a_link_runs_one_row_per_wrap_and_stops_at_the_right_edge() {
        // `abcd` at columns 3..7: two on row 0 from x 10, two on row 1.
        let cells = [
            (2, 9, 0),
            (3, 10, 0),
            (4, 11, 0),
            (5, 0, 1),
            (6, 1, 1),
            (7, 2, 1),
        ];
        assert_eq!(link_runs(&cells, "abcd", 3, 40), [(0, 10, 12), (1, 0, 2)]);
        // A wide last character is clipped to the area.
        assert_eq!(link_runs(&cells[..3], "a\u{4e2d}", 3, 12), [(0, 10, 12)]);
        // Scrolled so only its tail shows.
        assert_eq!(link_runs(&cells[3..], "abcd", 3, 40), [(1, 0, 2)]);
        assert!(link_runs(&cells, "ab", 9, 40).is_empty(), "off screen");
    }
}
