//! Centered modal overlays: quick-open (go to file), the command palette, and the
//! diff-target picker (a revision or branch to diff the active file against).
//!
//! Each is a [`Picker`] over labeled items with an incremental subsequence filter.
//! (The richer `karet-fuzzy` ranking / `karet-widgets::Picker` widget is a future
//! home; this keeps the skeleton dependency-light.)

use std::path::PathBuf;

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
pub enum OverlayEvent {
    /// Nothing was highlighted; dismiss the overlay.
    Close,
    /// Open the chosen file.
    AcceptFile(PathBuf),
    /// Run the chosen command.
    AcceptCommand(Command),
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
    /// Save every dirty editor, then switch branches.
    SaveAndSwitch { target: BranchTarget },
    /// Stash worktree changes and retry a refused branch switch.
    StashAndSwitch { target: BranchTarget },
    /// Confirm permanent stash removal by typing `drop`.
    ConfirmDropStash { reference: String },
    /// Confirm undoing a commit already present upstream by typing `undo`.
    ConfirmPublishedUndo,
    /// Rename `old` to the submitted name.
    RenameBranch { old: String },
    /// Confirm remote deletion by typing the exact branch name.
    ConfirmDeleteRemoteBranch { remote: String, branch: String },
    /// Confirm opening a relative file link that escaped the workspace.
    ConfirmOutsideWorkspaceLink { path: PathBuf },
    /// Confirm creating the missing project settings file before adding a word.
    ConfirmCreateProjectSettings { word: String, path: PathBuf },
    /// Confirm the first network-backed installation by typing `install`.
    InstallLanguageServer { server: LanguageServerId },
    /// Approve the exact update plan displayed by the backend.
    ApplyLanguageServerPlan { plan: LanguageServerPlanId },
    /// Restart a session-local process after an installed update.
    RestartLanguageServer { server: LanguageServerId },
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

/// An incremental picker over labeled items of type `T`.
pub struct Picker<T> {
    title: String,
    query: String,
    items: Vec<(String, T)>,
    filtered: Vec<usize>,
    selected: usize,
}

impl<T> Picker<T> {
    /// Build a picker titled `title` over `items` (label + value).
    fn new(title: impl Into<String>, items: Vec<(String, T)>) -> Self {
        let filtered = (0..items.len()).collect();
        Self {
            title: title.into(),
            query: String::new(),
            items,
            filtered,
            selected: 0,
        }
    }

    /// The picker title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The current query string.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The visible (filtered) row labels, in order.
    #[must_use]
    pub fn rows(&self) -> Vec<&str> {
        self.filtered
            .iter()
            .map(|&i| self.items[i].0.as_str())
            .collect()
    }

    /// The visible (filtered) row values, in order.
    fn values(&self) -> Vec<&T> {
        self.filtered.iter().map(|&i| &self.items[i].1).collect()
    }

    /// The selected row index within the filtered list.
    #[must_use]
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Recompute the filtered list for the current query.
    fn refilter(&mut self) {
        let needle = self.query.to_ascii_lowercase();
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, (label, _))| subsequence(&needle, &label.to_ascii_lowercase()))
            .map(|(i, _)| i)
            .collect();
        self.selected = 0;
    }

    /// The currently-selected value, if any.
    fn accepted(&self) -> Option<&T> {
        self.filtered.get(self.selected).map(|&i| &self.items[i].1)
    }

    /// Move the selection up.
    fn select_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Move the selection down, clamped to the filtered list.
    fn select_down(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1).min(self.filtered.len() - 1);
        }
    }

    /// Append a character to the query and refilter.
    fn push_char(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    /// Remove the last query character and refilter.
    fn pop_char(&mut self) {
        self.query.pop();
        self.refilter();
    }

    /// Append pasted text to the query and refilter.
    fn push_str(&mut self, text: &str) {
        self.query.push_str(text);
        self.refilter();
    }
}

/// A modal overlay.
pub enum Overlay {
    /// Quick-open: pick a file to open.
    QuickOpen(Picker<PathBuf>),
    /// Command palette: pick a command to run.
    CommandPalette(Picker<Command>),
    /// Diff-target picker: pick a revision or branch to diff the active file against.
    DiffTarget(Picker<DiffTarget>),
    /// Existing local and remote branches.
    Branch(Picker<BranchTarget>),
    /// Full create-branch form.
    CreateBranch(BranchForm),
    /// Open GitHub pull requests.
    PullRequest {
        remote: String,
        picker: Picker<PullRequestSummary>,
    },
    /// Stash creation form.
    StashForm(StashForm),
    /// Actions over existing stash entries.
    Stash(Picker<StashAction>),
    /// Single-value follow-up prompt.
    Text(TextPrompt),
    /// Local branches eligible for safe deletion.
    DeleteLocalBranch(Picker<String>),
    /// Remote branches eligible for typed-confirmation deletion.
    DeleteRemoteBranch(Picker<(String, String)>),
}

