//! The vocabulary spoken between the session actor and its server tasks.
//!
//! [`ServerCmd`] travels down to the task serving one slot (a provider at a
//! repository root, see [`SlotKey`]); [`LspUpdate`] travels
//! back. Both are plain data on purpose: the task owns the `LspClient` and the
//! protocol, while position conversion (LSP UTF-16 ↔ buffer UTF-32) and event
//! emission happen on the actor, where the buffer lives.

use std::path::PathBuf;
use std::time::Duration;

use karet_core::CompletionItem;
use karet_core::CompletionList;
use karet_core::CompletionOptions;
use karet_core::CompletionTrigger;
use karet_core::Diagnostic;
use karet_core::HierarchyDirection;
use karet_core::HierarchyItem;
use karet_core::Hover;
use karet_core::InlayHint;
use karet_core::InlineCompletionItem;
use karet_core::LineCol;
use karet_core::Location;
use karet_core::Range;
use karet_core::ServerFeature;
use karet_core::SignatureHelp;
use karet_core::SignatureHelpOptions;
use karet_core::Symbol;
use karet_core::TextEdit;
use karet_core::WorkspaceEdit;
use karet_core::WorkspaceSymbol;
use karet_lsp::Indentation;

use super::code_actions::CodeActionAnswer;
use super::code_actions::CodeActionCmd;
use super::code_lens::CodeLensAnswer;
use super::code_lens::CodeLensCmd;
use super::delta::DocumentDelta;
use super::inline_completion::InlineCompletionCmd;
use super::link_color::LinkColorAnswer;
use super::link_color::LinkColorCmd;
use super::slot::SlotKey;
use super::slot::SlotToken;
use super::window::Notice;
use super::window::WindowUpdate;
use crate::api::DocumentId;
use crate::api::LanguageServerId;
use crate::api::LanguageServerRuntimeState;
use crate::api::LocationKind;
use crate::api::RequestId;

