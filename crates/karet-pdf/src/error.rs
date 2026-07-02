//! The error type surfaced by PDF loading and rendering.

use hayro::hayro_syntax::LoadPdfError;

/// An error loading or rasterizing a PDF document.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PdfError {
    /// The bytes could not be parsed as a PDF document.
    #[error("could not parse PDF document")]
    Parse,
    /// The document is encrypted / password-protected, which is not supported.
    #[error("PDF document is encrypted (password-protected)")]
    Encrypted,
    /// The requested page index is out of range for the document.
    #[error("page {index} is out of range (document has {count} page(s))")]
    PageOutOfRange {
        /// The requested 0-based page index.
        index: usize,
        /// The number of pages the document actually has.
        count: usize,
    },
}

impl PdfError {
    /// Map a `hayro` load error into a [`PdfError`].
    pub(crate) fn from_load(err: LoadPdfError) -> Self {
        match err {
            LoadPdfError::Decryption(_) => Self::Encrypted,
            LoadPdfError::Invalid => Self::Parse,
        }
    }
}
