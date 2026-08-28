//! Centered modal overlays: quick-open (go to file), the command palette, the
//! diff-target picker, and the branch/stash forms and prompts.
//!
//! Every list-style overlay is one shared fuzzy [`Picker`] (from
//! `karet-widgets`, ranked by `karet-fuzzy`) whose items *are* their
//! [`OverlayEvent`] outcomes — accepting a row just clones its event.

use std::path::Path;
use std::path::PathBuf;

use karet_core::LineCol;
use karet_session::LanguageServerId;
use karet_session::LanguageServerPlanId;
use karet_session::PullRequestSummary;
use karet_vcs::BranchTarget;
use karet_vcs::CreateBranchOptions;
use karet_vcs::StashOptions;

use crate::command::Command;
use crate::command::{self};
use crate::keymap;

/// The outcome of accepting the highlighted overlay row.
#[derive(Clone)]
pub enum OverlayEvent {
    /// Nothing was highlighted; dismiss the overlay.
    Close,
    /// Open the chosen file.
    AcceptFile(PathBuf),
    /// Run the chosen command.
    AcceptCommand(Command),
    /// Run the edited interactive-rebase plan.
    AcceptRebaseTodo {
        /// The revision to rebase onto.
        onto: String,
        /// The plan, oldest first.
        steps: Vec<karet_vcs::RebaseStep>,
    },
    /// Diff the active file against the chosen revision.
    AcceptDiffTarget {
        /// The revision to diff against (a full hash or a branch name).
        rev: String,
        /// The short human label for the diff title (a short hash or branch name).
        label: String,
    },
    /// Switch to the selected local or remote-tracking branch.
    AcceptBranch(BranchTarget),
    /// Submit the complete create-branch form.
    AcceptCreateBranch(CreateBranchOptions),
    /// Fetch and check out an open pull request.
    AcceptPullRequest { remote: String, number: u64 },
    /// Submit the stash-creation form.
    AcceptStash(StashOptions),
    /// Run an action for one stash entry.
    AcceptStashAction(StashAction),
    /// Submit a free-text prompt for a follow-up action.
    AcceptText { purpose: TextPurpose, text: String },
    /// Safely delete the selected local branch.
    AcceptDeleteLocalBranch(String),
    /// Arm typed confirmation for the selected remote branch.
    AcceptDeleteRemoteBranch { remote: String, branch: String },
    /// Open the Seam view on the chosen start point.
    AcceptSeamRoot(PathBuf),
    /// Jump to one of several definition locations.
    AcceptLocation {
        /// The defining file.
        path: PathBuf,
        /// Where in it the caret should land.
        position: LineCol,
    },
}

/// Follow-up action selected for one stash.
#[derive(Clone)]
pub enum StashAction {
    /// Preview the stash patch.
    Preview(String),
    /// Apply without removing.
    Apply(String),
    /// Apply and remove.
    Pop(String),
    /// Permanently remove.
    Drop(String),
    /// Prompt for a new branch name.
    Branch(String),
}

/// Meaning attached to a generic text prompt.
#[derive(Clone)]
pub enum TextPurpose {
    /// Create a branch from this stash reference.
    StashBranch { reference: String },
    /// Rename `old` to the submitted name.
    RenameBranch { old: String },
    /// Tag `rev` with the submitted name (lightweight).
    TagCreate { rev: String },
    /// Evaluate the submitted expression in the debuggee.
    DebugEvaluate,
    /// Confirm the first network-backed installation by typing `install`.
    InstallLanguageServer { server: LanguageServerId },
    /// Approve the exact update plan displayed by the backend.
    ApplyLanguageServerPlan {
        plan: LanguageServerPlanId,
        servers: Vec<LanguageServerId>,
        /// Whether this plan installs a missing provider rather than updating one.
        install: bool,
    },
    /// Replace the language-server manager's filter with the submitted text.
    FilterLanguageServers,
    /// Confirm deactivation of one Karet-managed provider by typing `uninstall`.
    UninstallLanguageServer { server: LanguageServerId },
}