/// A command for the server task serving one slot.
pub(crate) enum ServerCmd {
    /// Forward `textDocument/didOpen`.
    DidOpen {
        /// The document path.
        path: PathBuf,
        /// LSP `languageId` for this document.
        language: String,
        /// The document version.
        version: i32,
        /// The full text.
        text: String,
    },
    /// Forward `textDocument/didChange` (debounced): as ranges where `delta`
    /// survives to the flush, else as the full text.
    DidChange {
        /// The document path.
        path: PathBuf,
        /// The document version.
        version: i32,
        /// The full text after the change, always: the replay set keeps it.
        text: String,
        /// The edits from the previous version, when the session could derive
        /// them; cleared on receipt when they do not start from the version
        /// the server task last saw (see `delta`).
        delta: Option<DocumentDelta>,
    },
    /// Forward `textDocument/didClose`.
    DidClose {
        /// The document path.
        path: PathBuf,
    },
    /// Send a notification outside document sync on the live connection, or
    /// drop it when there is none (see [`Notice`]).
    Notify(Notice),
    /// Forward `textDocument/didSave`.
    DidSave {
        /// Saved document path.
        path: PathBuf,
        /// Current full text, supplied for servers that request it.
        text: String,
    },
    /// Request completions; always answered with an [`LspUpdate::Completions`].
    Completion {
        /// The originating request, echoed on the answer.
        request: RequestId,
        /// The target document, echoed on the answer.
        doc: DocumentId,
        /// The buffer version at request time, echoed on the answer.
        version: u64,
        /// The document path.
        path: PathBuf,
        /// The position, already converted to UTF-16 columns.
        position: LineCol,
        /// Why the candidates are asked for.
        trigger: CompletionTrigger,
    },
    /// Fill in what the server deferred for one of its completion items
    /// (`completionItem/resolve`). Needs no text, so it never waits on a
    /// flush.
    ResolveCompletion {
        /// The originating request, echoed on the answer.
        request: RequestId,
        /// The document the item completes in, echoed on the answer.
        doc: DocumentId,
        /// The item, payload included (boxed: it dwarfs every other command).
        item: Box<CompletionItem>,
    },
    /// Request the document's structural symbols.
    DocumentSymbols {
        /// The originating request, echoed on the answer.
        request: RequestId,
        /// The target document, echoed on the answer.
        doc: DocumentId,
        /// The buffer version at request time, echoed on the answer.
        version: u64,
        /// The document path.
        path: PathBuf,
    },
    /// Request inlay hints for a range.
    InlayHints {
        /// The originating request, echoed on the answer.
        request: RequestId,
        /// The target document, echoed on the answer.
        doc: DocumentId,
        /// The buffer version at request time, echoed on the answer.
        version: u64,
        /// The document path.
        path: PathBuf,
        /// The range, already converted to UTF-16 columns.
        range: Range,
    },
    /// Fill in what the server deferred for one of its inlay hints
    /// (`inlayHint/resolve`). Needs no text, so it never waits on a flush.
    ResolveInlayHint {
        /// The originating request, echoed on the answer.
        request: RequestId,
        /// The document the hint annotates, echoed on the answer.
        doc: DocumentId,
        /// The hint, payload included.
        hint: InlayHint,
    },
    /// Request hover information.
    Hover {
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: PathBuf,
        position: LineCol,
    },
    /// Request signature help; `manual` as on
    /// [`crate::api::LspCommand::SignatureHelp`].
    SignatureHelp {
        request: RequestId,
        doc: DocumentId,
        path: PathBuf,
        position: LineCol,
        manual: bool,
    },
    /// Request the selection ranges around each of `positions`, for a
    /// [`crate::api::LspCommand::SelectionRanges`].
    SelectionRanges {
        /// The originating request, the document asked about and its version
        /// at request time, all echoed on the answer.
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: PathBuf,
        /// One per caret, already converted to UTF-16 columns.
        positions: Vec<LineCol>,
    },
    /// Request the occurrences of the symbol at `position`, for a
    /// [`crate::api::LspCommand::DocumentHighlights`].
    DocumentHighlights {
        /// The originating request, the document asked about and its version
        /// at request time, all echoed on the answer.
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: PathBuf,
        /// Already converted to UTF-16 columns.
        position: LineCol,
    },
    /// Request definition locations, or with a `kind` the locations a
    /// [`crate::api::LspCommand::FindLocations`] asks for.
    Definition {
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: PathBuf,
        position: LineCol,
        /// `None` for Go to Definition itself.
        kind: Option<LocationKind>,
    },
    WorkspaceSymbols {
        request: RequestId,
        query: String,
    },
    Rename {
        request: RequestId,
        /// The document the rename was asked from, and its version, so an
        /// answer to a buffer that has moved on is refused, not applied.
        doc: DocumentId,
        version: u64,
        path: PathBuf,
        position: LineCol,
        new_name: String,
    },
    /// Ask whether the symbol at `position` can be renamed.
    PrepareRename {
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: PathBuf,
        /// In UTF-16 columns, for the server.
        position: LineCol,
        /// The same position in buffer columns, carried back with the answer
        /// for picking the identifier when the server leaves that to us.
        caret: LineCol,
    },
    Formatting {
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: PathBuf,
        ask: FormattingAsk,
    },
    /// A request an edit makes at `ask`'s position: the linked ranges there,
    /// or the edits a typed character triggers; always answered with an
    /// [`LspUpdate::Editing`].
    Editing {
        /// The originating request, the document asked about and its version
        /// at request time, all echoed on the answer.
        request: RequestId,
        doc: DocumentId,
        version: u64,
        path: PathBuf,
        ask: EditingAsk,
    },
    /// A call- or type-hierarchy request; always answered with an
    /// [`LspUpdate::Hierarchy`].
    Hierarchy {
        /// The originating request, echoed on the answer.
        request: RequestId,
        direction: HierarchyDirection,
        ask: HierarchyAsk,
    },
    /// One step of listing or running code actions; always answered with an
    /// [`LspUpdate::CodeAction`].
    CodeAction(CodeActionCmd),
    /// One code-lens step: a listing or a run; always answered with an
    /// [`LspUpdate::CodeLens`].
    CodeLens(CodeLensCmd),
    /// One document-link or colour step: a listing or a link resolve; always
    /// answered with an [`LspUpdate::LinkColor`].
    LinkColor(LinkColorCmd),
    /// One inline-completion step: asking for items, or telling the server
    /// one was accepted. Answered with an [`LspUpdate::InlineCompletions`]
    /// only when there are items to show (see `inline_completion`).
    InlineCompletion(InlineCompletionCmd),
}