impl Overlay {
    /// Build a quick-open overlay over `(display, path)` pairs.
    #[must_use]
    pub fn quick_open(files: Vec<(String, PathBuf)>) -> Self {
        Self::QuickOpen(Picker::new("Go to File", files))
    }

    /// Build the command palette.
    #[must_use]
    pub fn command_palette() -> Self {
        let items = command::palette()
            .into_iter()
            .map(|cmd| (cmd.label().to_string(), cmd))
            .collect();
        Self::CommandPalette(Picker::new("Command Palette", items))
    }

    /// Build a command picker over an explicit action subset.
    #[must_use]
    pub fn commands(title: impl Into<String>, commands: Vec<Command>) -> Self {
        let items = commands
            .into_iter()
            .map(|command| (command.label().to_string(), command))
            .collect();
        Self::CommandPalette(Picker::new(title, items))
    }

    /// Build a diff-target picker titled `title` over `(display, target)` pairs.
    #[must_use]
    pub fn diff_target(title: impl Into<String>, items: Vec<(String, DiffTarget)>) -> Self {
        Self::DiffTarget(Picker::new(title, items))
    }

    /// Build a branch picker.
    #[must_use]
    pub fn branches(items: Vec<(String, BranchTarget)>) -> Self {
        Self::Branch(Picker::new("Switch branch", items))
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
                    item,
                )
            })
            .collect();
        Self::PullRequest {
            remote,
            picker: Picker::new("Open pull requests", rows),
        }
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
            items.push((
                format!("Preview   {base}"),
                StashAction::Preview(reference.clone()),
            ));
            items.push((
                format!("Apply     {base}"),
                StashAction::Apply(reference.clone()),
            ));
            items.push((
                format!("Pop       {base}"),
                StashAction::Pop(reference.clone()),
            ));
            items.push((
                format!("Branch…   {base}"),
                StashAction::Branch(reference.clone()),
            ));
            items.push((format!("Drop      {base}"), StashAction::Drop(reference)));
        }
        Self::Stash(Picker::new("Manage stashes", items))
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

    /// Build a local-branch deletion picker.
    #[must_use]
    pub fn delete_local_branches(items: Vec<String>) -> Self {
        let rows = items.into_iter().map(|name| (name.clone(), name)).collect();
        Self::DeleteLocalBranch(Picker::new("Delete local branch", rows))
    }

    /// Build a remote-branch deletion picker.
    #[must_use]
    pub fn delete_remote_branches(items: Vec<(String, String)>) -> Self {
        let rows = items
            .into_iter()
            .map(|(remote, branch)| (format!("{remote}/{branch}"), (remote, branch)))
            .collect();
        Self::DeleteRemoteBranch(Picker::new("Delete remote branch", rows))
    }

    /// The overlay title.
    #[must_use]
    pub fn title(&self) -> &str {
        match self {
            Self::QuickOpen(p) => p.title(),
            Self::CommandPalette(p) => p.title(),
            Self::DiffTarget(p) => p.title(),
            Self::Branch(p) => p.title(),
            Self::CreateBranch(_) => "Create branch · ↑↓ fields · Space toggles",
            Self::PullRequest { picker, .. } => picker.title(),
            Self::StashForm(_) => "Stash changes · ↑↓ fields · Space toggles",
            Self::Stash(p) => p.title(),
            Self::Text(prompt) => &prompt.title,
            Self::DeleteLocalBranch(p) => p.title(),
            Self::DeleteRemoteBranch(p) => p.title(),
        }
    }

    /// The current query string.
    #[must_use]
    pub fn query(&self) -> &str {
        match self {
            Self::QuickOpen(p) => p.query(),
            Self::CommandPalette(p) => p.query(),
            Self::DiffTarget(p) => p.query(),
            Self::Branch(p) => p.query(),
            Self::CreateBranch(_) | Self::StashForm(_) => "Edit selected field",
            Self::PullRequest { picker, .. } => picker.query(),
            Self::Stash(p) => p.query(),
            Self::Text(prompt) => &prompt.text,
            Self::DeleteLocalBranch(p) => p.query(),
            Self::DeleteRemoteBranch(p) => p.query(),
        }
    }

    /// The visible row labels.
    #[must_use]
    pub fn rows(&self) -> Vec<&str> {
        match self {
            Self::QuickOpen(p) => p.rows(),
            Self::CommandPalette(p) => p.rows(),
            Self::DiffTarget(p) => p.rows(),
            Self::Branch(p) => p.rows(),
            Self::CreateBranch(form) => form.rows.iter().map(String::as_str).collect(),
            Self::PullRequest { picker, .. } => picker.rows(),
            Self::StashForm(form) => form.rows.iter().map(String::as_str).collect(),
            Self::Stash(p) => p.rows(),
            Self::Text(_) => Vec::new(),
            Self::DeleteLocalBranch(p) => p.rows(),
            Self::DeleteRemoteBranch(p) => p.rows(),
        }
    }

    /// The per-row right-aligned hints (key chords), aligned with [`rows`](Self::rows).
    /// Only command-palette rows carry hints.
    #[must_use]
    pub fn row_hints(&self) -> Vec<Option<String>> {
        match self {
            Self::QuickOpen(p) => p.rows().iter().map(|_| None).collect(),
            Self::CommandPalette(p) => p
                .values()
                .into_iter()
                .map(|cmd| keymap::hint_for(*cmd, keymap::ChordStyle::Verbose))
                .collect(),
            Self::DiffTarget(p) => p.rows().iter().map(|_| None).collect(),
            Self::Branch(p) => p.rows().iter().map(|_| None).collect(),
            Self::CreateBranch(form) => form.rows.iter().map(|_| None).collect(),
            Self::PullRequest { picker, .. } => picker.rows().iter().map(|_| None).collect(),
            Self::StashForm(form) => form.rows.iter().map(|_| None).collect(),
            Self::Stash(p) => p.rows().iter().map(|_| None).collect(),
            Self::Text(_) => Vec::new(),
            Self::DeleteLocalBranch(p) => p.rows().iter().map(|_| None).collect(),
            Self::DeleteRemoteBranch(p) => p.rows().iter().map(|_| None).collect(),
        }
    }

    /// The selected row index.
    #[must_use]
    pub fn selected(&self) -> usize {
        match self {
            Self::QuickOpen(p) => p.selected(),
            Self::CommandPalette(p) => p.selected(),
            Self::DiffTarget(p) => p.selected(),
            Self::Branch(p) => p.selected(),
            Self::CreateBranch(form) => form.selected,
            Self::PullRequest { picker, .. } => picker.selected(),
            Self::StashForm(form) => form.selected,
            Self::Stash(p) => p.selected(),
            Self::Text(_) => 0,
            Self::DeleteLocalBranch(p) => p.selected(),
            Self::DeleteRemoteBranch(p) => p.selected(),
        }
    }

    /// Move the selection up.
    pub fn select_up(&mut self) {
        match self {
            Self::QuickOpen(p) => p.select_up(),
            Self::CommandPalette(p) => p.select_up(),
            Self::DiffTarget(p) => p.select_up(),
            Self::Branch(p) => p.select_up(),
            Self::CreateBranch(form) => form.selected = form.selected.saturating_sub(1),
            Self::PullRequest { picker, .. } => picker.select_up(),
            Self::StashForm(form) => form.selected = form.selected.saturating_sub(1),
            Self::Stash(p) => p.select_up(),
            Self::Text(_) => {},
            Self::DeleteLocalBranch(p) => p.select_up(),
            Self::DeleteRemoteBranch(p) => p.select_up(),
        }
    }

    /// Move the selection down.
    pub fn select_down(&mut self) {
        match self {
            Self::QuickOpen(p) => p.select_down(),
            Self::CommandPalette(p) => p.select_down(),
            Self::DiffTarget(p) => p.select_down(),
            Self::Branch(p) => p.select_down(),
            Self::CreateBranch(form) => form.selected = (form.selected + 1).min(4),
            Self::PullRequest { picker, .. } => picker.select_down(),
            Self::StashForm(form) => form.selected = (form.selected + 1).min(2),
            Self::Stash(p) => p.select_down(),
            Self::Text(_) => {},
            Self::DeleteLocalBranch(p) => p.select_down(),
            Self::DeleteRemoteBranch(p) => p.select_down(),
        }
    }

    /// Append a character to the query.
    pub fn push_char(&mut self, c: char) {
        match self {
            Self::QuickOpen(p) => p.push_char(c),
            Self::CommandPalette(p) => p.push_char(c),
            Self::DiffTarget(p) => p.push_char(c),
            Self::Branch(p) => p.push_char(c),
            Self::CreateBranch(form) => form.push_char(c),
            Self::PullRequest { picker, .. } => picker.push_char(c),
            Self::StashForm(form) => form.push_char(c),
            Self::Stash(p) => p.push_char(c),
            Self::Text(prompt) => prompt.text.push(c),
            Self::DeleteLocalBranch(p) => p.push_char(c),
            Self::DeleteRemoteBranch(p) => p.push_char(c),
        }
    }

    /// Remove the last query character.
    pub fn pop_char(&mut self) {
        match self {
            Self::QuickOpen(p) => p.pop_char(),
            Self::CommandPalette(p) => p.pop_char(),
            Self::DiffTarget(p) => p.pop_char(),
            Self::Branch(p) => p.pop_char(),
            Self::CreateBranch(form) => form.pop_char(),
            Self::PullRequest { picker, .. } => picker.pop_char(),
            Self::StashForm(form) => {
                if form.selected == 0 {
                    form.message.pop();
                    form.refresh();
                }
            },
            Self::Stash(p) => p.pop_char(),
            Self::Text(prompt) => {
                prompt.text.pop();
            },
            Self::DeleteLocalBranch(p) => p.pop_char(),
            Self::DeleteRemoteBranch(p) => p.pop_char(),
        }
    }

    /// Append pasted text to the query.
    pub fn push_str(&mut self, text: &str) {
        match self {
            Self::QuickOpen(p) => p.push_str(text),
            Self::CommandPalette(p) => p.push_str(text),
            Self::DiffTarget(p) => p.push_str(text),
            Self::Branch(p) => p.push_str(text),
            Self::CreateBranch(form) => {
                for character in text.chars() {
                    form.push_char(character);
                }
            },
            Self::PullRequest { picker, .. } => picker.push_str(text),
            Self::StashForm(form) => {
                if form.selected == 0 {
                    form.message.push_str(text);
                    form.refresh();
                }
            },
            Self::Stash(p) => p.push_str(text),
            Self::Text(prompt) => prompt.text.push_str(text),
            Self::DeleteLocalBranch(p) => p.push_str(text),
            Self::DeleteRemoteBranch(p) => p.push_str(text),
        }
    }

    /// The outcome of accepting the highlighted row (open a file / run a command /
    /// diff against a revision), or [`OverlayEvent::Close`] when nothing is
    /// highlighted.
    #[must_use]
    pub fn accept(&self) -> OverlayEvent {
        match self {
            Self::QuickOpen(p) => p
                .accepted()
                .cloned()
                .map_or(OverlayEvent::Close, OverlayEvent::AcceptFile),
            Self::CommandPalette(p) => p
                .accepted()
                .copied()
                .map_or(OverlayEvent::Close, OverlayEvent::AcceptCommand),
            Self::DiffTarget(p) => p.accepted().cloned().map_or(OverlayEvent::Close, |target| {
                OverlayEvent::AcceptDiffTarget {
                    rev: target.rev,
                    label: target.label,
                }
            }),
            Self::Branch(p) => p
                .accepted()
                .cloned()
                .map_or(OverlayEvent::Close, OverlayEvent::AcceptBranch),
            Self::CreateBranch(form) => OverlayEvent::AcceptCreateBranch(form.options()),
            Self::PullRequest { remote, picker } => {
                picker.accepted().map_or(OverlayEvent::Close, |item| {
                    OverlayEvent::AcceptPullRequest {
                        remote: remote.clone(),
                        number: item.number,
                    }
                })
            },
            Self::StashForm(form) => OverlayEvent::AcceptStash(form.options()),
            Self::Stash(p) => p
                .accepted()
                .cloned()
                .map_or(OverlayEvent::Close, OverlayEvent::AcceptStashAction),
            Self::Text(prompt) => OverlayEvent::AcceptText {
                purpose: prompt.purpose.clone(),
                text: prompt.text.clone(),
            },
            Self::DeleteLocalBranch(p) => p
                .accepted()
                .cloned()
                .map_or(OverlayEvent::Close, OverlayEvent::AcceptDeleteLocalBranch),
            Self::DeleteRemoteBranch(p) => p
                .accepted()
                .cloned()
                .map_or(OverlayEvent::Close, |(remote, branch)| {
                    OverlayEvent::AcceptDeleteRemoteBranch { remote, branch }
                }),
        }
    }
}

/// Whether `needle` is a subsequence of `hay` (both already lowercased).
fn subsequence(needle: &str, hay: &str) -> bool {
    let mut chars = hay.chars();
    needle.chars().all(|c| chars.any(|h| h == c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_matches_in_order() {
        assert!(subsequence("ap", "app.rs"));
        assert!(subsequence("ars", "app.rs"));
        assert!(!subsequence("rsa", "app.rs"));
    }

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
