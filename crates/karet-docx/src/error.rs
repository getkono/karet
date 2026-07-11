//! The error type produced by karet-docx.

use thiserror::Error;

/// An error reading or parsing a DOCX document.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DocxError {
    /// The input bytes are not a valid ZIP container (a `.docx` is a ZIP of XML).
    #[error("not a DOCX: the bytes are not a valid ZIP archive")]
    NotAZip,
    /// The ZIP has no `word/document.xml` entry — it is not a Word document.
    #[error("not a DOCX: the archive has no word/document.xml entry")]
    MissingDocument,
    /// `word/document.xml` is not valid UTF-8.
    #[error("word/document.xml is not valid UTF-8")]
    NotUtf8,
    /// `word/document.xml` could not be parsed as XML.
    #[error("malformed XML in word/document.xml: {0}")]
    Xml(String),
}