pub(crate) struct BranchForm {
    name: String,
    start_point: String,
    switch: bool,
    remotes: Vec<String>,
    publish: usize,
    set_upstream: bool,
    selected: usize,
    rows: Vec<String>,
}

impl BranchForm {
    fn new(remotes: Vec<String>) -> Self {
        let mut form = Self {
            name: String::new(),
            start_point: "HEAD".to_string(),
            switch: true,
            remotes,
            publish: 0,
            set_upstream: true,
            selected: 0,
            rows: Vec::new(),
        };
        form.refresh();
        form
    }

    fn refresh(&mut self) {
        let remote = if self.publish == 0 {
            "do not publish".to_string()
        } else {
            format!("publish to {}", self.remotes[self.publish - 1])
        };
        self.rows = vec![
            format!("Name              {}", self.name),
            format!("Start point       {}", self.start_point),
            format!("Switch now        {}", yes_no(self.switch)),
            format!("Publish remote    {remote}"),
            format!("Set upstream      {}", yes_no(self.set_upstream)),
        ];
    }

    fn push_char(&mut self, c: char) {
        match self.selected {
            0 => self.name.push(c),
            1 => self.start_point.push(c),
            2 if c == ' ' => self.switch = !self.switch,
            3 if c == ' ' => self.publish = (self.publish + 1) % (self.remotes.len() + 1),
            4 if c == ' ' => self.set_upstream = !self.set_upstream,
            _ => {},
        }
        self.refresh();
    }

    fn pop_char(&mut self) {
        match self.selected {
            0 => {
                self.name.pop();
            },
            1 => {
                self.start_point.pop();
            },
            _ => {},
        }
        self.refresh();
    }

    fn options(&self) -> CreateBranchOptions {
        let mut options = CreateBranchOptions::default();
        options.name.clone_from(&self.name);
        options.start_point.clone_from(&self.start_point);
        options.switch = self.switch;
        options.publish_remote = self
            .publish
            .checked_sub(1)
            .and_then(|index| self.remotes.get(index).cloned());
        options.set_upstream = self.set_upstream;
        options
    }
}

pub(crate) struct StashForm {
    message: String,
    include_untracked: bool,
    keep_index: bool,
    selected: usize,
    rows: Vec<String>,
}

impl StashForm {
    fn new() -> Self {
        let mut form = Self {
            message: String::new(),
            include_untracked: false,
            keep_index: false,
            selected: 0,
            rows: Vec::new(),
        };
        form.refresh();
        form
    }

    fn refresh(&mut self) {
        self.rows = vec![
            format!("Message             {}", self.message),
            format!("Include untracked   {}", yes_no(self.include_untracked)),
            format!("Keep index          {}", yes_no(self.keep_index)),
        ];
    }

    fn push_char(&mut self, c: char) {
        match self.selected {
            0 => self.message.push(c),
            1 if c == ' ' => self.include_untracked = !self.include_untracked,
            2 if c == ' ' => self.keep_index = !self.keep_index,
            _ => {},
        }
        self.refresh();
    }

    fn options(&self) -> StashOptions {
        let mut options = StashOptions::default();
        options.message = (!self.message.is_empty()).then(|| self.message.clone());
        options.include_untracked = self.include_untracked;
        options.keep_index = self.keep_index;
        options
    }
}

pub(crate) struct TextPrompt {
    title: String,
    text: String,
    purpose: TextPurpose,
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// A diff-target picker row's value: the revision to resolve and its short label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffTarget {
    /// The revision to diff against (a full hash or a branch name).
    pub rev: String,
    /// The short human label for the diff title (a short hash or branch name).
    pub label: String,
}

pub use karet_widgets::picker::Picker;

