//! Go to Definition: request, correlate, and land the caret.
//!
//! The backend seam already existed — `Command::Definition` in, `Event::Definitions`
//! out — and this module is the client half. Ctrl+click and F12 funnel through the
//! same [`App::request_definition`], so a mouse jump and a keyboard jump behave
//! identically and there is one place where staleness is decided.

use karet_core::Location;

use super::*;

/// How many jump origins to keep. Deep enough to walk back through a chain of
/// definitions; bounded so a long session cannot grow it without limit.
const MAX_JUMP_HISTORY: usize = 64;

/// Where a definition jump started, so "Go Back" can return there.
///
/// The position is stored as a path rather than a [`ViewId`] on purpose: a view id
/// dies with its tab, while a path can simply be reopened.
#[derive(Clone, Debug)]
pub(crate) struct JumpOrigin {
    pub(crate) pane: PaneId,
    pub(crate) path: PathBuf,
    pub(crate) position: LineCol,
}

/// The symbol under a Ctrl-held pointer, to be underlined as navigable.
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
        let Some((doc, position)) = self.completion_target() else {
            self.status = Some("go to definition: open a code file first".to_string());
            return;
        };
        let Some(view) = self.tabs.get(self.active).map(|tab| tab.view) else {
            return;
        };
        self.pending_definition = self
            .send(SessionCommand::Definition { doc, position })
            .map(|id| PendingDefinition { id, doc, view });
    }

    /// Adopt a definition answer: jump to it, or say why there is nothing to jump to.
    pub(crate) fn on_definitions(&mut self, id: Option<RequestId>, locations: Vec<Location>) {
        let Some(pending) = self.pending_definition else {
            return; // unsolicited
        };
        if id != Some(pending.id) {
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
        match locations.split_first() {
            None => self.status = Some(self.no_definition_reason().to_string()),
            // One answer never costs a keystroke to accept.
            Some((only, [])) => {
                let (path, position) = (only.path.clone(), only.range.start);
                self.jump_to_location(&path, position);
            },
            Some(_) => self.overlay = Some(Overlay::definitions(&self.root.clone(), locations)),
        }
    }

    /// Why an empty answer came back, phrased for the status line.
    ///
    /// A server that is still starting answers instantly with nothing, which would
    /// otherwise read as "this symbol has no definition".
    fn no_definition_reason(&self) -> &'static str {
        match self.active_language_server_badge() {
            None | Some(LanguageServerBadge::Unavailable) => "no language server for this file",
            Some(LanguageServerBadge::Starting | LanguageServerBadge::Retrying) => {
                "language server is still starting"
            },
            _ => "no definition found",
        }
    }

    /// Move to `position` in `path`, recording where the jump started.
    ///
    /// The caret lands on the definition's start, collapsed to a bare caret with no
    /// selection. For a server that answers with a `LocationLink` that is the first
    /// character of the definition's *name*; a plain `Location` is whatever the
    /// server calls the start, often the `pub`/`fn` before it.
    pub(crate) fn jump_to_location(&mut self, path: &Path, position: LineCol) {
        self.push_jump_origin(path, position);
        self.focus_by_file_line(path, position);
    }

    /// Remember where a jump started, unless it goes nowhere.
    ///
    /// Pushing here rather than at the request means a dismissed picker leaves no
    /// entry behind.
    fn push_jump_origin(&mut self, target: &Path, position: LineCol) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let Some(path) = tab.path().map(Path::to_path_buf) else {
            return;
        };
        let origin = JumpOrigin {
            pane: self.focus_pane(),
            position: tab.editor.cursor(),
            path,
        };
        // A jump that lands on the line it started from, or a repeat of the last
        // jump, is not worth a step back.
        let same_line = origin.path == target && origin.position.line == position.line;
        let repeat = self
            .definition_jumps
            .back()
            .is_some_and(|last| last.path == origin.path && last.position == origin.position);
        if same_line || repeat {
            return;
        }
        if self.definition_jumps.len() == MAX_JUMP_HISTORY {
            self.definition_jumps.pop_front();
        }
        self.definition_jumps.push_back(origin);
    }

    /// Return to the position the most recent jump started from.
    pub(crate) fn jump_back(&mut self) {
        while let Some(origin) = self.definition_jumps.pop_back() {
            // Never resurrect a file deleted since the jump: `open_file` would
            // happily produce an empty phantom buffer for it.
            if !origin.path.exists() {
                continue;
            }
            self.focus_pane_switch(origin.pane);
            // Deliberately not `jump_to_location`: pushing a new origin here would
            // make Go Back bounce between two positions forever.
            self.focus_by_file_line(&origin.path, origin.position);
            return;
        }
        self.status = Some("nothing to go back to".to_string());
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

    /// The word under the pointer, when Ctrl is held over a symbol a click could
    /// actually resolve.
    fn hovered_definition_symbol(&self, mouse: &MouseEvent) -> Option<DefinitionHover> {
        if !mouse
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
        {
            return None;
        }
        // Promise only what a click can deliver: this is the same condition under
        // which the backend finds a server to ask, so no probe request is needed.
        if self.active_language_server_badge() != Some(LanguageServerBadge::InSync) {
            return None;
        }
        let point = (mouse.column, mouse.row);
        let frame = self
            .pane_frames
            .iter()
            .find(|frame| rect_contains(frame.content_rect, point))?;
        if frame.pane != self.focus_pane() || !rect_contains(frame.editor_rect, point) {
            return None;
        }
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code {
            doc: Some(_),
            buffer,
            folds,
            folded,
            ..
        } = &tab.kind
        else {
            return None;
        };
        let fold_lines = resolve_folds(folds, folded);
        let area = frame.editor_rect;
        let pos = tab
            .editor
            .pos_at(area, buffer, &fold_lines, mouse.column, mouse.row);
        // Round-trip guard: `pos_at` clamps a point in the gutter, past the end of a
        // line, or in a reserved scrollbar track onto a real position, which would
        // underline a word the pointer is not actually over. Asking where that
        // position *renders* rejects all of those at once, without this code needing
        // to know the geometry.
        if tab.editor.screen_cell(area, buffer, &fold_lines, pos) != Some(point) {
            return None;
        }
        let (start, end) = word_at(buffer, pos);
        // `word_bounds` falls back to a single character off a word; only a real
        // identifier is navigable.
        if start == end {
            return None;
        }
        Some(DefinitionHover {
            view: tab.view,
            range: Range { start, end },
        })
    }

    /// The underline decoration for the Ctrl-hovered symbol in the active tab.
    pub(crate) fn definition_underline_decoration(&self) -> Option<Decoration> {
        let hover = self.definition_hover?;
        let tab = self.tabs.get(self.active)?;
        (tab.view == hover.view).then_some(Decoration {
            range: hover.range,
            kind: DecorationKind::Underline,
            // No role: the underline takes the token's own foreground, so it can
            // never come out illegible in a theme that never anticipated it.
            role: None,
        })
    }
}
