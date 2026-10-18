//! Where each callable sits, and what every name could mean — computed once per link.
//!
//! Resolution by name is a series of "the nearest one wins" questions: the same type,
//! the same file, the same module, the same package. Each needs a callable's place in
//! the tree, so that is worked out once for every callable rather than once per call.

use std::collections::HashMap;

use karet_treesitter::LanguageId;

use super::site::Import;
use crate::config::Configuration;
use crate::id::SeamId;
use crate::index::SeamIndex;
use crate::model::ConfigMembership;
use crate::model::FileId;
use crate::model::NodeKind;

/// One callable's place in the tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Scope {
    /// Which language mapping produced it; names never resolve across families.
    pub(crate) family: Option<LanguageId>,
    /// The nearest type or contract around it — what `self` means inside it.
    pub(crate) owner: Option<SeamId>,
    /// The nearest module or package around it.
    pub(crate) module: Option<SeamId>,
    /// The file it is written in.
    pub(crate) file: FileId,
    /// The package root it belongs to.
    pub(crate) package: SeamId,
    /// Whether it exists on the host build — not excluded by a gate or a test path.
    pub(crate) live: bool,
    /// Whether it has a body rather than only declaring one.
    pub(crate) has_body: bool,
    /// Whether it sits directly in a contract.
    pub(crate) in_contract: bool,
    /// Whether its language lets a bare name reach a member of the enclosing type.
    pub(crate) implicit_this: bool,
}

/// Every callable's scope, and every name a call or a qualifier might mean.
pub(crate) struct Tables<'a> {
    pub(crate) index: &'a SeamIndex,
    pub(crate) scopes: HashMap<SeamId, Scope>,
    pub(crate) callables: HashMap<&'a str, Vec<SeamId>>,
    containers: HashMap<String, Vec<SeamId>>,
    families: HashMap<FileId, Option<LanguageId>>,
    ancestry: HashMap<SeamId, Vec<SeamId>>,
}

impl<'a> Tables<'a> {
    /// Work out every callable's place, and index callables and containers by name.
    pub(crate) fn new(index: &'a SeamIndex) -> Self {
        let membership = crate::config::evaluate(index, &Configuration::unconfigured());
        let mut families: HashMap<FileId, Option<LanguageId>> = HashMap::new();
        let mut ancestry: HashMap<SeamId, Vec<SeamId>> = HashMap::new();
        let mut scopes = HashMap::new();
        let mut callables: HashMap<&'a str, Vec<SeamId>> = HashMap::new();
        let kind_of = |id: &SeamId| index.node(*id).map(|node| node.kind);
        for (id, callable) in index.calls().callables() {
            let Some(node) = index.node(id) else {
                continue;
            };
            let family = *families
                .entry(node.location.file)
                .or_insert_with(|| family_of(index, node.location.file));
            let ancestors = index.ancestors(id);
            let owner = owner_of(&ancestors, kind_of);
            let module = ancestors
                .iter()
                .copied()
                .find(|up| matches!(kind_of(up), Some(NodeKind::Module | NodeKind::Package)));
            let in_contract = node
                .parent
                .and_then(|parent| index.node(parent))
                .is_some_and(|parent| parent.kind == NodeKind::Interface);
            let implicit_this = family
                .and_then(crate::lang::for_language)
                .is_some_and(|mapping| mapping.implicit_receiver());
            scopes.insert(
                id,
                Scope {
                    family,
                    owner,
                    module,
                    file: node.location.file,
                    package: ancestors.last().copied().unwrap_or(id),
                    live: membership.get(&id) != Some(&ConfigMembership::Inactive),
                    has_body: callable.has_body,
                    in_contract,
                    implicit_this,
                },
            );
            callables.entry(node.name.as_str()).or_default().push(id);
            ancestry.insert(id, ancestors);
        }

        let mut containers: HashMap<String, Vec<SeamId>> = HashMap::new();
        let mut ordered: Vec<&crate::model::Node> = index
            .nodes()
            .filter(|node| {
                matches!(
                    node.kind,
                    NodeKind::Type | NodeKind::Interface | NodeKind::Module | NodeKind::Package
                )
            })
            .collect();
        ordered.sort_unstable_by_key(|node| node.id);
        for node in ordered {
            families
                .entry(node.location.file)
                .or_insert_with(|| family_of(index, node.location.file));
            // Keyed as code writes it: a Cargo package `karet-core` is `karet_core` in a path.
            containers
                .entry(node.name.replace('-', "_"))
                .or_default()
                .push(node.id);
            ancestry.insert(node.id, index.ancestors(node.id));
        }

        Self {
            index,
            scopes,
            callables,
            containers,
            families,
            ancestry,
        }
    }

