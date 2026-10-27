//! Code actions: the quick-fix menu (Ctrl+., Alt+Enter), the source actions
//! in the palette, and the gutter lightbulb.
//!
//! The backend owns the whole action -- `CodeActions` lists what the language
//! server offers over the caret (the diagnostics there go along as context),
//! `ApplyCodeAction` resolves, applies, and runs the one picked. This module
//! is the client half: asking, showing, and reporting.
//!
//! The menu is the app's context menu, anchored under the caret, its actions
//! grouped by kind. Like Go to Definition it shows nothing while the answer is
//! on its way: a placeholder for a sub-second round trip is the flicker the
//! loading-state rules warn off.
//!
//! The lightbulb is a probe: once the caret (or the text) has been still for
//! the edit debounce, the backend is asked, as an automatic request that never
//! reports a server without code actions, and the caret line shows the bulb
//! when any action could run there. A move within the line keeps the bulb up
//! until the next answer replaces it; a move to another line, or an edit,
//! takes it down at once, since it no longer describes where the caret is.

use karet_core::CodeAction;
use karet_session::AppliedWorkspaceEdit;
use karet_session::CodeActionTrigger;
use karet_session::LSP_CHANGE_DEBOUNCE;

use super::*;

/// The kind prefixes the menu groups by, in the order the groups appear, and
/// each group's heading. Anything else falls in a last "More Actions" group.
const GROUPS: [(&str, &str); 3] = [
    ("quickfix", "Quick Fix"),
    ("refactor", "Refactor"),
    ("source", "Source Action"),
];

/// What a requested list of actions is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ActionsFor {
    /// Show them in the menu.
    Menu,
    /// Run the first one that can run, preferring the server's preferred one
    /// (Organize Imports, Fix All); the label names the command in notices.
    First(&'static str),
}

/// A `CodeActions` request the user made, awaiting its answer.
#[derive(Clone, Debug)]
pub(crate) struct PendingActions {
    /// The request.
    pub(crate) id: RequestId,
    /// The document asked about.
    pub(crate) doc: DocumentId,
    /// The view it was asked from; switching away drops the answer.
    pub(crate) view: ViewId,
    /// What the answer is for.
    pub(crate) purpose: ActionsFor,
}

/// Where the lightbulb probe stands: a caret position at one revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProbeKey {
    /// The document.
    pub(crate) doc: DocumentId,
    /// Its version.
    pub(crate) version: u64,
    /// The primary selection, or the caret as an empty range.
    pub(crate) range: Range,
}

/// The lightbulb probe in flight or waiting for the caret to settle.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Probe {
    /// What it asks about.
    pub(crate) key: ProbeKey,
    /// When it may be sent: the caret has to stay put until then.
    pub(crate) due: Instant,
    /// The request, once sent.
    pub(crate) sent: Option<RequestId>,
}

/// The client's code-action state.
#[derive(Clone, Debug, Default)]
pub(crate) struct CodeActions {
    /// The menu or palette request awaiting its answer.
    pub(crate) pending: Option<PendingActions>,
    /// The `ApplyCodeAction` awaiting its answer, and the action's title.
    pub(crate) applying: Option<(RequestId, String)>,
    /// The lightbulb probe.
    pub(crate) probe: Option<Probe>,
    /// Where the lightbulb shows: the probe position whose answer had an
    /// action that can run.
    pub(crate) bulb: Option<ProbeKey>,
}

/// The lightbulb glyph in `style`.
fn bulb_glyph(style: IconStyle) -> char {
    match style {
        IconStyle::NerdFont => '\u{f0eb}', // lightbulb
        IconStyle::Unicode => '\u{2726}',  // ✦ black four-pointed star
        IconStyle::Ascii => '*',
    }
}

