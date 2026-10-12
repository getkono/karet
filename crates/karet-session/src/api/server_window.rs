//! What a running language server says to the user: work it reports progress
//! on, questions it asks, and the log it keeps.
//!
//! Neutral, serde-ready mirrors of `karet-lsp`'s window types, so a client
//! across the [`Backend`](crate::Backend) seam never names `karet-lsp`.

use std::path::PathBuf;

use karet_core::Severity;

use super::LanguageServerId;

/// The server's name for one piece of work, handed back to cancel it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum WorkToken {
    /// A numeric token.
    Number(i32),
    /// A string token.
    Text(String),
}

/// Which step of a piece of work a [`LanguageServerWork`] reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum WorkPhase {
    /// The work started; carries its title.
    Begin,
    /// The work moved on.
    Report,
    /// The work finished or was cancelled. Nothing follows for its token.
    End,
}

/// One progress report for work a language server is doing (`$/progress`).
///
/// A client tracks work by `(server, root, token)`: a `Begin`, any number of
/// `Report`s, then an `End`. Work also ends, without an `End`, when its
/// server's connection does -- [`Event::LanguageServerRuntimeChanged`] to any
/// state but `Running` -- so a client clears it then too.
///
/// [`Event::LanguageServerRuntimeChanged`]: crate::Event::LanguageServerRuntimeChanged
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LanguageServerWork {
    /// The provider doing the work.
    pub server: LanguageServerId,
    /// The repository root its connection serves.
    pub root: PathBuf,
    /// The server's name for the work, for [`Command::CancelLanguageServerWork`].
    ///
    /// [`Command::CancelLanguageServerWork`]: crate::Command::CancelLanguageServerWork
    pub token: WorkToken,
    /// Which step this is.
    pub phase: WorkPhase,
    /// What the work is (`Indexing`), on `Begin` only, reduced to one
    /// printable line.
    pub title: Option<String>,
    /// What it is doing now, when the server says, reduced to one printable
    /// line.
    pub message: Option<String>,
    /// How far along, 0–100, when the server can say.
    pub percentage: Option<u32>,
    /// Whether the user may cancel it; `None` leaves the last answer standing.
    pub cancellable: Option<bool>,
}

/// Identifies a language server's pending question
/// ([`Event::LanguageServerMessageRequest`]) until it is answered.
///
/// [`Event::LanguageServerMessageRequest`]: crate::Event::LanguageServerMessageRequest
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct LanguageServerMessageId(pub u64);

/// One line of a language server's log (`window/logMessage`, and every
/// `window/showMessage` too, so the log is the complete record).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LanguageServerLogEntry {
    /// The repository root of the connection that logged it.
    pub root: PathBuf,
    /// How urgent the server said it was. Log and debug detail read as
    /// [`Severity::Hint`].
    pub severity: Severity,
    /// The text, its lines kept but made safe to paint: control and
    /// invisible formatting characters (escape sequences, bidi overrides,
    /// tabs) become spaces.
    pub text: String,
}