/// A modal overlay.
pub enum Overlay {
    /// Interactive-rebase plan editor.
    RebaseTodo(RebaseTodoForm),
    /// A fuzzy list picker whose rows carry their accept outcomes directly.
    Picker(Picker<OverlayEvent>),
    /// Full create-branch form.
    CreateBranch(BranchForm),
    /// Stash creation form.
    StashForm(StashForm),
    /// Single-value follow-up prompt.
    Text(TextPrompt),
}

/// The interactive-rebase plan being edited: one row per commit, oldest
/// first, exactly as git's todo file reads.
#[derive(Clone, Debug)]
pub struct RebaseTodoForm {
    /// The (full) revision the branch rebases onto.
    pub onto: String,
    /// Steps oldest-first: `(action, full hash, short hash, summary)`.
    pub steps: Vec<(karet_vcs::RebaseAction, String, String, String)>,
    /// The selected row.
    pub selected: usize,
    /// Rendered rows, kept in step with `steps`.
    rows: Vec<String>,
}

impl RebaseTodoForm {
    fn rebuild_rows(&mut self) {
        self.rows = self
            .steps
            .iter()
            .map(|(action, _, short, summary)| format!("{:6} {short} {summary}", action.verb()))
            .collect();
    }

    /// Set the selected row's action.
    fn set_action(&mut self, action: karet_vcs::RebaseAction) {
        if let Some(step) = self.steps.get_mut(self.selected) {
            step.0 = action;
            self.rebuild_rows();
        }
    }

    /// Swap the selected row with its neighbour (`delta` = ±1).
    fn reorder(&mut self, delta: i32) {
        let to = self.selected.saturating_add_signed(delta as isize);
        if to < self.steps.len() && self.selected < self.steps.len() {
            self.steps.swap(self.selected, to);
            self.selected = to;
            self.rebuild_rows();
        }
    }
}

impl Overlay {
    /// Build a quick-open overlay over `(display, path)` pairs.
    #[must_use]
    pub fn quick_open(files: Vec<(String, PathBuf)>) -> Self {
        let items = files
            .into_iter()
            .map(|(label, path)| (label, OverlayEvent::AcceptFile(path)))
            .collect();
        Self::Picker(Picker::new("Go to File", items))
    }

    /// Build the picker over the start points the Seam view can be opened on.
    #[must_use]
    pub fn seam_roots(items: Vec<(String, PathBuf)>) -> Self {
        let items = items
            .into_iter()
            .map(|(label, root)| (label, OverlayEvent::AcceptSeamRoot(root)))
            .collect();
        Self::Picker(Picker::new("Open Seam View at", items))
    }

    /// Build the picker offered when a symbol has several definitions.
    ///
    /// Rows are workspace-relative and 1-based, matching every other place the app
    /// names a position. Server order is preserved — it is best-first — but exact
    /// duplicates are dropped, since servers do sometimes report a target twice.
    #[must_use]
    pub fn definitions(root: &Path, locations: Vec<karet_core::Location>) -> Self {
        let mut seen = Vec::new();
        let mut items = Vec::new();
        for location in locations {
            let position = location.range.start;
            let key = (location.path.clone(), position);
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            let relative = location.path.strip_prefix(root).unwrap_or(&location.path);
            items.push((
                format!("{}:{}", relative.display(), position.line.saturating_add(1)),
                OverlayEvent::AcceptLocation {
                    path: location.path,
                    position,
                },
            ));
        }
        Self::Picker(Picker::new("Go to Definition", items))
    }

    /// Build the command palette.
    #[must_use]
    pub fn command_palette() -> Self {
        Self::commands("Command Palette", command::palette())
    }

    /// Build a command picker over an explicit action subset.
    #[must_use]
    pub fn commands(title: impl Into<String>, commands: Vec<Command>) -> Self {
        let items = commands
            .into_iter()
            .map(|command| {
                (
                    command.label().to_string(),
                    OverlayEvent::AcceptCommand(command),
                )
            })
            .collect();
        Self::Picker(Picker::new(title, items))
    }

