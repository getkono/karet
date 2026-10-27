//! Go to Definition and its siblings: request, correlate, and land the caret.
//!
//! The backend seam already existed — `LspCommand::Definition` in, `Event::Definitions`
//! out — and this module is the client half. Ctrl+click and F12 funnel through the
//! same [`App::request_definition`], so a mouse jump and a keyboard jump behave
//! identically and there is one place where staleness is decided.
//!
//! Go to Declaration, Type Definition and Implementation, and Find All References
//! (`Command::FindLocations` in, `Event::Locations` out) are the same request with
//! a [`LocationKind`], and share every rule here: one pending request, dropped when
//! the user has moved to another view, a jump for one answer and the location
//! picker for several.

use std::collections::HashMap;

use karet_core::DocumentLink;
use karet_core::Location;

use super::*;

/// How long a picker row's source-line preview may run before it is cut.
const PREVIEW_CHARS: usize = 80;

/// The noun a navigation request is named by in its notices: `None` is Go to
/// Definition.
fn navigation_noun(kind: Option<LocationKind>) -> &'static str {
    match kind {
        None => "definition",
        Some(LocationKind::Declaration) => "declaration",
        Some(LocationKind::TypeDefinition) => "type definition",
        Some(LocationKind::Implementation) => "implementation",
        Some(LocationKind::References) => "references",
        Some(_) => "location",
    }
}

/// The title of the picker offered for several answers.
fn navigation_title(kind: Option<LocationKind>) -> &'static str {
    match kind {
        None => "Go to Definition",
        Some(LocationKind::Declaration) => "Go to Declaration",
        Some(LocationKind::TypeDefinition) => "Go to Type Definition",
        Some(LocationKind::Implementation) => "Go to Implementation",
        Some(LocationKind::References) => "References",
        Some(_) => "Go to Location",
    }
}

/// The symbol or source link under a Ctrl-held pointer, to be underlined as
/// navigable.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DefinitionHover {
    pub(crate) view: ViewId,
    pub(crate) range: Range,
}

/// A definition request awaiting its answer.
///
/// `Event::Definitions` carries no document or version, so the request id is the
/// *only* correlation key: a late answer to a superseded request must not evict the
/// record for the live one. The view is remembered so a jump is dropped when the
/// user has moved on, and it survives tab reordering (which an index would not).
#[derive(Clone, Copy, Debug)]
pub(crate) struct PendingDefinition {
    pub(crate) id: RequestId,
    pub(crate) doc: DocumentId,
    pub(crate) view: ViewId,
    /// What was asked: `None` for Go to Definition itself.
    pub(crate) kind: Option<LocationKind>,
}

impl App {
    /// Resolve the definition of the symbol at `pos`, placing the caret there first.
    ///
    /// Setting the caret makes Ctrl+click on a non-symbol degrade to exactly a plain
    /// click, and makes the *clicked* symbol — rather than wherever the caret
    /// happened to be — the position Go Back returns to.
    pub(crate) fn go_to_definition_at(&mut self, pos: LineCol) {
        if let Some(Tab {
            kind: TabKind::Code { buffer, .. },
            editor,
            ..
        }) = self.tabs.get_mut(self.active)
        {
            let buffer = buffer.clone();
            editor.set_caret(&buffer, pos);
        }
        self.request_definition();
    }

    /// Ask the language server for the definition of the symbol at the caret.
    ///
    /// A new request replaces any in flight, so the last gesture wins. There is no
    /// "resolving…" status: nothing on screen changes until the answer lands, and a
    /// spinner for a sub-second round trip is the flicker the UI guidelines warn off.
    pub(crate) fn request_definition(&mut self) {
        self.request_locations(None);
    }