/// How a server's diagnostic set reached the client.
///
/// A server may do both at once: rust-analyzer, offered pull, stops publishing
/// its own diagnostics but keeps publishing `cargo check`'s. Each feed is a
/// complete set for what it covers, so each replaces only its own layer and
/// the two are shown together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DiagnosticFeed {
    /// `textDocument/publishDiagnostics`.
    Pushed,
    /// The answer to a `textDocument/diagnostic` pull.
    Pulled,
}

/// One server diagnostic layer of a document: a slot's set from one feed.
pub(crate) type DiagnosticLayer = (SlotKey, DiagnosticFeed);

/// What one formatting request asks of its server beyond the document itself.
///
/// Carried per request rather than read from the manager's settings: these are
/// editor settings the session resolves, and the server task has neither the
/// document nor the editor settings to resolve them from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FormattingAsk {
    /// Who asked, and for how much of the document.
    pub(crate) scope: FormattingScope,
    /// The buffer's resolved `editor.tabSize` / `editor.insertSpaces`.
    ///
    /// The setting is per *language*, resolved against the document's
    /// selector. A server that honours it reindents the whole file, so a
    /// default here is not a neutral choice.
    pub(crate) indentation: Indentation,
    /// Why the document is being saved, when the server is to be asked for
    /// its will-save edits (`textDocument/willSaveWaitUntil`) first. Only a
    /// save's scopes ask; `None` for one whose will-save edits are applied.
    pub(crate) will_save: Option<karet_lsp::SaveReason>,
    /// `editor.formatOnSaveTimeout`: how long the save waits for this answer.
    /// The task derives its own, slightly longer, bound from it. A format the
    /// user asked for by hand is bounded the same way.
    pub(crate) save_timeout: Duration,
}

/// What a formatting request covers, and whether a save is waiting on it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum FormattingScope {
    /// The whole document, before a save writes it. Refusals stay silent: a
    /// notice on every save would be noise.
    #[default]
    Save,
    /// Nothing formatted, before a save that does not format: only the
    /// server's will-save edits are asked for, and the answer is always an
    /// [`EditingReply::WillSave`].
    SaveUnformatted,
    /// The whole document, asked for by hand (Format Document).
    Document,
    /// These ranges, in UTF-16 columns, asked for by hand (Format Selection).
    /// One `textDocument/rangeFormatting` each, against the same text.
    Ranges(Vec<Range>),
}

/// What a [`ServerCmd::Editing`] asks, in UTF-16 columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditingAsk {
    /// The ranges linked to the one at `position` (Start Linked Editing).
    Linked {
        /// The caret.
        position: LineCol,
    },
    /// The edits the server makes after `ch` was typed, `position` being
    /// just after it.
    OnType {
        /// The caret after the typed character.
        position: LineCol,
        /// The character typed; `\n` for a newline.
        ch: char,
        /// The buffer's resolved indentation, as a formatting request sends.
        indentation: Indentation,
    },
}

/// A server's answer to a [`ServerCmd::Editing`], or the will-save half of a
/// save's [`ServerCmd::Formatting`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EditingAnswer {
    /// The originating request.
    pub(crate) request: RequestId,
    /// The document asked about.
    pub(crate) doc: DocumentId,
    /// The document's version at request time.
    pub(crate) version: u64,
    /// What the server said.
    pub(crate) reply: EditingReply,
}

/// What an [`EditingAnswer`] carries, ranges in UTF-16 columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditingReply {
    /// The linked ranges; `None` when the server has none there, does not
    /// offer them, failed, or could not be asked.
    Linked(Option<Vec<Range>>),
    /// The edits a typed character triggered, and every character the server
    /// formats on.
    OnType {
        /// The server's edits; empty for a character it does not format on.
        edits: Vec<TextEdit>,
        /// The server's trigger characters, first one first: empty when it
        /// formats on none, and `None` when no server could be asked, so
        /// nothing is known about them yet.
        triggers: Option<Vec<char>>,
    },
    /// A save's will-save edits, sent in place of its
    /// [`LspUpdate::Formatting`] when there are any or the save does not
    /// format. With `then_format`, the save still formats once they are
    /// applied: formatting edits made against the text before them would not
    /// apply after them, so none were asked for yet.
    WillSave {
        /// The server's edits, empty when it had none.
        edits: Vec<TextEdit>,
        /// Whether the save formats after applying them.
        then_format: bool,
    },
}