    /// Build a diff-target picker titled `title` over `(display, target)` pairs.
    #[must_use]
    pub fn diff_target(title: impl Into<String>, items: Vec<(String, DiffTarget)>) -> Self {
        let items = items
            .into_iter()
            .map(|(display, target)| {
                (
                    display,
                    OverlayEvent::AcceptDiffTarget {
                        rev: target.rev,
                        label: target.label,
                    },
                )
            })
            .collect();
        Self::Picker(Picker::new(title, items))
    }

    /// Build a branch picker.
    #[must_use]
    pub fn branches(items: Vec<(String, BranchTarget)>) -> Self {
        let items = items
            .into_iter()
            .map(|(label, target)| (label, OverlayEvent::AcceptBranch(target)))
            .collect();
        Self::Picker(Picker::new("Switch branch", items))
    }

    /// Build the complete branch-creation form.
    #[must_use]
    pub fn create_branch(remotes: Vec<String>) -> Self {
        Self::CreateBranch(BranchForm::new(remotes))
    }

    /// Build an open-pull-request picker.
    #[must_use]
    pub fn pull_requests(remote: String, items: Vec<PullRequestSummary>) -> Self {
        let rows = items
            .into_iter()
            .map(|item| {
                let draft = if item.draft { "draft · " } else { "" };
                let author = item.author.as_deref().unwrap_or("unknown");
                (
                    format!("#{}  {}  {draft}{author}", item.number, item.title),
                    OverlayEvent::AcceptPullRequest {
                        remote: remote.clone(),
                        number: item.number,
                    },
                )
            })
            .collect();
        Self::Picker(Picker::new("Open pull requests", rows))
    }

    /// Build the stash creation form.
    #[must_use]
    pub fn stash_form() -> Self {
        Self::StashForm(StashForm::new())
    }

    /// Build the stash manager with preview/apply/pop/drop/branch actions.
    #[must_use]
    pub fn stashes(entries: &[karet_vcs::StashEntry]) -> Self {
        let mut items = Vec::new();
        for entry in entries {
            let reference = entry.reference.clone();
            let base = format!("{}  {}", entry.reference, entry.message);
            let action = |a: StashAction| OverlayEvent::AcceptStashAction(a);
            items.push((
                format!("Preview   {base}"),
                action(StashAction::Preview(reference.clone())),
            ));
            items.push((
                format!("Apply     {base}"),
                action(StashAction::Apply(reference.clone())),
            ));
            items.push((
                format!("Pop       {base}"),
                action(StashAction::Pop(reference.clone())),
            ));
            items.push((
                format!("Branch…   {base}"),
                action(StashAction::Branch(reference.clone())),
            ));
            items.push((
                format!("Drop      {base}"),
                action(StashAction::Drop(reference)),
            ));
        }
        Self::Picker(Picker::new("Manage stashes", items))
    }

    /// Build a free-text follow-up prompt.
    #[must_use]
    pub fn text(title: impl Into<String>, purpose: TextPurpose) -> Self {
        Self::Text(TextPrompt {
            title: title.into(),
            text: String::new(),
            purpose,
        })
    }

    /// Build the interactive-rebase plan editor. `steps` oldest first as
    /// `(hash, short, summary)`.
    #[must_use]
    pub fn rebase_todo(onto: String, steps: Vec<(String, String, String)>) -> Self {
        let mut form = RebaseTodoForm {
            onto,
            steps: steps
                .into_iter()
                .map(|(hash, short, summary)| (karet_vcs::RebaseAction::Pick, hash, short, summary))
                .collect(),
            selected: 0,
            rows: Vec::new(),
        };
        form.rebuild_rows();
        Self::RebaseTodo(form)
    }

