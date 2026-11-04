//! The Seam view's app side: the commands and pointer routing that drive its
//! model, and the package discovery behind its root picker.
//!
//! The model itself — [`SeamViewState`](crate::tab::seam::SeamViewState) and
//! its navigation — is tab content, so it lives in [`crate::tab::seam`].

mod actions;
mod mouse;
pub(crate) mod roots;

#[cfg(test)]
mod tests;
