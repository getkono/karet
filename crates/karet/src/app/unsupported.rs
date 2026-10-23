//! Saying so when a language server does not offer what the user asked for.
//!
//! The session refuses a request the server never advertised without issuing
//! it, and answers it empty. For a request the user made by hand it also says
//! *why*, ahead of the empty answer ([`SessionEvent::FeatureUnsupported`]).
//! Without that, "go to definition" on a server that has no definition
//! provider read "no definition found" -- a claim about the symbol, when the
//! truth is a fact about the provider the user can act on.

use karet_core::ServerFeature;
use karet_session::LanguageServerId;

use super::*;

/// How a feature is named in a notice, as the user would ask for it.
fn feature_label(feature: ServerFeature) -> &'static str {
    match feature {
        ServerFeature::Hover => "hover",
        ServerFeature::Definition => "go to definition",
        ServerFeature::Declaration => "go to declaration",
        ServerFeature::TypeDefinition => "go to type definition",
        ServerFeature::Rename => "rename",
        ServerFeature::WorkspaceSymbol => "workspace symbol search",
        ServerFeature::References => "find references",
        ServerFeature::Implementation => "go to implementation",
        ServerFeature::Formatting => "formatting",
        ServerFeature::RangeFormatting => "formatting a selection",
        ServerFeature::CodeAction => "code actions",
        ServerFeature::LinkedEditingRange => "linked editing",
        _ => "this request",
    }
}

impl App {
    /// Tell the user the server serving their request does not offer it.
    ///
    /// Arrives before the request's own empty answer, which then must not
    /// contradict it: a pending definition is settled here, so its empty
    /// answer is not reported as "no definition found", and a pending hover
    /// is marked so its answer -- still worth showing if there are diagnostics
    /// under the caret -- does not add "no hover information" on top.
    pub(super) fn on_feature_unsupported(
        &mut self,
        id: Option<RequestId>,
        server: LanguageServerId,
        feature: ServerFeature,
    ) {
        if let Some(id) = id {
            if self
                .pending_definition
                .is_some_and(|pending| pending.id == id)
            {
                self.pending_definition = None;
            }
            if let Some(pending) = self.pending_hover.as_mut()
                && pending.id == id
            {
                pending.refused = true;
            }
            // The rename's own answer -- an empty, unapplied edit -- follows;
            // this notice already says why.
            if self
                .pending_rename
                .as_ref()
                .is_some_and(|pending| pending.id == id)
            {
                self.pending_rename = None;
            }
            // Likewise a quick-fix request: its empty answer must not add
            // "none available" to a notice that already says why.
            self.refuse_code_actions(id);
            self.refuse_linked_editing(id);
        }
        self.notify(
            Report::Refusal,
            NotificationKind::Lsp,
            format!(
                "{} does not support {}",
                server.display_name(),
                feature_label(feature)
            ),
        );
    }
}
