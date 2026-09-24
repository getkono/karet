//! What one connection may be asked, and for which documents.
//!
//! Two sources say what a server supports. The handshake advertises features
//! for every document it will ever see. A `client/registerCapability` turns a
//! feature on later, optionally only for the documents its selector matches
//! (see [`crate::selector`]). They are kept apart rather than merged into one
//! [`Capabilities`], because merging loses both distinctions that matter: which
//! documents a registration covers, and that withdrawing a registration must
//! not disable a feature the handshake advertised outright.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use karet_core::Capabilities;
use karet_core::ServerFeature;

use crate::selector::Selector;

/// One live dynamic registration.
#[derive(Debug)]
struct Registration {
    feature: ServerFeature,
    /// `None` covers every document.
    selector: Option<Selector>,
}

/// The advertised capabilities, the live registrations, and the language each
/// open document was opened as -- everything a per-document gate reads.
#[derive(Debug, Default)]
pub(crate) struct Gate {
    /// From the `initialize` reply: every document.
    advertised: Capabilities,
    /// Keyed by the server's registration id: two registrations can name the
    /// same method, and unregistering one must leave the other.
    registrations: HashMap<String, Registration>,
    /// The `languageId` each open document was announced with, which a
    /// selector's `language` filter is matched against.
    languages: HashMap<PathBuf, String>,
}

impl Gate {
    /// Record what the handshake advertised.
    pub(crate) fn advertise(&mut self, capabilities: Capabilities) {
        self.advertised = capabilities;
    }

    /// Record a registration of `feature` under `id`, for the documents
    /// `selector` covers.
    pub(crate) fn register(
        &mut self,
        id: String,
        feature: ServerFeature,
        selector: Option<Selector>,
    ) {
        self.registrations
            .insert(id, Registration { feature, selector });
    }

    /// Withdraw registration `id`, returning the feature it had turned on.
    pub(crate) fn unregister(&mut self, id: &str) -> Option<ServerFeature> {
        self.registrations
            .remove(id)
            .map(|registration| registration.feature)
    }

    /// Record that `path` was opened as `language`.
    pub(crate) fn opened(&mut self, path: &Path, language: &str) {
        self.languages
            .insert(path.to_path_buf(), language.to_owned());
    }

    /// Record that `path` was closed.
    pub(crate) fn closed(&mut self, path: &Path) {
        self.languages.remove(path);
    }

    /// The connection-wide view: everything advertised, plus every feature a
    /// live registration turns on for at least one document.
    pub(crate) fn capabilities(&self) -> Capabilities {
        let mut out = self.advertised.clone();
        for registration in self.registrations.values() {
            out.enable(registration.feature);
        }
        out
    }

    /// Whether `feature` is available for at least one document.
    pub(crate) fn supports(&self, feature: ServerFeature) -> bool {
        self.advertised.supports(feature)
            || self
                .registrations
                .values()
                .any(|registration| registration.feature == feature)
    }

    /// Whether `feature` is available for the document at `doc`: advertised,
    /// or registered with a selector that covers it.
    pub(crate) fn supports_for(&self, feature: ServerFeature, doc: &Path) -> bool {
        if self.advertised.supports(feature) {
            return true;
        }
        let language = self.languages.get(doc).map(String::as_str);
        self.registrations.values().any(|registration| {
            registration.feature == feature
                && registration
                    .selector
                    .as_ref()
                    .is_none_or(|selector| selector.matches(doc, language))
        })
    }
}