    /// Ask the language server where to navigate from the symbol at the caret:
    /// its definition (`None`), or what `kind` names.
    ///
    /// Every kind shares the one pending slot, so the last navigation gesture
    /// wins whichever it was.
    pub(crate) fn request_locations(&mut self, kind: Option<LocationKind>) {
        let Some((doc, position)) = self.completion_target() else {
            let noun = navigation_title(kind).to_lowercase();
            self.notify(
                Report::Refusal,
                NotificationKind::Lsp,
                format!("{noun}: open a code file first"),
            );
            return;
        };
        let Some(view) = self.tabs.get(self.active).map(|tab| tab.view) else {
            return;
        };
        let command = match kind {
            None => LspCommand::Definition { doc, position },
            Some(kind) => LspCommand::FindLocations {
                doc,
                position,
                kind,
            },
        };
        self.pending_definition = self.send(command).map(|id| PendingDefinition {
            id,
            doc,
            view,
            kind,
        });
    }

    /// Adopt a definition answer: jump to it, or say why there is nothing to jump to.
    pub(crate) fn on_definitions(&mut self, id: Option<RequestId>, locations: Vec<Location>) {
        self.on_locations(id, None, locations);
    }

    /// Adopt a navigation answer: jump to a lone location, offer the picker for
    /// several, or say why there is nothing to jump to.
    ///
    /// References always open the picker, even for one: the user asked *where
    /// the symbol is used*, and a list of one answers that where a silent jump
    /// (often onto the declaration under the caret) would not.
    pub(crate) fn on_locations(
        &mut self,
        id: Option<RequestId>,
        kind: Option<LocationKind>,
        locations: Vec<Location>,
    ) {
        let Some(pending) = self.pending_definition else {
            return; // unsolicited
        };
        if id != Some(pending.id) || kind != pending.kind {
            return; // a superseded request answering late; the live one still stands
        }
        self.pending_definition = None;
        // The user asked explicitly, so a moved *caret* is no reason to drop the
        // jump — but a different view or document means they have moved on.
        let current = self.tabs.get(self.active);
        let same_view = current.is_some_and(|tab| tab.view == pending.view);
        let same_doc = current.is_some_and(
            |tab| matches!(tab.kind, TabKind::Code { doc: Some(doc), .. } if doc == pending.doc),
        );
        if !same_view || !same_doc {
            return;
        }
        let references = kind == Some(LocationKind::References);
        match locations.split_first() {
            None => {
                let reason = self.no_definition_reason(kind);
                self.notify(Report::Refusal, NotificationKind::Lsp, reason);
            },
            // One answer never costs a keystroke to accept.
            Some((only, [])) if !references => {
                let (path, position) = (only.path.clone(), only.range.start);
                self.jump_to_location(&path, position);
            },
            Some(_) => {
                let previews = self.location_previews(&locations);
                let entries = locations.into_iter().zip(previews).collect();
                let title = navigation_title(kind);
                self.overlay = Some(Overlay::locations(title, &self.root.clone(), entries));
            },
        }
    }

    /// Why an empty answer came back, phrased for the refusal it becomes.
    ///
    /// A server that is still starting answers instantly with nothing, which would
    /// otherwise read as "this symbol has no definition".
    fn no_definition_reason(&self, kind: Option<LocationKind>) -> String {
        match self.active_language_server_badge().map(|badge| badge.state) {
            None
            | Some(
                LanguageServerBadge::Off
                | LanguageServerBadge::NotInstalled
                | LanguageServerBadge::NeedsSetup,
            ) => "no language server for this file".to_string(),
            Some(LanguageServerBadge::Failed) => {
                "the language server for this file is not running".to_string()
            },
            Some(LanguageServerBadge::Starting | LanguageServerBadge::Retrying) => {
                "language server is still starting".to_string()
            },
            _ => format!("no {} found", navigation_noun(kind)),
        }
    }

    /// The trimmed source line each location points at, for its picker row.
    ///
    /// An open tab's buffer is read first, so unsaved edits are what the row
    /// shows; any other file is read from disk once, however many locations it
    /// holds. A line that cannot be read previews as nothing rather than
    /// failing the picker: the location is still worth offering.
    fn location_previews(&self, locations: &[Location]) -> Vec<String> {
        let mut files: HashMap<&Path, Option<Vec<String>>> = HashMap::new();
        locations
            .iter()
            .map(|location| {
                let line = location.range.start.line as usize;
                let open = self.tabs.iter().find_map(|tab| match &tab.kind {
                    TabKind::Code { path, buffer, .. } if *path == location.path => {
                        Some(buffer.line(line))
                    },
                    _ => None,
                });
                let text = open.unwrap_or_else(|| {
                    let lines = files.entry(location.path.as_path()).or_insert_with(|| {
                        let text = std::fs::read_to_string(&location.path).ok()?;
                        Some(text.lines().map(str::to_string).collect())
                    });
                    lines.as_ref().and_then(|lines| lines.get(line).cloned())
                });
                let text = text.unwrap_or_default();
                text.trim().chars().take(PREVIEW_CHARS).collect()
            })
            .collect()
    }
}

