//! Repository discovery and the error-mapping helpers shared by the other modules.

use crate::{Repository, VcsError};
use std::path::Path;

/// Map any error that implements [`std::fmt::Display`] into [`VcsError::Git`].
pub(crate) fn to_git<E: std::fmt::Display>(e: E) -> VcsError {
    VcsError::Git(e.to_string())
}

/// Map a discovery error: "no repository found" becomes [`VcsError::NotARepository`];
/// anything else (e.g. an inaccessible directory) becomes [`VcsError::Git`].
fn map_discover(e: gix::discover::Error) -> VcsError {
    use gix::discover::upwards::Error as U;
    match e {
        gix::discover::Error::Discover(
            U::NoGitRepository { .. }
            | U::NoGitRepositoryWithinCeiling { .. }
            | U::NoGitRepositoryWithinFs { .. }
            | U::NoMatchingCeilingDir
            | U::NoTrustedGitRepository { .. },
        ) => VcsError::NotARepository,
        other => VcsError::Git(other.to_string()),
    }
}

impl Repository {
    /// Discover the repository containing `path`, searching upwards through parents.
    ///
    /// # Errors
    /// Returns [`VcsError::NotARepository`] if no repository is found, or
    /// [`VcsError::Git`] for any other discovery failure.
    pub fn discover(path: &Path) -> Result<Self, VcsError> {
        let inner = gix::discover(path).map_err(map_discover)?;
        Ok(Self { inner })
    }
}
