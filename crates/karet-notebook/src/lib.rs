//! `karet-notebook` — Jupyter notebook (nbformat 4) support for karet.
//!
//! Headless: [`parse`] reads an `.ipynb` into a neutral [`Notebook`] model
//! that **round-trips** — the `source` string-vs-string-list forms, unknown
//! fields, and metadata all survive [`to_json`] byte-for-byte at the JSON
//! value level — and [`to_markdown`] renders the model for karet's read-only
//! document preview. Hand-rolled over serde (the `nbformat` crate carries
//! `anyhow` in its public API and stale deps).
//!
//! No presentation, no kernel: execution arrives behind the optional kernel
//! work, and rendering is the consumer's.

mod convert;
mod model;

pub use convert::to_markdown;
pub use model::Cell;
pub use model::CellKind;
pub use model::Notebook;
pub use model::Output;
pub use model::Source;

/// Errors produced by notebook parsing and serialization.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum NotebookError {
    /// The input is not valid nbformat JSON.
    #[error("not a valid notebook: {0}")]
    Parse(String),
    /// The notebook declares a major version this crate does not read.
    #[error("unsupported nbformat version {0} (karet reads nbformat 4)")]
    UnsupportedVersion(u32),
    /// The model could not be serialized (pathological metadata).
    #[error("could not serialize the notebook: {0}")]
    Serialize(String),
}

/// Parse an `.ipynb` document (nbformat 4.x) into a [`Notebook`].
///
/// # Errors
/// Returns [`NotebookError::Parse`] for malformed JSON or shape, and
/// [`NotebookError::UnsupportedVersion`] for nbformat majors other than 4.
pub fn parse(input: &str) -> Result<Notebook, NotebookError> {
    let notebook: Notebook =
        serde_json::from_str(input).map_err(|error| NotebookError::Parse(error.to_string()))?;
    if notebook.nbformat != 4 {
        return Err(NotebookError::UnsupportedVersion(notebook.nbformat));
    }
    Ok(notebook)
}

/// Serialize a [`Notebook`] back to nbformat JSON (pretty-printed, like
/// Jupyter writes it).
///
/// # Errors
/// Returns [`NotebookError::Serialize`] if the model cannot be encoded.
pub fn to_json(notebook: &Notebook) -> Result<String, NotebookError> {
    serde_json::to_string_pretty(notebook)
        .map_err(|error| NotebookError::Serialize(error.to_string()))
}