    /// Build a local-branch deletion picker.
    #[must_use]
    pub fn delete_local_branches(items: Vec<String>) -> Self {
        let rows = items
            .into_iter()
            .map(|name| (name.clone(), OverlayEvent::AcceptDeleteLocalBranch(name)))
            .collect();
        Self::Picker(Picker::new("Delete local branch", rows))
    }

    /// Build a remote-branch deletion picker.
    #[must_use]
    pub fn delete_remote_branches(items: Vec<(String, String)>) -> Self {
        let rows = items
            .into_iter()
            .map(|(remote, branch)| {
                (
                    format!("{remote}/{branch}"),
                    OverlayEvent::AcceptDeleteRemoteBranch { remote, branch },
                )
            })
            .collect();
        Self::Picker(Picker::new("Delete remote branch", rows))
    }

    /// The overlay title.
    #[must_use]
    pub fn title(&self) -> &str {
        match self {
            Self::Picker(p) => p.title(),
            Self::RebaseTodo(_) => {
                "Interactive rebase · p/r/e/s/f/d set action · K/J reorder · Enter runs"
            },
            Self::CreateBranch(_) => "Create branch · ↑↓ fields · Space toggles",
            Self::StashForm(_) => "Stash changes · ↑↓ fields · Space toggles",
            Self::Text(prompt) => &prompt.title,
        }
    }

    /// The current query string.
    #[must_use]
    pub fn query(&self) -> &str {
        match self {
            Self::Picker(p) => p.query(),
            Self::RebaseTodo(_) => "oldest first, as git applies them",
            Self::CreateBranch(_) | Self::StashForm(_) => "Edit selected field",
            Self::Text(prompt) => &prompt.text,
        }
    }

    /// The visible row labels.
    #[must_use]
    pub fn rows(&self) -> Vec<&str> {
        match self {
            Self::Picker(p) => p.rows(),
            Self::RebaseTodo(form) => form.rows.iter().map(String::as_str).collect(),
            Self::CreateBranch(form) => form.rows.iter().map(String::as_str).collect(),
            Self::StashForm(form) => form.rows.iter().map(String::as_str).collect(),
            Self::Text(_) => Vec::new(),
        }
    }

    /// The per-row right-aligned hints (key chords), aligned with [`rows`](Self::rows).
    /// Only command rows carry hints.
    #[must_use]
    pub fn row_hints(&self) -> Vec<Option<String>> {
        match self {
            Self::RebaseTodo(form) => vec![None; form.rows.len()],
            Self::Picker(p) => p
                .values()
                .into_iter()
                .map(|event| match event {
                    OverlayEvent::AcceptCommand(command) => {
                        keymap::hint_for(*command, keymap::ChordStyle::Verbose)
                    },
                    _ => None,
                })
                .collect(),
            Self::CreateBranch(form) => form.rows.iter().map(|_| None).collect(),
            Self::StashForm(form) => form.rows.iter().map(|_| None).collect(),
            Self::Text(_) => Vec::new(),
        }
    }

    /// The selected row index.
    #[must_use]
    pub fn selected(&self) -> usize {
        match self {
            Self::Picker(p) => p.selected(),
            Self::RebaseTodo(form) => form.selected,
            Self::CreateBranch(form) => form.selected,
            Self::StashForm(form) => form.selected,
            Self::Text(_) => 0,
        }
    }

    /// Move the selection up.
    pub fn select_up(&mut self) {
        match self {
            Self::Picker(p) => p.select_up(),
            Self::RebaseTodo(form) => {
                form.selected = form
                    .selected
                    .saturating_add_signed(-1)
                    .min(form.steps.len().saturating_sub(1));
            },
            Self::CreateBranch(form) => form.selected = form.selected.saturating_sub(1),
            Self::StashForm(form) => form.selected = form.selected.saturating_sub(1),
            Self::Text(_) => {},
        }
    }