/// The menu rows for `actions`: grouped by kind in [`GROUPS`] order, the
/// server's preferred actions first within a group, a heading over each group
/// when there is more than one, and disabled actions dimmed with their reason.
pub(crate) fn menu_entries(
    doc: DocumentId,
    version: u64,
    actions: Vec<CodeAction>,
) -> Vec<ContextMenuEntry> {
    let group_of = |action: &CodeAction| {
        GROUPS
            .iter()
            .position(|(kind, _)| action.is_kind(kind))
            .unwrap_or(GROUPS.len())
    };
    let mut groups: Vec<Vec<CodeAction>> = vec![Vec::new(); GROUPS.len() + 1];
    for action in actions {
        let group = group_of(&action);
        if let Some(members) = groups.get_mut(group) {
            members.push(action);
        }
    }
    let shown = groups.iter().filter(|members| !members.is_empty()).count();
    let mut entries = Vec::new();
    for (index, mut members) in groups.into_iter().enumerate() {
        if members.is_empty() {
            continue;
        }
        // Stable: the server's order holds among equals.
        members.sort_by_key(|action| !action.preferred);
        if shown > 1 {
            let heading = GROUPS.get(index).map_or("More Actions", |(_, name)| name);
            entries.push(ContextMenuEntry {
                action: ContextMenuAction::Heading,
                label: Some(heading.to_owned()),
                enabled: false,
                note: None,
            });
        }
        for action in members {
            let title = action.title.clone();
            let disabled = action.disabled.clone();
            let row = ContextMenuAction::CodeAction {
                doc,
                version,
                action: Box::new(action),
            };
            entries.push(match disabled {
                Some(reason) => ContextMenuEntry::disabled_custom(title, row, reason),
                None => ContextMenuEntry::custom(title, row),
            });
        }
    }
    entries
}

/// The action [`ActionsFor::First`] runs: the first preferred one that can
/// run, else the first that can.
fn first_runnable(actions: Vec<CodeAction>) -> Option<CodeAction> {
    let runnable = |action: &CodeAction| action.disabled.is_none();
    let preferred = actions
        .iter()
        .position(|action| action.preferred && runnable(action));
    let index = preferred.or_else(|| actions.iter().position(runnable))?;
    actions.into_iter().nth(index)
}

