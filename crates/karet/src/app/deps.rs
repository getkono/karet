//! Dependency-hint commands for open manifests (`Cargo.toml`): refresh the
//! check, bump one dependency, bump them all. The hints themselves stream in
//! as `Event::ManifestHints` and render as end-of-line annotations.

use karet_core::LineCol;
use karet_core::Range;
use karet_editor::editing;
use karet_session::ManifestHint;
use karet_session::ManifestHintState;

use super::*;

impl App {
    /// The active tab's manifest hints, when it is a checked manifest at the
    /// checked version.
    fn active_manifest_hints(&self) -> Option<(&Tab, &[ManifestHint])> {
        let tab = self.tabs.get(self.active)?;
        let TabKind::Code {
            doc: Some(doc),
            buffer,
            ..
        } = &tab.kind
        else {
            return None;
        };
        let (version, hints) = self.docs.manifest_hints.get(doc)?;
        (*version == buffer.version()).then_some((tab, hints.as_slice()))
    }

    /// A markdown section describing the dependency under `line`, for the
    /// hover popup.
    pub(super) fn manifest_hint_markdown(&self, doc: DocumentId, line: u32) -> Option<String> {
        let (version, hints) = self.docs.manifest_hints.get(&doc)?;
        let current_version = self.tabs.iter().find_map(|tab| match &tab.kind {
            TabKind::Code {
                doc: Some(d),
                buffer,
                ..
            } if *d == doc => Some(buffer.version()),
            _ => None,
        })?;
        if *version != current_version {
            return None;
        }
        let hint = hints.iter().find(|hint| hint.line == line)?;
        let state = match hint.state {
            ManifestHintState::UpToDate => "up to date",
            ManifestHintState::Patch => "patch available",
            ManifestHintState::Outdated => "update available",
            ManifestHintState::Vulnerable => "**vulnerable**",
            ManifestHintState::Error => "check failed",
        };
        let mut text = format!("**{}** `{}` — {state}", hint.name, hint.current);
        if let Some(latest) = &hint.latest {
            text.push_str(&format!(" (latest `{latest}`)"));
        }
        if !hint.vulnerabilities.is_empty() {
            text.push_str(&format!(
                "\n\nAdvisories: {}",
                hint.vulnerabilities.join(", ")
            ));
        }
        text.push_str(&format!(
            "\n\n[docs.rs](https://docs.rs/{0}) · [crates.io](https://crates.io/crates/{0})",
            hint.name
        ));
        Some(text)
    }

    /// Re-run the freshness check for the active manifest.
    pub(super) fn deps_refresh(&mut self) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let TabKind::Code { doc: Some(doc), .. } = &tab.kind else {
            return;
        };
        let doc = *doc;
        self.send(SessionCommand::RefreshManifestHints { doc });
        self.status = Some("re-checking dependencies".to_owned());
    }

    /// Bump the dependency under the caret to its newest version.
    pub(super) fn deps_update_at_caret(&mut self) {
        let Some((tab, hints)) = self.active_manifest_hints() else {
            self.status = Some("no dependency hints for this tab".to_owned());
            return;
        };
        let caret = tab.editor.cursor();
        let Some(edit) = hints
            .iter()
            .find(|hint| hint.line == caret.line)
            .and_then(hint_edit)
        else {
            self.status = Some("no update available on this line".to_owned());
            return;
        };
        let (range, text, name) = edit;
        let status = format!("{name} → {text}");
        self.submit_edit(move |c, _s, _b, base| {
            (c == caret).then(|| editing::insert(range.start, Some(range), base, &text))
        });
        self.status = Some(status);
    }

    /// Bump every outdated dependency in the active manifest in one edit.
    pub(super) fn deps_update_all(&mut self) {
        let Some((tab, hints)) = self.active_manifest_hints() else {
            self.status = Some("no dependency hints for this tab".to_owned());
            return;
        };
        let caret = tab.editor.cursor();
        let edits: Vec<(Range, String)> = hints
            .iter()
            .filter_map(hint_edit)
            .map(|(range, text, _)| (range, text))
            .collect();
        if edits.is_empty() {
            self.status = Some("every dependency is current".to_owned());
            return;
        }
        let count = edits.len();
        self.submit_edit(move |c, _s, _b, base| {
            if c != caret {
                return None;
            }
            let mut all = edits.iter();
            let (first_range, first_text) = all.next()?;
            let mut edit = editing::insert(first_range.start, Some(*first_range), base, first_text);
            for (range, text) in all {
                edit.change.edits.push(karet_core::TextEdit {
                    range: *range,
                    new_text: text.clone(),
                });
            }
            edit.caret = c;
            Some(edit)
        });
        self.status = Some(format!(
            "updated {count} dependenc{}",
            if count == 1 { "y" } else { "ies" }
        ));
    }
}

/// The in-place version replacement a hint offers, when it offers one.
fn hint_edit(hint: &ManifestHint) -> Option<(Range, String, String)> {
    if !matches!(
        hint.state,
        ManifestHintState::Patch | ManifestHintState::Outdated | ManifestHintState::Vulnerable
    ) {
        return None;
    }
    let latest = hint.latest.clone()?;
    let range = Range {
        start: LineCol::new(hint.line, hint.col_start),
        end: LineCol::new(hint.line, hint.col_end),
    };
    Some((range, latest, hint.name.clone()))
}