    /// Move the selection down.
    pub fn select_down(&mut self) {
        match self {
            Self::Picker(p) => p.select_down(),
            Self::RebaseTodo(form) => {
                form.selected = form
                    .selected
                    .saturating_add_signed(1)
                    .min(form.steps.len().saturating_sub(1));
            },
            Self::CreateBranch(form) => form.selected = (form.selected + 1).min(4),
            Self::StashForm(form) => form.selected = (form.selected + 1).min(2),
            Self::Text(_) => {},
        }
    }

    /// Append a character to the query.
    pub fn push_char(&mut self, c: char) {
        match self {
            Self::Picker(p) => p.push_char(c),
            Self::RebaseTodo(form) => {
                use karet_vcs::RebaseAction as A;
                match c {
                    'p' => form.set_action(A::Pick),
                    'r' => form.set_action(A::Reword),
                    'e' => form.set_action(A::Edit),
                    's' => form.set_action(A::Squash),
                    'f' => form.set_action(A::Fixup),
                    'd' => form.set_action(A::Drop),
                    'K' => form.reorder(-1),
                    'J' => form.reorder(1),
                    _ => {},
                }
            },
            Self::CreateBranch(form) => form.push_char(c),
            Self::StashForm(form) => form.push_char(c),
            Self::Text(prompt) => prompt.text.push(c),
        }
    }

    /// Remove the last query character.
    pub fn pop_char(&mut self) {
        match self {
            Self::Picker(p) => p.pop_char(),
            Self::RebaseTodo(_) => {},
            Self::CreateBranch(form) => form.pop_char(),
            Self::StashForm(form) => {
                if form.selected == 0 {
                    form.message.pop();
                    form.refresh();
                }
            },
            Self::Text(prompt) => {
                prompt.text.pop();
            },
        }
    }

    /// Append pasted text to the query.
    pub fn push_str(&mut self, text: &str) {
        match self {
            Self::Picker(p) => p.push_str(text),
            Self::RebaseTodo(_) => {},
            Self::CreateBranch(form) => {
                for character in text.chars() {
                    form.push_char(character);
                }
            },
            Self::StashForm(form) => {
                if form.selected == 0 {
                    form.message.push_str(text);
                    form.refresh();
                }
            },
            Self::Text(prompt) => prompt.text.push_str(text),
        }
    }

