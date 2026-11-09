//! The context menu every surface shares: opening the pane menu over a file
//! tab, stepping through the rows, and running the one accepted.
//!
//! What a menu offers belongs to the surface that opens it — the explorer's
//! rows to `explorer`, the spelling and code-action rows to theirs — while the
//! menu itself, and what accepting a row does, lives here once.

use std::path::Path;

use karet_core::NotificationKind;

use crate::app::App;
use crate::app::notifications::Report;
use crate::app::state::ContextMenu;
use crate::app::state::ContextMenuAction;
use crate::app::state::ContextMenuEntry;
use crate::app::state::DictionaryTarget;
use crate::command::Command;
use crate::remote;
use crate::tab::Tab;
use crate::tab::TabKind;

impl App {
    /// Close the context menu, if one is open.
    pub(super) fn context_menu_clear(&mut self) {
        self.context_menu = None;
    }

    /// Open the pane context menu at `(x, y)` for the focused pane's active tab.
    /// Only file-backed tabs get one; a pathless tab (Welcome, commit graph, …)
    /// opens nothing.
    ///
    /// `over_text` is whether the right click landed on the document's text. Only
    /// then does a text document's menu lead with Cut, Copy and Paste: they act on
    /// the selection, or on the caret that click has just placed. A right click on
    /// the tab strip names the file, not the text, so it offers the file rows alone.
    pub(super) fn open_pane_context_menu(&mut self, x: u16, y: u16, over_text: bool) {
        let Some(path) = self
            .tabs
            .get(self.active)
            .and_then(Tab::path)
            .map(Path::to_path_buf)
        else {
            return;
        };
        let text = over_text
            && matches!(
                self.tabs.get(self.active).map(|tab| &tab.kind),
                Some(TabKind::Code { .. })
            );
        let mut entries = if text {
            vec![
                ContextMenuEntry::enabled(Command::Cut),
                ContextMenuEntry::enabled(Command::Copy),
                ContextMenuEntry::enabled(Command::Paste),
            ]
        } else {
            Vec::new()
        };
        entries.extend(self.pane_context_entries(&path));
        self.context_menu = Some(ContextMenu::new(x, y, entries));
    }

    /// The pane context menu's rows for the active file at `path`. The path items
    /// always work; the link items are enabled exactly when [`remote::link`] can
    /// build them (the same call their dispatch runs), with its refusal reason as
    /// the disabled note.
    pub(super) fn pane_context_entries(&mut self, path: &Path) -> Vec<ContextMenuEntry> {
        let mut entries = vec![
            ContextMenuEntry::enabled(Command::CopyPath),
            ContextMenuEntry::enabled(Command::CopyRelativePath),
            ContextMenuEntry::enabled(Command::RevealActiveInExplorer),
        ];
        // Owned disabled-notes computed up front, so the facts borrow does not
        // overlap the &mut calls below. `None` = enabled.
        let link_notes: [Option<String>; 3] = {
            let facts = self.cached_remote_facts(path);
            let note = |kind| match facts {
                Some(Ok(facts)) => remote::link(&facts.link_target(), kind, None).err(),
                Some(Err(note)) => Some(note.clone()),
                None => Some("resolving repository remote…".to_string()),
            };
            [
                note(remote::LinkKind::RemoteFile),
                note(remote::LinkKind::GithubPermalink),
                note(remote::LinkKind::GithubHeadLink),
            ]
        };
        let link_entry = |command, note: &Option<String>| match note {
            None => ContextMenuEntry::enabled(command),
            Some(note) => ContextMenuEntry::disabled(command, note.clone()),
        };
        entries.push(link_entry(Command::CopyRemoteFileUrl, &link_notes[0]));
        // The Open Changes actions need a repository and a tracked file, but no
        // remote — their enablement is checked separately from the link rows.
        let changes_note = self.open_changes_note(path);
        for command in [
            Command::OpenChangesWithPrevious,
            Command::OpenChangesWithRevision,
            Command::OpenChangesWithBranch,
        ] {
            entries.push(match &changes_note {
                None => ContextMenuEntry::enabled(command),
                Some(note) => ContextMenuEntry::disabled(command, note.clone()),
            });
        }
        entries.push(link_entry(Command::CopyGithubPermalink, &link_notes[1]));
        entries.push(link_entry(Command::CopyGithubHeadLink, &link_notes[2]));
        entries
    }

    pub(super) fn context_menu_step(&mut self, delta: i32) {
        if let Some(menu) = self.context_menu.as_mut() {
            menu.select_by(delta);
        }
    }

    pub(super) fn accept_context_menu(&mut self) {
        let Some(entry) = self
            .context_menu
            .as_ref()
            .and_then(ContextMenu::selected_entry)
        else {
            self.context_menu = None;
            return;
        };
        if !entry.enabled {
            // Refuse a disabled row: surface its explanatory note (when it has one)
            // and keep the menu open so another row can be chosen.
            if let Some(note) = entry.note.clone() {
                self.notify(Report::Refusal, NotificationKind::System, note);
            }
            return;
        }
        let action = entry.action.clone();
        self.context_menu = None;
        match action {
            ContextMenuAction::Command(command) => self.dispatch(command),
            ContextMenuAction::ReplaceSpelling {
                doc,
                range,
                replacement,
            } => self.replace_spelling(doc, range, replacement),
            ContextMenuAction::AddSpellingToDictionary { word, target } => match target {
                DictionaryTarget::Project => self.add_spelling_to_project_dictionary(word),
                DictionaryTarget::User => self.add_spelling_to_user_dictionary(word),
            },
            ContextMenuAction::CodeAction {
                doc,
                version,
                action,
            } => self.run_code_action(doc, version, *action),
            ContextMenuAction::Heading => {},
        }
    }

    pub(super) fn close_context_menu(&mut self) {
        self.context_menu_clear();
    }
}