/// What a server said about renaming at a position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RenameAnswer {
    /// No server could be asked: none runs for the document, or the
    /// connection is down.
    NoServer,
    /// The server does not offer rename at all.
    Unsupported,
    /// The server says the position cannot be renamed, with its reason when
    /// it gave one.
    Refused(Option<String>),
    /// The server named the span to rename (UTF-16 columns).
    Span(karet_core::PrepareRename),
    /// The server renames but does not prepare: the client picks the
    /// identifier at the position.
    Identifier,
}

/// A server's selection ranges, as a [`ServerCmd::SelectionRanges`] asked
/// for them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SelectionRangesAnswer {
    /// The originating request.
    pub(crate) request: RequestId,
    /// The document asked about.
    pub(crate) doc: DocumentId,
    /// The document's version at request time.
    pub(crate) version: u64,
    /// One chain per position asked, innermost first, in UTF-16 columns; an
    /// empty chain where the server had none or could not be asked.
    pub(crate) chains: Vec<Vec<Range>>,
}

/// What a [`ServerCmd::Hierarchy`] asks the server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum HierarchyAsk {
    /// The items at `position` in `path`, already in UTF-16 columns.
    Prepare { path: PathBuf, position: LineCol },
    /// The items one level from this one, which the server produced.
    Children(HierarchyItem),
}

/// A server's document highlights, as a [`ServerCmd::DocumentHighlights`]
/// asked for them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DocumentHighlightsAnswer {
    /// The originating request.
    pub(crate) request: RequestId,
    /// The document asked about.
    pub(crate) doc: DocumentId,
    /// The document's version at request time.
    pub(crate) version: u64,
    /// The occurrences, in UTF-16 columns; `None` when no server answered
    /// (none could be asked, it does not offer highlights, or it failed).
    pub(crate) highlights: Option<Vec<karet_core::DocumentHighlight>>,
}

