use super::*;

/// The palette commands, in display order.
#[must_use]
pub fn palette() -> Vec<Command> {
    [
        Command::OpenQuickOpen,
        Command::SelectPanel(SidebarPanel::Explorer),
        Command::SelectPanel(SidebarPanel::Search),
        Command::SelectPanel(SidebarPanel::SourceControl),
        Command::ToggleSidebar,
        Command::ToggleOutline,
        Command::ToggleFocus,
        Command::OpenFind,
        Command::OpenGlobalSearch,
        Command::TriggerCompletion,
        Command::ToggleInlineBlame,
        Command::ShowCommitGraph,
        Command::OpenCommitByHash,
        Command::ShowFileHistory,
        Command::DiffUnpushed,
        Command::DiffSinceBase,
        Command::OpenBlameDetail,
        Command::ShowDependencyGraph,
        Command::ShowLoadedConfig,
        Command::CheckLanguageServerUpdates,
        Command::ExplorerNewFile,
        Command::ExplorerNewFolder,
        Command::ExplorerRename,
        Command::ExplorerRefresh,
        Command::ExplorerCollapseAll,
        Command::ExplorerCopy,
        Command::ExplorerCut,
        Command::ExplorerPaste,
        Command::ExplorerDuplicate,
        Command::ExplorerDelete,
        Command::ExplorerCopyPath,
        Command::ExplorerCopyRelativePath,
        Command::Copy,
        Command::CopyPath,
        Command::CopyRelativePath,
        Command::RevealActiveInExplorer,
        Command::CopyRemoteFileUrl,
        Command::CopyGithubPermalink,
        Command::CopyGithubHeadLink,
        Command::OpenChangesWithPrevious,
        Command::OpenChangesWithRevision,
        Command::OpenChangesWithBranch,
        Command::NextTab,
        Command::PrevTab,
        Command::MoveTabLeft,
        Command::MoveTabRight,
        Command::CloseTab,
        Command::CloseOtherTabs,
        Command::CloseTabsToRight,
        Command::CloseAllTabs,
        Command::ReopenClosedTab,
        Command::Save,
        Command::Undo,
        Command::Redo,
        Command::Cut,
        Command::Paste,
        Command::ToggleDiffLayout,
        Command::ToggleFold,
        Command::AddCursorAbove,
        Command::AddCursorBelow,
        Command::AddCursorNextOccurrence,
        Command::ScmStageAll,
        Command::ScmUnstageAll,
        Command::ScmCommit,
        Command::ScmRefresh,
        Command::MarkdownPreviewSide,
        Command::SplitRight,
        Command::SplitDown,
        Command::FocusNextPane,
        Command::FocusPrevPane,
        Command::DismissNotification,
        Command::DismissAllNotifications,
        Command::Quit,
    ]
    .into_iter()
    .filter(|c| c.in_palette())
    .collect()
}

/// Why a `--command` name failed to resolve against the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveNamedError {
    /// No palette command's title or slug matches the name.
    Unknown {
        /// The name as given on the command line.
        name: String,
        /// The closest palette titles, best first (may be empty).
        suggestions: Vec<&'static str>,
    },
    /// The name is a slug shared by more than one palette command.
    Ambiguous {
        /// The name as given on the command line.
        name: String,
        /// The titles of every command carrying that slug.
        candidates: Vec<&'static str>,
    },
}

impl std::fmt::Display for ResolveNamedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown { name, suggestions } => {
                write!(f, "unknown command {name:?}")?;
                if !suggestions.is_empty() {
                    write!(f, "; did you mean: {}?", suggestions.join(", "))?;
                }
                Ok(())
            },
            Self::Ambiguous { name, candidates } => write!(
                f,
                "{name:?} matches more than one command: {}; use the full title",
                candidates.join(", ")
            ),
        }
    }
}

impl std::error::Error for ResolveNamedError {}

/// Resolve a `--command` name to a palette command: a case-insensitive **exact**
/// match on a command's title ([`Command::label`], e.g. "Source Control: Commit
/// Graph") or its short slug ([`Command::hint_verb`], e.g. "graph"). Titles are
/// unique; a slug shared by several commands is [`ResolveNamedError::Ambiguous`],
/// and anything else is [`ResolveNamedError::Unknown`] with the closest titles as
/// suggestions.
///
/// # Errors
/// Returns [`ResolveNamedError`] when the name matches no palette command exactly
/// or matches a slug carried by more than one.
pub fn resolve_named(name: &str) -> Result<Command, ResolveNamedError> {
    let query = name.trim().to_lowercase();
    let commands = palette();
    if let Some(cmd) = commands.iter().find(|c| c.label().to_lowercase() == query) {
        return Ok(*cmd);
    }
    let slug_matches: Vec<Command> = commands
        .iter()
        .copied()
        .filter(|c| c.hint_verb().is_some_and(|v| v == query))
        .collect();
    match slug_matches.as_slice() {
        [cmd] => Ok(*cmd),
        [] => Err(ResolveNamedError::Unknown {
            name: name.to_string(),
            suggestions: suggest(&query, &commands),
        }),
        many => Err(ResolveNamedError::Ambiguous {
            name: name.to_string(),
            candidates: many.iter().map(|c| c.label()).collect(),
        }),
    }
}

/// The number of suggestions [`resolve_named`] offers for an unknown name.
pub(super) const MAX_SUGGESTIONS: usize = 3;

/// The closest palette titles to `query` (already lowercased), best first: ranked
/// by the [substring edit distance](substring_distance) of the query to the title
/// or its slug (an exactly-contained query scores 0), tie-broken on the title for
/// determinism. Titles further than the cutoff — half the query length — are
/// suppressed: a wall of unrelated suggestions is worse than none.
pub(super) fn suggest(query: &str, commands: &[Command]) -> Vec<&'static str> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, &'static str)> = commands
        .iter()
        .map(|c| {
            let label = c.label();
            let by_label = substring_distance(query, &label.to_lowercase());
            let by_slug = c
                .hint_verb()
                .map_or(usize::MAX, |slug| substring_distance(query, slug));
            (by_label.min(by_slug), label)
        })
        .collect();
    scored.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)));
    let cutoff = (query.chars().count() / 2).max(2);
    scored
        .into_iter()
        .filter(|(score, _)| *score <= cutoff)
        .take(MAX_SUGGESTIONS)
        .map(|(_, label)| label)
        .collect()
}

/// The minimal Levenshtein edit distance (insert / delete / substitute, all cost 1,
/// over `char`s) between `pattern` and **any substring** of `text` — the classic
/// semi-global alignment, with a free start and end in `text`. A contained pattern
/// scores 0, and a typo of a phrase inside a long title scores just its own edits,
/// not the length of the rest of the title. Small inputs only — palette titles.
pub(super) fn substring_distance(pattern: &str, text: &str) -> usize {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    // Row 0 is all zeros: the match may begin at any position in `text` for free.
    let mut prev = vec![0usize; t.len() + 1];
    let mut cur = vec![0usize; t.len() + 1];
    for (i, pc) in p.iter().enumerate() {
        cur[0] = i + 1;
        for (j, tc) in t.iter().enumerate() {
            let sub = prev[j] + usize::from(pc != tc);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    // The match may end at any position in `text` for free: take the best cell.
    prev.into_iter().min().unwrap_or(0)
}
