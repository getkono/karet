//! Document links and colours: listing a document's for a client, and
//! resolving the target of the link it follows.
//!
//! A listing is answered by [`Event::LinksAndColors`] in buffer columns, each
//! half `None` when nothing was learned about it; a resolve by
//! [`Event::DocumentLinkResolved`].

use karet_core::DocumentLink;

use super::super::columns::utf16_range_to_buffer;
use super::super::lsp_requests::NO_SERVER;
use super::*;
use crate::lsp::LinkColorAnswer;
use crate::lsp::LinkColorCmd;
use crate::lsp::NO_TARGET;

impl Session {
    /// Serve [`Command::LinksAndColors`]: ask the document's server,
    /// answering at once with none of either when no server runs for it.
    pub(in crate::session) fn links_and_colors(&mut self, id: RequestId, doc_id: DocumentId) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        let cmd = LinkColorCmd::List {
            request: id,
            doc: doc_id,
            version: doc.buffer.version(),
            path: doc.path.clone(),
        };
        if let Some(cmd) = self.lsp.link_color(doc.language_selector, &doc.path, cmd) {
            self.adopt_link_color(cmd.unserved(NO_SERVER));
        }
    }

    /// Serve [`Command::ResolveDocumentLink`]: ask the document's server for
    /// the link's target. A link that already has one is answered as it is,
    /// and one with no payload has nothing to hand the server, so it is
    /// answered as having no target.
    pub(in crate::session) fn resolve_document_link(
        &mut self,
        id: RequestId,
        doc_id: DocumentId,
        link: DocumentLink,
    ) {
        let Some(doc) = Self::doc_or_report(&self.store, &self.events, id, doc_id) else {
            return;
        };
        if link.is_resolved() || link.payload.is_none() {
            let result = if link.is_resolved() {
                Ok(link)
            } else {
                Err(NO_TARGET.to_owned())
            };
            let answer = LinkColorAnswer::Resolved {
                request: id,
                doc: doc_id,
                result,
            };
            self.adopt_link_color(answer);
            return;
        }
        let cmd = LinkColorCmd::Resolve {
            request: id,
            doc: doc_id,
            link: Box::new(link),
        };
        if let Some(cmd) = self.lsp.link_color(doc.language_selector, &doc.path, cmd) {
            self.adopt_link_color(cmd.unserved(NO_SERVER));
        }
    }

    /// Adopt a server task's link-and-colour answer.
    ///
    /// A listing made against text that has since changed is reported as
    /// unanswered, both halves: its ranges describe text that is no longer
    /// there, and converting them against the new text would misplace them.
    pub(in crate::session) fn adopt_link_color(&mut self, answer: LinkColorAnswer) {
        match answer {
            LinkColorAnswer::Listed {
                request,
                doc,
                version,
                links,
                colors,
            } => {
                let current = self
                    .store
                    .docs
                    .get(&doc)
                    .filter(|document| document.buffer.version() == version);
                let (links, colors) = match current {
                    Some(document) => {
                        let buffer = &document.buffer;
                        let links = links.map(|mut links| {
                            for link in &mut links {
                                link.range = utf16_range_to_buffer(buffer, link.range);
                            }
                            links
                        });
                        let colors = colors.map(|mut colors| {
                            for colour in &mut colors {
                                colour.range = utf16_range_to_buffer(buffer, colour.range);
                            }
                            colors
                        });
                        (links, colors)
                    },
                    None => (None, None),
                };
                let event = Event::LinksAndColors {
                    doc,
                    version,
                    links,
                    colors,
                };
                self.emit(Some(request), event);
            },
            LinkColorAnswer::Resolved {
                request,
                doc,
                result,
            } => self.emit(Some(request), Event::DocumentLinkResolved { doc, result }),
        }
    }
}