/// A result flowing from a server task back to the session actor.
pub(crate) enum LspUpdate {
    /// A server-pushed status line (jdtls `language/status`-style), for the
    /// status bar while a heavyweight server imports/indexes.
    ServerStatus {
        /// The slot this report is about, and the incarnation making it.
        token: SlotToken,
        /// The slot the reporting task holds.
        key: SlotKey,
        /// The human-readable status message.
        message: String,
    },
    /// Something the server said to the user: progress, a message, or a
    /// question. Fenced on the slot: a retired server's question is dropped,
    /// which answers it "none".
    Window {
        /// The incarnation of the slot whose server spoke.
        token: SlotToken,
        /// The slot whose server spoke.
        key: SlotKey,
        /// What it said.
        update: WindowUpdate,
    },
    /// The server asked for every inlay hint it answered to be re-fetched
    /// (`workspace/inlayHint/refresh`).
    ///
    /// Fenced on the slot like the other reports a task makes about itself: a
    /// retired task's refresh describes a server nobody is asking any more.
    InlayHintsRefresh {
        /// The incarnation of the slot whose server asked.
        token: SlotToken,
        /// The slot whose server asked.
        key: SlotKey,
    },
    /// Completion items answering a [`ServerCmd::Completion`] (ranges still in
    /// UTF-16 columns; the session converts them against the buffer).
    Completions {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The originating request.
        request: RequestId,
        /// The target document.
        doc: DocumentId,
        /// The buffer version the request was made against.
        version: u64,
        /// The mapped list.
        list: CompletionList,
        /// The answering server's trigger and commit characters.
        options: CompletionOptions,
    },
    /// The answer to a [`ServerCmd::ResolveCompletion`]: what the server
    /// filled in, ranges still in UTF-16 columns, or `None` when nothing was.
    CompletionResolved {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The originating request.
        request: RequestId,
        /// The document the item completes in.
        doc: DocumentId,
        /// What the server filled in.
        resolved: Option<CompletionItem>,
    },
    /// Inlay hints answering a [`ServerCmd::InlayHints`] request. Positions
    /// remain in UTF-16 until the session adopts the update.
    InlayHints {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The originating request.
        request: RequestId,
        /// The target document.
        doc: DocumentId,
        /// The buffer version the request was made against.
        version: u64,
        /// The mapped hints.
        hints: Vec<InlayHint>,
    },
    /// Inline completions answering an [`InlineCompletionCmd::Ask`], from the
    /// server `server` names. Ranges remain in UTF-16 until the session adopts
    /// the update. Never empty: a server with nothing to propose stays quiet.
    InlineCompletions {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The slot whose server answered, which an accept is routed back to.
        server: SlotKey,
        /// The originating request.
        request: RequestId,
        /// The target document.
        doc: DocumentId,
        /// The buffer version the request was made against.
        version: u64,
        /// The items.
        items: Vec<InlineCompletionItem>,
    },
    /// The answer to an [`InlineCompletionCmd::Account`] step.
    Account {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The originating request.
        request: RequestId,
        /// The provider the step named.
        server: LanguageServerId,
        /// What came of it.
        outcome: crate::api::AccountOutcome,
    },
    /// A [`ServerCmd::InlayHints`] request a server existed for but did not
    /// answer: it failed, timed out, was superseded, or its slot retired.
    /// Distinct from an empty [`Self::InlayHints`], which says there are none.
    InlayHintsFailed {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The originating request.
        request: RequestId,
        /// The target document.
        doc: DocumentId,
        /// The buffer version the request was made against.
        version: u64,
    },
    /// The answer to a [`ServerCmd::ResolveInlayHint`]: the hint as resolved,
    /// or unchanged when it could not be. Positions and part locations remain
    /// in UTF-16 until the session adopts the update.
    InlayHintResolved {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The originating request.
        request: RequestId,
        /// The document the hint annotates.
        doc: DocumentId,
        /// The hint.
        hint: InlayHint,
    },
    /// Document symbols answering a [`ServerCmd::DocumentSymbols`] request. Ranges
    /// remain in UTF-16 until the session adopts the update.
    Symbols {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The originating request.
        request: RequestId,
        /// The target document.
        doc: DocumentId,
        /// The buffer version the request was made against.
        version: u64,
        /// The mapped symbol tree.
        symbols: Vec<Symbol>,
    },
    /// A request was refused because the server never offered `feature`.
    ///
    /// Sent only for requests a user asks for by hand, and always *before*
    /// the request's own empty answer, so the client can explain the empty
    /// answer rather than report it as nothing found.
    Unsupported {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The refused request.
        request: RequestId,
        /// The provider that was asked.
        server: LanguageServerId,
        /// What it does not offer.
        feature: ServerFeature,
    },
    /// Hover response in UTF-16 coordinates.
    Hover {
        generation: u64,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        hover: Option<Hover>,
    },
    /// Signature-help response, with the server's trigger characters beside
    /// it. Carries no positions, so it needs no conversion on the actor.
    SignatureHelp {
        generation: u64,
        request: RequestId,
        doc: DocumentId,
        help: Option<SignatureHelp>,
        options: SignatureHelpOptions,
    },
    /// The answer to a [`ServerCmd::SelectionRanges`].
    SelectionRanges {
        generation: u64,
        answer: SelectionRangesAnswer,
    },
    /// The answer to a [`ServerCmd::DocumentHighlights`].
    DocumentHighlights {
        generation: u64,
        answer: DocumentHighlightsAnswer,
    },
    /// Definition (or, with a `kind`, other navigation) response in UTF-16
    /// coordinates.
    Definitions {
        generation: u64,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        /// What was asked, as on [`ServerCmd::Definition`].
        kind: Option<LocationKind>,
        locations: Vec<Location>,
    },
    /// Workspace symbols, each with its file, in the server's UTF-16 columns.
    WorkspaceSymbols {
        generation: u64,
        request: RequestId,
        symbols: Vec<WorkspaceSymbol>,
    },
    /// A rename's edit, in UTF-16 columns, for the session to apply, or the
    /// reason the request produced none (a server's error reply, an edit the
    /// client refused to convert, a request that timed out).
    WorkspaceEdit {
        generation: u64,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        edit: Result<WorkspaceEdit, String>,
    },
    /// The answer to a [`ServerCmd::PrepareRename`]; ranges in UTF-16 columns.
    PrepareRename {
        generation: u64,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        /// The request's position in buffer columns.
        caret: LineCol,
        answer: RenameAnswer,
    },
    /// A server's `workspace/applyEdit`, waiting for the session's answer.
    ///
    /// Fenced on the slot: a retired server's request is dropped, which
    /// answers it `applied: false`.
    ApplyEdit {
        /// The incarnation of the slot whose server asked.
        token: SlotToken,
        /// The slot whose server asked.
        key: SlotKey,
        /// The request, answered through its own `respond`.
        request: karet_lsp::ApplyEditRequest,
    },
    Formatting {
        generation: u64,
        request: RequestId,
        doc: DocumentId,
        version: u64,
        /// Whether a language server actually formatted this file.
        ///
        /// The question the session asks, rather than what the server claims it
        /// can do. Distinct from empty `edits`: a server that formatted and
        /// found nothing to change has done its job and this is `true`, while a
        /// server that never offered the method, was not reachable, or failed
        /// the request has formatted nothing — and the session is then free to
        /// use its own formatter instead.
        formatted: bool,
        edits: Vec<TextEdit>,
        /// What the request covered, as asked. Only a
        /// [`FormattingScope::Save`] answer may finish a save, and one that
        /// arrives after its save gave up must not be mistaken for a format
        /// the user asked for by hand.
        scope: FormattingScope,
    },
    /// The answer to a [`ServerCmd::Editing`], or a save's will-save edits.
    Editing {
        generation: u64,
        answer: EditingAnswer,
    },
    /// The answer to a [`ServerCmd::Hierarchy`], in UTF-16 columns: empty
    /// when no server could answer it.
    Hierarchy {
        generation: u64,
        request: RequestId,
        direction: HierarchyDirection,
        items: Vec<HierarchyItem>,
    },
    /// The answer to a [`ServerCmd::CodeAction`]; ranges in UTF-16 columns.
    CodeAction {
        generation: u64,
        answer: CodeActionAnswer,
    },
    /// The answer to a [`ServerCmd::CodeLens`]; ranges in UTF-16 columns.
    CodeLens {
        generation: u64,
        answer: CodeLensAnswer,
    },
    /// The answer to a [`ServerCmd::LinkColor`]; a listing's ranges in
    /// UTF-16 columns.
    LinkColor {
        generation: u64,
        answer: LinkColorAnswer,
    },
    /// A complete server diagnostic layer for one file.
    Diagnostics {
        /// The incarnation of the slot that published these.
        token: SlotToken,
        /// Provider/root identity whose diagnostic layer is replaced.
        server: SlotKey,
        /// File whose LSP diagnostic layer is replaced.
        path: PathBuf,
        /// LSP document version, when the server supplied it.
        version: Option<i32>,
        /// Whether the server published these or answered a pull; each feed
        /// is its own layer.
        feed: DiagnosticFeed,
        /// Diagnostics in UTF-16 coordinates.
        diagnostics: Vec<Diagnostic>,
    },
    /// A document's complete semantic tokens from the language's semantic
    /// token owner, replacing whatever overlay it had.
    SemanticTokens {
        /// The incarnation of the slot that answered.
        token: SlotToken,
        /// The slot whose server answered.
        server: SlotKey,
        /// The document they classify.
        path: PathBuf,
        /// The document version they were asked at.
        version: i32,
        /// The tokens, in UTF-16 columns.
        tokens: Vec<karet_core::SemanticToken>,
    },
    /// A document's folding ranges from its primary server, replacing the
    /// grammar's fold regions on its snapshots (see `super::folding`).
    FoldingRanges {
        /// The incarnation of the slot that answered.
        token: SlotToken,
        /// The slot whose server answered.
        server: SlotKey,
        /// The document they fold.
        path: PathBuf,
        /// The document version they were asked at.
        version: i32,
        /// The regions, normalized.
        folds: karet_core::FoldRegions,
    },
    /// The server binary could not be started (reported once per language).
    SpawnFailed {
        /// The incarnation of the slot that failed to start.
        token: SlotToken,
        /// The slot that failed to start.
        ///
        /// Render `key.provider`, never the key: the key is
        /// `provider@/absolute/repository/root`, and printing it whole put a
        /// full path into the user's notification.
        key: SlotKey,
        /// The executable and arguments karet ran.
        command: String,
        /// The most specific one-line reason available, which for a server that
        /// ran at all is usually its own last line of stderr.
        reason: String,
        /// Whether retrying could ever help.
        permanent: bool,
    },
    /// A launch preflight failed with a specific diagnosis (reported once per
    /// generation); the server is not spawned.
    PreflightFailed {
        /// The manager generation the preflight ran under.
        generation: u64,
        /// The human-readable diagnosis (what is missing and how to fix it).
        message: String,
    },
    /// A running server's connection closed (reported once per language).
    ServerDied {
        /// The incarnation of the slot that died.
        token: SlotToken,
        /// The slot whose server died.
        key: SlotKey,
    },
    /// A document-sync command never reached its server.
    ///
    /// Deliberately not a [`Self::RuntimeState`]: the server is running fine, it
    /// is merely behind, and publishing `Retrying` for it wrote a wrong state into
    /// the manager that nothing ever corrected -- the task had not transitioned, so
    /// it never re-reported `Running`, and the badge stayed a warning for the rest
    /// of the session while every request was answered normally.
    SyncFailed {
        /// The manager generation that spawned the server task.
        generation: u64,
        /// The provider whose copy of the document is now behind.
        server: LanguageServerId,
        /// What went wrong, phrased for the user.
        reason: String,
    },
    /// A provider stayed down past the grace period: drop what it published.
    ///
    /// Diagnostics used to be inserted and never removed, so a crashed server's
    /// squiggles outlived it. Sent by a task that is still live -- so still of the
    /// current generation -- and only after the grace window, so a reconnect inside
    /// it does not make every marker flicker off and back on.
    ///
    /// Scoped to what a live task can say about itself: a *retired* task's layer
    /// is cleared by the manager, synchronously, as part of retiring it.
    DiagnosticsCleared {
        /// The incarnation of the slot asking for the clear.
        token: SlotToken,
        /// The diagnostic layer to drop, keyed exactly as it was published.
        ///
        /// Being the slot's own key, this already scopes the clear to the one
        /// instance that died: a provider running at two repository roots keeps
        /// the markers published by the root that is still healthy.
        server: SlotKey,
    },
    /// The server withdrew pull diagnostics for a document it was pulled
    /// for: drop the slot's [`DiagnosticFeed::Pulled`] layer from every
    /// document, related documents' reports included.
    ///
    /// Only that feed goes: the server is still up, so what it publishes
    /// stays, and the documents it still answers pulls for are pulled again.
    PulledDiagnosticsWithdrawn {
        /// The incarnation of the slot whose pulls were withdrawn.
        token: SlotToken,
        /// The slot whose pulled layer is dropped.
        server: SlotKey,
    },
    /// A built-in provider karet can install is locally absent. No network
    /// operation was attempted.
    InstallRequired {
        /// The manager generation that observed the missing installation.
        generation: u64,
        /// Missing managed provider.
        server: LanguageServerId,
        /// The language whose document wanted it — the key its per-language
        /// enable flag is stored under.
        language: String,
    },
    /// A built-in provider is locally absent and karet cannot install it.
    ///
    /// Separate from [`LspUpdate::InstallRequired`] because the two ask
    /// completely different things of the user: one offers a download, the
    /// other explains a toolchain they have to set up themselves. Offering the
    /// download for these providers produced a prompt whose install always
    /// failed.
    ManualInstallRequired {
        /// The manager generation that observed the missing installation.
        generation: u64,
        /// The provider the user must supply.
        server: LanguageServerId,
        /// The executable karet looked for.
        command: String,
        /// Why karet will not install it, from
        /// [`manual_install_reason`](crate::lsp_registry::manual_install_reason).
        reason: String,
    },
    /// A provider/root connection changed lifecycle state.
    RuntimeState {
        /// The incarnation of the slot reporting.
        token: SlotToken,
        /// The slot reporting about itself. Both halves the client needs --
        /// provider and root -- come from it, and it is also what the manager
        /// looks the slot up by, so a report cannot be filed against a different
        /// instance than the one that sent it.
        key: SlotKey,
        state: LanguageServerRuntimeState,
        error: Option<String>,
    },
}
