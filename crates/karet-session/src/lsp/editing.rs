//! Serving a [`ServerCmd::Editing`] on a server task: the ranges linked to
//! the caret's (Start Linked Editing), or the edits a typed character
//! triggers (on-type formatting).
//!
//! [`ServerCmd::Editing`]: super::message::ServerCmd::Editing

use std::path::Path;

use karet_core::ServerFeature;
use karet_lsp::LspClient;
use karet_lsp::LspError;

use super::message::EditingAsk;
use super::message::EditingReply;

/// What `ask` is answered with when no server can be asked: no linked
/// ranges, or no edits and no trigger characters.
pub(super) fn unanswered(ask: &EditingAsk) -> EditingReply {
    match ask {
        EditingAsk::Linked { .. } => EditingReply::Linked(None),
        EditingAsk::OnType { .. } => EditingReply::OnType {
            edits: Vec::new(),
            triggers: Vec::new(),
        },
    }
}

/// The feature a refusal of `ask` is reported under, when it is reported at
/// all: linked editing is always asked for by hand, while on-type formatting
/// is offered after every typed character and never says it is refused.
pub(super) fn reported(ask: &EditingAsk) -> Option<ServerFeature> {
    match ask {
        EditingAsk::Linked { .. } => Some(ServerFeature::LinkedEditingRange),
        EditingAsk::OnType { .. } => None,
    }
}

/// Ask `client` what `ask` asks about `path`; a refusal or a failure is the
/// error, for the caller to report and charge.
pub(super) async fn ask(
    client: &LspClient,
    path: &Path,
    ask: &EditingAsk,
) -> Result<EditingReply, LspError> {
    match *ask {
        EditingAsk::Linked { position } => {
            let linked = client.linked_editing_ranges(path, position).await?;
            Ok(EditingReply::Linked(linked.map(|linked| linked.ranges)))
        },
        EditingAsk::OnType {
            position,
            ch,
            indentation,
        } => {
            let edits = client
                .on_type_formatting(path, position, ch, indentation)
                .await?;
            Ok(EditingReply::OnType {
                edits,
                triggers: triggers(client),
            })
        },
    }
}

/// Every character `client`'s server formats on, its first trigger first;
/// empty when it formats on none.
pub(super) fn triggers(client: &LspClient) -> Vec<char> {
    client
        .capabilities()
        .on_type_formatting
        .map(|options| {
            let mut all = vec![options.first_trigger_character];
            all.extend(options.more_trigger_characters);
            all
        })
        .unwrap_or_default()
}