    /// The outcome of accepting the highlighted row — the row's own event for a
    /// picker, the assembled form/prompt for the rest — or
    /// [`OverlayEvent::Close`] when nothing is highlighted.
    #[must_use]
    pub fn accept(&self) -> OverlayEvent {
        match self {
            Self::Picker(p) => p.accepted().cloned().unwrap_or(OverlayEvent::Close),
            Self::RebaseTodo(form) => OverlayEvent::AcceptRebaseTodo {
                onto: form.onto.clone(),
                steps: form
                    .steps
                    .iter()
                    .map(|(action, hash, ..)| karet_vcs::RebaseStep {
                        action: *action,
                        rev: hash.clone(),
                    })
                    .collect(),
            },
            Self::CreateBranch(form) => OverlayEvent::AcceptCreateBranch(form.options()),
            Self::StashForm(form) => OverlayEvent::AcceptStash(form.options()),
            Self::Text(prompt) => OverlayEvent::AcceptText {
                purpose: prompt.purpose.clone(),
                text: prompt.text.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_filters_and_accept_opens() {
        let files = vec![
            ("app.rs".to_string(), PathBuf::from("/x/app.rs")),
            ("main.rs".to_string(), PathBuf::from("/x/main.rs")),
        ];
        let mut overlay = Overlay::quick_open(files);
        // Type "ma" -> only main.rs remains.
        overlay.push_char('m');
        overlay.push_char('a');
        assert_eq!(overlay.rows(), vec!["main.rs"]);
        match overlay.accept() {
            OverlayEvent::AcceptFile(p) => assert_eq!(p, PathBuf::from("/x/main.rs")),
            _ => unreachable!("accept opens the single match"),
        }
    }

    #[test]
    fn palette_accepts_a_command() {
        let mut overlay = Overlay::command_palette();
        // "quit" filters to the Quit command.
        for c in "quit".chars() {
            overlay.push_char(c);
        }
        match overlay.accept() {
            OverlayEvent::AcceptCommand(cmd) => assert_eq!(cmd, Command::Quit),
            _ => unreachable!("accept runs the filtered command"),
        }
    }

    #[test]
    fn diff_target_picker_filters_and_accepts_a_revision() {
        let items = vec![
            (
                "abc1234 first commit".to_string(),
                DiffTarget {
                    rev: "abc1234deadbeef".to_string(),
                    label: "abc1234".to_string(),
                },
            ),
            (
                "feature".to_string(),
                DiffTarget {
                    rev: "feature".to_string(),
                    label: "feature".to_string(),
                },
            ),
        ];
        let mut overlay = Overlay::diff_target("Open Changes: With Revision", items);
        assert_eq!(overlay.title(), "Open Changes: With Revision");
        for c in "feat".chars() {
            overlay.push_char(c);
        }
        assert_eq!(overlay.rows(), vec!["feature"]);
        match overlay.accept() {
            OverlayEvent::AcceptDiffTarget { rev, label } => {
                assert_eq!(rev, "feature");
                assert_eq!(label, "feature");
            },
            _ => unreachable!("accept picks the filtered revision"),
        }
    }

    #[test]
    fn palette_rows_have_aligned_hints() {
        let overlay = Overlay::command_palette();
        assert_eq!(overlay.rows().len(), overlay.row_hints().len());
        // The Quit row carries its Ctrl+Q hint.
        let quit = overlay
            .rows()
            .iter()
            .position(|r| *r == Command::Quit.label())
            .expect("quit row present");
        assert_eq!(overlay.row_hints()[quit].as_deref(), Some("Ctrl+Q"));
    }

    #[test]
    fn create_branch_form_exposes_every_common_control() {
        let mut overlay = Overlay::create_branch(vec!["origin".to_string()]);
        let rows = overlay.rows();
        assert!(rows.iter().any(|row| row.contains("Name")));
        assert!(rows.iter().any(|row| row.contains("Start point")));
        assert!(rows.iter().any(|row| row.contains("Switch now")));
        assert!(rows.iter().any(|row| row.contains("Publish remote")));
        assert!(rows.iter().any(|row| row.contains("Set upstream")));
        for character in "feature/test".chars() {
            overlay.push_char(character);
        }
        match overlay.accept() {
            OverlayEvent::AcceptCreateBranch(options) => {
                assert_eq!(options.name, "feature/test");
                assert_eq!(options.start_point, "HEAD");
                assert!(options.switch);
            },
            _ => unreachable!("branch form submits its options"),
        }
    }

    #[test]
    fn stash_form_edits_message_and_toggles_options() {
        let mut overlay = Overlay::stash_form();
        for character in "work".chars() {
            overlay.push_char(character);
        }
        overlay.select_down();
        overlay.push_char(' ');
        overlay.select_down();
        overlay.push_char(' ');
        match overlay.accept() {
            OverlayEvent::AcceptStash(options) => {
                assert_eq!(options.message.as_deref(), Some("work"));
                assert!(options.include_untracked);
                assert!(options.keep_index);
            },
            _ => unreachable!("stash form submits its options"),
        }
    }

    #[test]
    fn remote_branch_deletion_picker_preserves_remote_and_name() {
        let overlay =
            Overlay::delete_remote_branches(vec![("upstream".to_string(), "feature".to_string())]);
        match overlay.accept() {
            OverlayEvent::AcceptDeleteRemoteBranch { remote, branch } => {
                assert_eq!(remote, "upstream");
                assert_eq!(branch, "feature");
            },
            _ => unreachable!("remote branch picker submits both parts"),
        }
    }
}
