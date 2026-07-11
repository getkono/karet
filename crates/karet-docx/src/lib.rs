//! `karet-docx` — DOCX support for karet editors.
//!
//! A Word `.docx` is an OOXML package: a ZIP container whose `word/document.xml`
//! holds the body as WordprocessingML. This crate reads that package with a
//! **hand-rolled minimal reader** built on two lean, pure-Rust dependencies — `zip`
//! (trimmed to deflate-only, so no C-backed `bzip2`/`zstd` enters the build) and
//! the streaming `quick-xml` parser — instead of a heavy third-party DOCX crate.
//!
//! It exposes two layers:
//!
//! - [`parse`] turns the bytes into a small, **neutral** [`Document`] model
//!   ([`Block`] / [`ParaStyle`] / [`Span`]) that captures only what is needed to
//!   *display* a document: block structure and the emphases markdown can express.
//! - [`to_markdown`] projects that model to markdown text.
//!
//! ## Why markdown
//! karet already has a full markdown render pipeline (`karet-markdown`), so DOCX
//! display is simply DOCX → markdown → that existing preview machinery. The model
//! is kept neutral (not markdown-specific) so a future higher-fidelity renderer can
//! consume it directly rather than round-tripping through markdown.
//!
//! ```
//! # fn demo(docx_bytes: &[u8]) -> Result<(), karet_docx::DocxError> {
//! let document = karet_docx::parse(docx_bytes)?;
//! let markdown = karet_docx::to_markdown(&document);
//! # let _ = markdown;
//! # Ok(())
//! # }
//! ```
//!
//! See [`parse`] for the deliberate simplifications (best-effort list-ordering,
//! hyperlink resolution, image placeholders) and [`to_markdown`] for the mapping
//! (notably: underline has no markdown form and passes through as plain text).

mod error;
mod markdown;
mod model;
mod parse;

pub use error::DocxError;
pub use markdown::to_markdown;
pub use model::Block;
pub use model::Document;
pub use model::ParaStyle;
pub use model::Span;
pub use parse::parse;
