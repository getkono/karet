//! Exhaustive accessors over [`TabKind`]: each names every variant, so a new
//! kind cannot compile without saying how it scrolls and what it waits on.

use super::PagerState;
use super::TabKind;
use crate::pending::Pending;

impl TabKind {
    /// The two-axis scroll state of a read-only pager tab, or `None` for a kind
    /// that scrolls some other way (or not at all).
    #[must_use]
    pub(crate) fn pager_mut(&mut self) -> Option<&mut PagerState> {
        match self {
            Self::Diff { pager, .. }
            | Self::StashPreview { pager, .. }
            | Self::ServerLog { pager, .. }
            | Self::Graph { pager, .. }
            | Self::LoadedConfig { pager, .. }
            | Self::CommitLoading { pager, .. } => Some(pager),
            Self::Welcome
            | Self::LanguageServers(_)
            | Self::Code { .. }
            | Self::MarkdownPreview { .. }
            | Self::Hex { .. }
            | Self::Placeholder { .. }
            | Self::LatexPreview { .. }
            | Self::Seam(_)
            | Self::Commit { .. }
            | Self::Compare { .. }
            | Self::CommitGraph { .. } => None,
            #[cfg(feature = "images")]
            Self::Image { .. } => None,
            #[cfg(feature = "pdf")]
            Self::Document { .. } => None,
            #[cfg(feature = "terminal")]
            Self::Terminal(_) => None,
        }
    }

    /// The in-flight loads this tab's content is waiting on, whose delayed
    /// placeholders the event loop must schedule a reveal for. A code tab's
    /// wait for its text lives on the [`Tab`](super::Tab)
    /// ([`fill_since`](super::Tab::fill_since)), not here.
    pub(crate) fn pendings(&self) -> impl Iterator<Item = Pending> {
        let (first, second) = match self {
            Self::LanguageServers(view) => (view.loading_since, None),
            // Indexing takes seconds and a node's source is read from disk, so
            // both reveals are scheduled rather than left to the next keypress.
            Self::Seam(state) => (state.loading_since, state.detail_since),
            Self::CommitLoading {
                loading_since,
                error,
                ..
            }
            | Self::LatexPreview {
                loading_since,
                error,
                ..
            } => (error.is_none().then_some(*loading_since), None),
            Self::MarkdownPreview { pending_since, .. } => (*pending_since, None),
            Self::ServerLog { loading_since, .. } | Self::CommitGraph { loading_since, .. } => {
                (*loading_since, None)
            },
            Self::Commit { files, .. } | Self::Compare { files, .. } => (files.loading_since, None),
            Self::Diff {
                file,
                loading_since,
                error,
                ..
            } => (
                loading_since.filter(|_| file.is_none() && error.is_none()),
                None,
            ),
            Self::Welcome
            | Self::Code { .. }
            | Self::Hex { .. }
            | Self::Placeholder { .. }
            | Self::StashPreview { .. }
            | Self::Graph { .. }
            | Self::LoadedConfig { .. } => (None, None),
            #[cfg(feature = "images")]
            Self::Image { .. } => (None, None),
            #[cfg(feature = "pdf")]
            Self::Document { .. } => (None, None),
            #[cfg(feature = "terminal")]
            Self::Terminal(_) => (None, None),
        };
        [first, second].into_iter().flatten()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use karet_session::LanguageServerId;

    use super::super::Tab;
    use super::*;

    #[test]
    fn pager_mut_reaches_pager_kinds_only() {
        let mut stash = Tab::stash_preview("stash@{0}", String::new());
        if let Some(pager) = stash.kind.pager_mut() {
            pager.scroll = 7;
        }
        assert_eq!(stash.kind.pager_mut().map(|pager| pager.scroll), Some(7));
        assert!(Tab::commit_loading("abc").kind.pager_mut().is_some());
        assert!(Tab::welcome().kind.pager_mut().is_none());
        let mut latex = Tab::latex_preview(PathBuf::from("/x/a.tex"));
        assert!(latex.kind.pager_mut().is_none());
    }

    #[test]
    fn pendings_follow_a_load_until_it_fails() {
        let mut loading = Tab::commit_loading("abc");
        assert_eq!(loading.kind.pendings().count(), 1);
        if let TabKind::CommitLoading { error, .. } = &mut loading.kind {
            *error = Some("bad revision".into());
        }
        assert_eq!(loading.kind.pendings().count(), 0);

        let mut latex = Tab::latex_preview(PathBuf::from("/x/a.tex"));
        assert_eq!(latex.kind.pendings().count(), 1);
        if let TabKind::LatexPreview { error, .. } = &mut latex.kind {
            *error = Some("no compiler".into());
        }
        assert_eq!(latex.kind.pendings().count(), 0);

        let log = Tab::server_log(LanguageServerId::new("rust-analyzer"));
        assert_eq!(log.kind.pendings().count(), 1);
        assert_eq!(Tab::welcome().kind.pendings().count(), 0);
        let stash = Tab::stash_preview("stash@{0}", String::new());
        assert_eq!(stash.kind.pendings().count(), 0);
    }
}