    /// The language family a node was written in.
    pub(crate) fn family(&self, id: SeamId) -> Option<LanguageId> {
        if let Some(scope) = self.scopes.get(&id) {
            return scope.family;
        }
        let file = self.index.node(id)?.location.file;
        self.families.get(&file).copied().flatten()
    }

    /// Whether `id` can be named from code in `family`.
    ///
    /// A node with no language of its own — a package sits in its manifest — belongs to
    /// the languages its contents are written in, so a Rust path can name a Cargo package
    /// but not a Node package that happens to share its name.
    pub(crate) fn fits(&self, id: SeamId, family: Option<LanguageId>) -> bool {
        match self.family(id) {
            Some(own) => Some(own) == family,
            None => self
                .index
                .children(id)
                .iter()
                .any(|child| self.family(*child).is_some_and(|f| Some(f) == family)),
        }
    }

    /// The imports written in `file`.
    pub(crate) fn imports(&self, file: FileId) -> &'a [Import] {
        self.index
            .calls()
            .file(file)
            .map_or(&[][..], |calls| calls.imports.as_slice())
    }

    /// The callables named `name`, in id order.
    pub(crate) fn named(&self, name: &str) -> &[SeamId] {
        self.callables.get(name).map_or(&[][..], Vec::as_slice)
    }

    /// The types, contracts, modules and packages named `name`, in id order.
    pub(crate) fn containers_named(&self, name: &str) -> &[SeamId] {
        // Keyed `-` as `_`, so a name looked up as written — `string-utils` from a
        // TypeScript import path — is normalized the same way.
        self.containers
            .get(&name.replace('-', "_"))
            .map_or(&[][..], Vec::as_slice)
    }

    /// Whether `id` sits beneath a node named `name`.
    pub(crate) fn under_named(&self, id: SeamId, name: &str) -> bool {
        let ancestors = match self.ancestry.get(&id) {
            Some(cached) => std::borrow::Cow::Borrowed(cached.as_slice()),
            None => std::borrow::Cow::Owned(self.index.ancestors(id)),
        };
        ancestors.iter().any(|up| {
            self.index
                .node(*up)
                .is_some_and(|node| same_identifier(&node.name, name))
        })
    }
}

/// The type a callable belongs to: its nearest type or contract, or the implementation
/// block it is written in when that block could not be placed under one.
fn owner_of(ancestors: &[SeamId], kind_of: impl Fn(&SeamId) -> Option<NodeKind>) -> Option<SeamId> {
    let at = ancestors.iter().position(|up| {
        matches!(
            kind_of(up),
            Some(NodeKind::Type | NodeKind::Interface | NodeKind::Implementation)
        )
    })?;
    let found = ancestors.get(at)?;
    if kind_of(found) != Some(NodeKind::Implementation) {
        return Some(*found);
    }
    // A trait binding regrouped under its type answers to the type.
    match ancestors.get(at + 1) {
        Some(above) if matches!(kind_of(above), Some(NodeKind::Type | NodeKind::Interface)) => {
            Some(*above)
        },
        _ => Some(*found),
    }
}

/// Whether two names are the same identifier, reading a package's `-` as the `_` code
/// writes in its place.
fn same_identifier(declared: &str, written: &str) -> bool {
    declared.len() == written.len()
        && declared
            .bytes()
            .zip(written.bytes())
            .all(|(d, w)| d == w || (d == b'-' && w == b'_'))
}

/// The language family of the grammar that parsed `file`.
///
/// A family, not a grammar: JavaScript, TypeScript and TSX share one mapping, and a call
/// from one to another is an ordinary call.
fn family_of(index: &SeamIndex, file: FileId) -> Option<LanguageId> {
    let language = index
        .file_attribution(file)
        .map(|(_, language)| language)
        .or_else(|| karet_treesitter::language_id_from_path(index.file_path(file)?))?;
    crate::lang::for_language(language).map(|mapping| mapping.language())
}