impl App {
    /// The active code view's document, version, and primary selection (the
    /// caret as an empty range when nothing is selected).
    fn code_action_target(&self) -> Option<(ProbeKey, ViewId)> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code {
            doc: Some(doc),
            buffer,
            ..
        } = &tab.kind
        else {
            return None;
        };
        let caret = tab.editor.cursor();
        let range = tab.editor.selection_range().unwrap_or(Range {
            start: caret,
            end: caret,
        });
        let key = ProbeKey {
            doc: *doc,
            version: buffer.version(),
            range,
        };
        Some((key, tab.view))
    }

    /// Ask for the actions at the caret, limited to `kinds`, for `purpose`.
    pub(crate) fn request_code_actions(&mut self, kinds: &[&str], purpose: ActionsFor) {
        let Some((key, view)) = self.code_action_target() else {
            let what = match &purpose {
                ActionsFor::Menu => "code actions",
                ActionsFor::First(label) => label,
            };
            let message = format!("{what}: open a code file first");
            self.notify(Report::Refusal, NotificationKind::Lsp, message);
            return;
        };
        let id = self.send(LspCommand::CodeActions {
            doc: key.doc,
            range: key.range,
            kinds: kinds.iter().map(|kind| (*kind).to_owned()).collect(),
            trigger: CodeActionTrigger::Invoked,
        });
        self.code_actions.pending = id.map(|id| PendingActions {
            id,
            doc: key.doc,
            view,
            purpose,
        });
    }

    /// Adopt a `CodeActions` answer: the lightbulb's, or the menu's.
    pub(crate) fn on_code_actions(
        &mut self,
        id: Option<RequestId>,
        doc: DocumentId,
        version: u64,
        actions: Vec<CodeAction>,
        reason: Option<String>,
    ) {
        if let Some(probe) = self.code_actions.probe
            && probe.sent.is_some()
            && probe.sent == id
        {
            let runnable = actions.iter().any(|action| action.disabled.is_none());
            self.code_actions.bulb = runnable.then_some(probe.key);
            return;
        }
        let Some(pending) = self.code_actions.pending.take_if(|p| Some(p.id) == id) else {
            return; // superseded, or a probe answering late
        };
        // Moved on to another view, or typed since: the answer describes text
        // that is no longer in front of the user.
        let current = self.code_action_target();
        if current.is_none_or(|(key, view)| {
            view != pending.view || doc != pending.doc || key.doc != doc || key.version != version
        }) {
            return;
        }
        match pending.purpose {
            ActionsFor::Menu if actions.is_empty() => {
                let reason = reason.unwrap_or_else(|| "none available here".to_owned());
                let message = format!("code actions: {reason}");
                self.notify(Report::Refusal, NotificationKind::Lsp, message);
            },
            ActionsFor::Menu => self.open_code_action_menu(doc, version, actions),
            ActionsFor::First(label) => match first_runnable(actions) {
                Some(action) => self.run_code_action(doc, version, action),
                None => {
                    let reason = reason.unwrap_or_else(|| "nothing to do here".to_owned());
                    let message = format!("{label}: {reason}");
                    self.notify(Report::Refusal, NotificationKind::Lsp, message);
                },
            },
        }
    }

    /// Open the menu over `actions`, anchored on the line under the caret.
    fn open_code_action_menu(&mut self, doc: DocumentId, version: u64, actions: Vec<CodeAction>) {
        let editor = self.last_frame.editor_rect();
        let anchor = self.tabs.get(self.active).and_then(|tab| {
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
            tab.editor.primary_caret_cell(editor, buffer, &fold_lines)
        });
        let (x, y) = anchor.map_or((editor.left(), editor.top()), |(x, y)| {
            (x, y.saturating_add(1))
        });
        let entries = menu_entries(doc, version, actions);
        self.context_menu = Some(ContextMenu::new(x, y, entries));
    }

    /// Run one listed action.
    pub(crate) fn run_code_action(&mut self, doc: DocumentId, version: u64, action: CodeAction) {
        let title = action.title.clone();
        let id = self.send(LspCommand::ApplyCodeAction {
            doc,
            version,
            action,
        });
        self.code_actions.applying = id.map(|id| (id, title));
        // Whatever the bulb pointed at is being acted on; the next probe says
        // what is left.
        self.code_actions.bulb = None;
    }

    /// The answer to a run action, reported only when it did not work: a
    /// change that landed is already on screen. `true` when it was ours.
    pub(crate) fn settle_code_action(
        &mut self,
        id: Option<RequestId>,
        applied: &AppliedWorkspaceEdit,
    ) -> bool {
        let Some((_, title)) = self
            .code_actions
            .applying
            .take_if(|(pending, _)| Some(*pending) == id)
        else {
            return false;
        };
        if !applied.result.applied {
            let reason = applied
                .result
                .failure_reason
                .clone()
                .unwrap_or_else(|| "the action was not applied".to_owned());
            self.notify(
                Report::Failure,
                NotificationKind::Lsp,
                format!("{title}: {reason}"),
            );
        }
        true
    }

    /// The server does not offer code actions: the notice already says so,
    /// so the empty answer that follows must not add "none available".
    pub(crate) fn refuse_code_actions(&mut self, id: RequestId) {
        self.code_actions
            .pending
            .take_if(|pending| pending.id == id);
    }

    /// Move the lightbulb probe along with the caret, and send it once the
    /// caret has settled. Called after every frame.
    pub(crate) fn probe_code_actions(&mut self, now: Instant) {
        let Some((key, _)) = self.code_action_target() else {
            self.code_actions.probe = None;
            self.code_actions.bulb = None;
            return;
        };
        let state = &mut self.code_actions;
        if state.bulb.is_some_and(|bulb| {
            bulb.doc != key.doc
                || bulb.version != key.version
                || bulb.range.start.line != key.range.start.line
        }) {
            state.bulb = None;
        }
        match &mut state.probe {
            Some(probe) if probe.key == key => {
                if probe.sent.is_some() || now < probe.due {
                    return;
                }
            },
            probe => {
                *probe = Some(Probe {
                    key,
                    due: now + LSP_CHANGE_DEBOUNCE,
                    sent: None,
                });
                return;
            },
        }
        let sent = self.send(LspCommand::CodeActions {
            doc: key.doc,
            range: key.range,
            kinds: Vec::new(),
            trigger: CodeActionTrigger::Automatic,
        });
        if let Some(probe) = self.code_actions.probe.as_mut() {
            // Marked sent even when there is no backend to send to, so an
            // unanswerable probe is not re-sent every frame.
            probe.sent = Some(sent.unwrap_or(RequestId(0)));
        }
    }

    /// How long until the probe is due, so the loop wakes to send it without
    /// another keystroke.
    pub(crate) fn code_action_next_wake(&self, now: Instant) -> Option<Duration> {
        let probe = self.code_actions.probe?;
        probe
            .sent
            .is_none()
            .then(|| probe.due.saturating_duration_since(now))
    }

    /// The lightbulb in the gutter of the caret line, when the active view's
    /// document has an action there at its current version.
    pub(crate) fn code_action_bulb_decoration(&self) -> Option<Decoration> {
        let bulb = self.code_actions.bulb?;
        let (key, _) = self.code_action_target()?;
        if key.doc != bulb.doc || key.version != bulb.version {
            return None;
        }
        let line = key.range.start.line;
        Some(Decoration::new(
            Range {
                start: LineCol::new(line, 0),
                end: LineCol::new(line, 0),
            },
            DecorationKind::GutterMarker {
                glyph: bulb_glyph(self.icon_style),
            },
            Some(ThemeRole::DiagnosticWarning),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(title: &str, kind: Option<&str>) -> CodeAction {
        CodeAction {
            title: title.to_owned(),
            kind: kind.map(str::to_owned),
            ..CodeAction::default()
        }
    }

    fn labels(entries: &[ContextMenuEntry]) -> Vec<(String, bool)> {
        entries
            .iter()
            .map(|entry| (entry.label.clone().unwrap_or_default(), entry.enabled))
            .collect()
    }

    #[test]
    fn the_menu_groups_by_kind_with_preferred_actions_first() {
        let mut preferred = action("Import Foo", Some("quickfix"));
        preferred.preferred = true;
        let mut disabled = action("Inline", Some("refactor.inline"));
        disabled.disabled = Some("nothing to inline".into());
        let entries = menu_entries(
            DocumentId(1),
            3,
            vec![
                action("Organize", Some("source.organizeImports")),
                action("Extract", Some("refactor.extract")),
                action("Qualify Foo", Some("quickfix")),
                preferred,
                action("Run test", None),
                disabled,
            ],
        );
        let owned = |label: &str, enabled| (label.to_owned(), enabled);
        assert_eq!(
            labels(&entries),
            vec![
                owned("Quick Fix", false),
                owned("Import Foo", true),
                owned("Qualify Foo", true),
                owned("Refactor", false),
                owned("Extract", true),
                owned("Inline", false),
                owned("Source Action", false),
                owned("Organize", true),
                owned("More Actions", false),
                owned("Run test", true),
            ]
        );
        // A heading explains nothing when clicked; a disabled action does.
        assert_eq!(entries[0].note, None);
        assert_eq!(entries[5].note.as_deref(), Some("nothing to inline"));
        assert!(matches!(
            &entries[1].action,
            ContextMenuAction::CodeAction { doc: DocumentId(1), version: 3, action }
                if action.title == "Import Foo"
        ));
    }

    #[test]
    fn a_single_group_needs_no_heading() {
        let entries = menu_entries(
            DocumentId(1),
            0,
            vec![
                action("A", Some("quickfix")),
                action("B", Some("quickfix.other")),
            ],
        );
        assert_eq!(
            labels(&entries),
            vec![("A".to_owned(), true), ("B".to_owned(), true)]
        );
    }

    #[test]
    fn the_first_runnable_action_prefers_the_preferred_one() {
        let mut blocked = action("Blocked", None);
        blocked.disabled = Some("no".into());
        let mut preferred = action("Preferred", None);
        preferred.preferred = true;
        let mut preferred_but_disabled = action("Preferred, disabled", None);
        preferred_but_disabled.preferred = true;
        preferred_but_disabled.disabled = Some("no".into());
        let pick = |actions| first_runnable(actions).map(|action| action.title);
        assert_eq!(
            pick(vec![
                blocked.clone(),
                action("Plain", None),
                preferred.clone()
            ]),
            Some("Preferred".to_owned())
        );
        assert_eq!(
            pick(vec![
                preferred_but_disabled,
                blocked.clone(),
                action("Plain", None)
            ]),
            Some("Plain".to_owned())
        );
        assert_eq!(pick(vec![blocked]), None);
    }
}