impl App {
    /// Re-evaluate which symbol, if any, the pointer is Ctrl-hovering.
    ///
    /// Called for *every* mouse event, not only motion, so a drag, a click or a
    /// wheel tick all clear a stale underline. It also runs before the pointer-shape
    /// hint, which reads this state — updating it afterwards would leave the shape
    /// one event behind.
    ///
    /// Known limitation: releasing Ctrl without moving the pointer produces no event
    /// at all (the terminal is not asked to report key releases), so the underline
    /// survives until the next mouse event or key press. Terminal editors generally
    /// share this.
    pub(crate) fn update_definition_hover(&mut self, mouse: &MouseEvent) {
        self.definition_hover = self.hovered_definition_symbol(mouse);
    }

    /// What a Ctrl/Cmd+click at the pointer would follow, to be underlined: a
    /// link written in the source, or else a symbol a click could actually
    /// resolve.
    fn hovered_definition_symbol(&self, mouse: &MouseEvent) -> Option<DefinitionHover> {
        if !mouse
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
        {
            return None;
        }
        let point = (mouse.column, mouse.row);
        let area = self.focused_editor_area(point)?;
        let tab = self.tabs.get(self.active)?;
        // A link needs no language server: the click opens it directly. Alt or
        // Shift hand the click back to the editor, so they promise no link.
        let plain = !mouse
            .modifiers
            .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT);
        // A language server's link first, as a click follows it first.
        if let Some((_, link)) = self.server_link_in(tab, area, point).filter(|_| plain) {
            return Some(DefinitionHover {
                view: tab.view,
                range: link.range,
            });
        }
        if let Some((range, _)) = self.source_link_under(area, point).filter(|_| plain) {
            return Some(DefinitionHover {
                view: tab.view,
                range,
            });
        }
        // Promise only what a click can deliver: this is the same condition under
        // which the backend finds a server to ask, so no probe request is needed.
        if self.active_language_server_badge().map(|badge| badge.state)
            != Some(LanguageServerBadge::Ready)
        {
            return None;
        }
        if !matches!(tab.kind, TabKind::Code { doc: Some(_), .. }) {
            return None;
        }
        let (_, range) = self.identifier_under(area, point)?;
        Some(DefinitionHover {
            view: tab.view,
            range,
        })
    }

    /// The editor viewport of the focused pane, when `point` is inside it.
    ///
    /// The Ctrl-hover underline is drawn only here: it decorates the active
    /// tab, which is the focused pane's. A source-link click reads any split
    /// (see [`Self::pane_source_link`]).
    pub(crate) fn focused_editor_area(&self, point: (u16, u16)) -> Option<Rect> {
        let frame = self
            .pane_frames
            .iter()
            .find(|frame| rect_contains(frame.content_rect, point))?;
        (frame.pane == self.focus_pane() && rect_contains(frame.editor_rect, point))
            .then_some(frame.editor_rect)
    }

    /// The link written in the active code tab's source under the pointer at
    /// `point`, drawn in the editor viewport `area`: its range, in characters,
    /// and its target as written.
    ///
    /// A bare URL links in any file; a Markdown file adds its inline
    /// `[text](target)` links (see [`crate::autolink::source_links`]).
    pub(crate) fn source_link_under(
        &self,
        area: Rect,
        point: (u16, u16),
    ) -> Option<(Range, String)> {
        source_link_in(self.tabs.get(self.active)?, area, point)
    }

    /// The target of the source link under the pointer at `point` in whichever
    /// split's editor it is over, focused or not.
    ///
    /// A click there focuses that split before it does anything else, so the
    /// link is read from the tab that split shows rather than from the focused
    /// one: otherwise a Ctrl/Cmd+click on a link in a background split would
    /// fall through to Go to Definition.
    pub(crate) fn pane_source_link(&self, point: (u16, u16)) -> Option<String> {
        let (tab, area) = self.pane_tab_at(point)?;
        source_link_in(tab, area, point).map(|(_, target)| target)
    }

    /// The language server's link under the pointer at `point` in whichever
    /// split's editor it is over, focused or not, with the document it was
    /// listed for (see [`Self::pane_source_link`] for why any split).
    pub(crate) fn pane_server_link(&self, point: (u16, u16)) -> Option<(DocumentId, DocumentLink)> {
        let (tab, area) = self.pane_tab_at(point)?;
        self.server_link_in(tab, area, point)
            .map(|(doc, link)| (doc, link.clone()))
    }

    /// The front tab of the split whose editor is under `point`, and that
    /// editor's viewport.
    fn pane_tab_at(&self, point: (u16, u16)) -> Option<(&Tab, Rect)> {
        let frame = self
            .pane_frames
            .iter()
            .find(|frame| rect_contains(frame.content_rect, point))?;
        if !rect_contains(frame.editor_rect, point) {
            return None;
        }
        let tab = if frame.pane == self.focus_pane() {
            self.tabs.get(self.active)?
        } else {
            let stored = self.stored.get(&frame.pane)?;
            stored.tabs.get(stored.active)?
        };
        Some((tab, frame.editor_rect))
    }

    /// The language server's link under the pointer at `point` in code tab
    /// `tab`, drawn in the editor viewport `area`, with its document.
    fn server_link_in(
        &self,
        tab: &Tab,
        area: Rect,
        point: (u16, u16),
    ) -> Option<(DocumentId, &DocumentLink)> {
        let pos = char_in(tab, area, point)?;
        let TabKind::Code {
            doc: Some(doc),
            buffer,
            ..
        } = &tab.kind
        else {
            return None;
        };
        self.server_link_at(*doc, buffer.version(), pos)
            .map(|link| (*doc, link))
    }

    /// The target of the link written in the active code tab's source under
    /// the primary caret, as written. A caret just past a link's last
    /// character still touches it, so the caret a typed or pasted URL leaves
    /// behind opens that URL.
    pub(crate) fn source_link_at_caret(&self) -> Option<String> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code { buffer, path, .. } = &tab.kind else {
            return None;
        };
        let pos = tab.editor.cursor();
        let line = buffer.line(pos.line as usize)?;
        let byte = line
            .char_indices()
            .nth(pos.col as usize)
            .map_or(line.len(), |(byte, _)| byte);
        let links = crate::autolink::source_links(&line, is_markdown(path));
        // A link the caret is inside wins over one it merely ends against.
        let inside = links.iter().position(|link| link.range.contains(&byte));
        let index = inside.or_else(|| links.iter().position(|link| link.range.end == byte))?;
        links.into_iter().nth(index).map(|link| link.target)
    }

    /// The language server's link under the primary caret of the active code
    /// tab, with its document: only one the caret is on, so a source link the
    /// caret is in is never passed over for a server link it merely ends
    /// against.
    fn server_link_at_caret(&self) -> Option<(DocumentId, DocumentLink)> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code {
            doc: Some(doc),
            buffer,
            ..
        } = &tab.kind
        else {
            return None;
        };
        self.server_link_at(*doc, buffer.version(), tab.editor.cursor())
            .map(|link| (*doc, link.clone()))
    }

    /// Open the link under the primary caret as Ctrl/Cmd+click on it would
    /// (see [`Self::activate_link`]): the keyboard path for that gesture. A
    /// language server's link outranks one read from the source, as it does
    /// under the pointer.
    pub(crate) fn open_link_at_caret(&mut self) {
        if let Some((doc, link)) = self.server_link_at_caret() {
            self.follow_document_link(doc, link);
            return;
        }
        let Some(target) = self.source_link_at_caret() else {
            self.notify(
                Report::Refusal,
                NotificationKind::System,
                "open link: no link at the caret",
            );
            return;
        };
        // Only a code tab yields a link, and a code tab always has the file a
        // relative target resolves against, so activation always answers.
        self.activate_link(&target);
    }

    /// The identifier the pointer at `point` is on in the active code tab, drawn
    /// in the editor viewport `area`: the position under the pointer and the
    /// identifier's range. `None` over whitespace, punctuation, the gutter, past
    /// the end of a line, or below the last one, where Ctrl+click has nothing
    /// to resolve.
    pub(crate) fn identifier_under(
        &self,
        area: Rect,
        point: (u16, u16),
    ) -> Option<(LineCol, Range)> {
        let pos = self.char_under(area, point)?;
        let TabKind::Code { buffer, .. } = &self.tabs.get(self.active)?.kind else {
            return None;
        };
        let (start, end) = karet_editor::identifier_at(buffer, pos)?;
        Some((pos, Range { start, end }))
    }

    /// The character the pointer at `point` is on in the active code tab, drawn
    /// in the editor viewport `area`, or `None` when the pointer is on no
    /// character: in the gutter, past the end of a line, below the last line.
    pub(crate) fn char_under(&self, area: Rect, point: (u16, u16)) -> Option<LineCol> {
        char_in(self.tabs.get(self.active)?, area, point)
    }

    /// The underline decoration for the Ctrl-hovered symbol or source link in
    /// the active tab.
    pub(crate) fn definition_underline_decoration(&self) -> Option<Decoration> {
        let hover = self.definition_hover?;
        let tab = self.tabs.get(self.active)?;
        // No role: the underline takes the token's own foreground, so it can
        // never come out illegible in a theme that never anticipated it.
        (tab.view == hover.view).then_some(Decoration::new(
            hover.range,
            DecorationKind::Underline,
            None,
        ))
    }
}

/// The link written in code tab `tab`'s source under the pointer at `point`,
/// drawn in the editor viewport `area`: its range, in characters, and its
/// target as written.
fn source_link_in(tab: &Tab, area: Rect, point: (u16, u16)) -> Option<(Range, String)> {
    let pos = char_in(tab, area, point)?;
    let TabKind::Code { buffer, path, .. } = &tab.kind else {
        return None;
    };
    let line = buffer.line(pos.line as usize)?;
    let (byte, _) = line.char_indices().nth(pos.col as usize)?;
    let link = crate::autolink::source_links(&line, is_markdown(path))
        .into_iter()
        .find(|link| link.range.contains(&byte))?;
    let col = |byte: usize| u32::try_from(line[..byte].chars().count()).ok();
    let range = Range {
        start: LineCol::new(pos.line, col(link.range.start)?),
        end: LineCol::new(pos.line, col(link.range.end)?),
    };
    Some((range, link.target))
}

/// The character the pointer at `point` is on in code tab `tab`, drawn in the
/// editor viewport `area` (see [`App::char_under`]).
fn char_in(tab: &Tab, area: Rect, point: (u16, u16)) -> Option<LineCol> {
    let TabKind::Code {
        buffer,
        folds,
        folded,
        ..
    } = &tab.kind
    else {
        return None;
    };
    let fold_lines = resolve_folds(folds, folded);
    let pos = tab
        .editor
        .char_pos_at(area, buffer, &fold_lines, point.0, point.1);
    // Round-trip guard: `char_pos_at` clamps a point in the gutter, past the end of a
    // line, or in a reserved scrollbar track onto a real position, which would
    // name a word the pointer is not actually over. Asking which cells that
    // character *renders* in — from its own cell up to the next character's —
    // rejects all of those at once, without this code needing to know the
    // geometry, and still accepts the right half of a wide character.
    let cell = tab.editor.screen_cell(area, buffer, &fold_lines, pos)?;
    let next = LineCol::new(pos.line, pos.col.saturating_add(1));
    let next = tab.editor.screen_cell(area, buffer, &fold_lines, next);
    let on_it = point.1 == cell.1
        && point.0 >= cell.0
        && next.is_none_or(|next| next.1 != cell.1 || point.0 < next.0);
    // The cell right after a line's last character maps to the line end,
    // which is no character.
    let len = buffer
        .line(pos.line as usize)
        .map_or(0, |line| line.chars().count());
    (on_it && (pos.col as usize) < len).then_some(pos)
}
