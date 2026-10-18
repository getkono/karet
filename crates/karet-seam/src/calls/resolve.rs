//! Resolving a call site to what it reaches, by name.
//!
//! The structural tier has names and a tree, not types, so it resolves by scope: the
//! nearest declaration with the right name wins, and the first tier that finds anything
//! decides. One candidate is [`Resolution::Exact`]; several are [`Resolution::Ambiguous`]
//! and count only towards the upper bound. Nothing is guessed silently — every edge says
//! how it was reached.
//!
//! The tiers, first match wins:
//!
//! 1. **Receiver `self`/`super`** — members of the caller's own type.
//! 2. **Qualifier** (`a::b::f`, `Type.f`) — members of the container it names; a
//!    qualifier naming nothing in the index is [`Resolution::External`].
//! 3. **Method on a value** (`x.f()`) — members named `f` anywhere in the family; none is
//!    external, since a method nobody here declares belongs to someone else's type.
//! 4. **Free call** — the caller's type, then its file, its module, its imports, a type
//!    of that name (a constructor), its package, and the whole index.
//!
//! Every tier keeps only candidates in the caller's language family, prefers ones that
//! exist on the host build, and prefers a definition over a bare declaration of it.

use super::scope::Scope;
use super::scope::Tables;
use super::site;
use super::site::CallSite;
use super::site::Receiver;
use crate::edge::Endpoint;
use crate::edge::Resolution;
use crate::id::SeamId;
use crate::index::SeamIndex;
use crate::model::Lens;
use crate::model::NodeKind;

/// More candidates than this and a site is too ambiguous to count even in the upper
/// bound: `x.get()` would otherwise link to every `get` in the repository.
pub const MAX_CANDIDATES: usize = 16;

/// Substitution subtypes that make a type's member replaceable by a subtype's.
const OVERRIDABLE: &[&str] = &["abstract", "open", "override"];

/// Member names a constructor call is redirected to, by language convention.
const CONSTRUCTORS: &[&str] = &["new", "init", "constructor", "__init__"];

/// What one site resolved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// Every target, each with how it was reached.
    pub targets: Vec<(Endpoint, Resolution)>,
}

impl Resolved {
    /// A single target.
    #[must_use]
    pub fn one(endpoint: Endpoint, resolution: Resolution) -> Self {
        Self {
            targets: vec![(endpoint, resolution)],
        }
    }

    /// A site nothing could be found for.
    #[must_use]
    pub fn unresolved(hint: Option<String>) -> Self {
        Self::one(
            Endpoint::Unresolved {
                hint,
                resolvable: true,
            },
            Resolution::Unresolved,
        )
    }

    /// A callee outside the indexed code.
    #[must_use]
    pub fn external(display: impl Into<String>) -> Self {
        Self::one(
            Endpoint::External {
                display: display.into(),
            },
            Resolution::External,
        )
    }
}

/// A source of call resolutions.
///
/// The structural [`NameResolver`] is the one implementation today. The seam exists so a
/// semantic tier — a language server's call hierarchy — can answer first for the sites it
/// knows and leave the rest to names, without the graph or the report changing.
pub trait CallResolver: Sync {
    /// Resolve one site in `caller`, or `None` to leave it to the next resolver.
    fn resolve(&self, index: &SeamIndex, caller: SeamId, site: &CallSite) -> Option<Resolved>;
}

/// Resolution by name and scope, over one index.
pub struct NameResolver<'a> {
    tables: Tables<'a>,
}

impl<'a> NameResolver<'a> {
    /// Precompute every callable's scope and every name's candidates.
    #[must_use]
    pub fn new(index: &'a SeamIndex) -> Self {
        Self {
            tables: Tables::new(index),
        }
    }
}

impl CallResolver for NameResolver<'_> {
    fn resolve(&self, _: &SeamIndex, caller: SeamId, site: &CallSite) -> Option<Resolved> {
        let scope = self.tables.scopes.get(&caller)?;
        Some(
            Lookup {
                tables: &self.tables,
                scope,
                site,
            }
            .resolve(),
        )
    }
}

/// One site being resolved.
struct Lookup<'t, 'a> {
    tables: &'t Tables<'a>,
    scope: &'t Scope,
    site: &'t CallSite,
}

impl Lookup<'_, '_> {
    fn resolve(&self) -> Resolved {
        if self.site.external {
            return Resolved::external(self.site.name.clone().unwrap_or_default());
        }
        let Some(name) = self.site.name.as_deref() else {
            return Resolved::unresolved(None);
        };
        let resolved = match (&self.site.receiver, &self.site.qualifier) {
            (Some(Receiver::SelfRef), _) => self
                .own_members(name)
                .map(|found| self.decide(found))
                .unwrap_or_else(|| self.method(name)),
            (Some(Receiver::Super), _) => {
                let found = self.filter(self.members(name, |owner| owner != self.scope.owner));
                if found.is_empty() {
                    Resolved::external(format!("super.{name}"))
                } else {
                    // Which parent is meant takes the type hierarchy, which names alone do
                    // not give — so even one candidate is only a candidate.
                    self.ambiguous(found)
                }
            },
            (Some(Receiver::Path(path)), _) if self.names_container(path) => {
                self.qualified(path, name)
            },
            (Some(_), _) => self.method(name),
            (None, Some(qualifier)) => {
                let segments = site::segments(qualifier);
                match segments.last().map(String::as_str) {
                    None | Some("self" | "crate" | "super") => self.free(name),
                    Some(_) => self.qualified(qualifier, name),
                }
            },
            (None, None) => self.free(name),
        };
        let resolved = self.dispatch(resolved);
        if self.site.weak {
            return weaken(resolved);
        }
        resolved
    }

    /// Members of the caller's own type named `name`.
    fn own_members(&self, name: &str) -> Option<Vec<SeamId>> {
        let owner = self.scope.owner?;
        let found = self.filter(self.members(name, |candidate| candidate == Some(owner)));
        (!found.is_empty()).then_some(found)
    }

    /// Callables named `name` that are members of a type the predicate accepts.
    fn members(&self, name: &str, accept: impl Fn(Option<SeamId>) -> bool) -> Vec<SeamId> {
        self.tables
            .named(name)
            .iter()
            .copied()
            .filter(|id| {
                self.tables
                    .scopes
                    .get(id)
                    .is_some_and(|scope| scope.owner.is_some() && accept(scope.owner))
            })
            .collect()
    }

    /// A method called on a value whose type the structural tier cannot know.
    fn method(&self, name: &str) -> Resolved {
        let found = self.filter(self.members(name, |_| true));
        // A chain's intermediate value — `xs.iter().filter(…)` — is almost always a
        // library type, so a lone same-named member here is a coincidence of names.
        let chained = matches!(self.site.receiver, Some(Receiver::Expr));
        match found.as_slice() {
            [] => Resolved::external(format!(".{name}")),
            // One member by that name in the caller's own package, called on a named
            // value, is taken as the one meant. A same-named method on some library type
            // is possible, which is why the precision ledger counts these.
            [only]
                if !chained
                    && self
                        .tables
                        .scopes
                        .get(only)
                        .is_some_and(|scope| scope.package == self.scope.package) =>
            {
                Resolved::one(Endpoint::Resolved(*only), Resolution::Exact)
            },
            // One contract declaring the name and the rest implementing it: a call through
            // the contract, whose implementations dispatch then lists.
            _ if !chained => match self.contract_among(&found) {
                Some(contract) => Resolved::one(Endpoint::Resolved(contract), Resolution::Exact),
                None => self.ambiguous(found),
            },
            _ => self.ambiguous(found),
        }
    }

    /// The single contract declaration among same-named candidates, when every other
    /// candidate implements that contract.
    ///
    /// Any candidate that does not — an unrelated type's method of the same name — means
    /// the call may be to it instead, and then nothing is established.
    fn contract_among(&self, found: &[SeamId]) -> Option<SeamId> {
        let in_contract = |id: &SeamId| {
            self.tables
                .scopes
                .get(id)
                .is_some_and(|scope| scope.in_contract)
        };
        let mut contracts = found.iter().copied().filter(in_contract);
        let contract = contracts.next()?;
        if contracts.next().is_some() {
            return None;
        }
        let index = self.tables.index;
        let name = index
            .node(contract)
            .and_then(|node| node.parent)
            .and_then(|parent| index.node(parent))
            .map(|parent| parent.name.as_str())?;
        found
            .iter()
            .filter(|id| **id != contract)
            .all(|id| self.mentions(*id, name))
            .then_some(contract)
    }

    /// Whether a receiver is written as a container rather than as a value.
    ///
    /// A receiver bound by an import (`os.path.join`) or spelled like a type
    /// (`Widget.create()`) is a qualifier. Anything else is a value: a local `config`
    /// beside a module named `config` must not be read as that module.
    fn names_container(&self, path: &str) -> bool {
        let segments = site::segments(path);
        let Some(first) = segments.first() else {
            return false;
        };
        let imported = self
            .tables
            .imports(self.scope.file)
            .iter()
            .any(|import| import.binds() == Some(first.as_str()));
        imported || first.chars().next().is_some_and(char::is_uppercase)
    }

    /// The candidates in the container a qualifier names.
    fn qualified(&self, qualifier: &str, name: &str) -> Resolved {
        let segments = self.expand(site::segments(qualifier));
        let Some(last) = segments.last() else {
            return self.free(name);
        };
        let containers: Vec<SeamId> = if last == "Self" {
            self.scope.owner.into_iter().collect()
        } else {
            let all: Vec<SeamId> = self
                .tables
                .containers_named(last)
                .iter()
                .copied()
                .filter(|id| self.tables.fits(*id, self.scope.family))
                .collect();
            // A longer path narrows: `net::Client` prefers the `Client` under `net`.
            match segments.iter().rev().nth(1) {
                Some(parent) => {
                    let narrowed: Vec<SeamId> = all
                        .iter()
                        .copied()
                        .filter(|id| self.tables.under_named(*id, parent))
                        .collect();
                    if narrowed.is_empty() { all } else { narrowed }
                },
                None => all,
            }
        };
        if containers.is_empty() {
            return Resolved::external(format!("{}::{name}", segments.join("::")));
        }
        let found = self.filter(
            self.tables
                .named(name)
                .iter()
                .copied()
                .filter(|id| {
                    let index = self.tables.index;
                    let parent = index.node(*id).and_then(|node| node.parent);
                    let owner = self.tables.scopes.get(id).and_then(|scope| scope.owner);
                    containers
                        .iter()
                        .any(|container| parent == Some(*container) || owner == Some(*container))
                })
                .collect(),
        );
        if found.is_empty() {
            // The container is ours but the member is not written anywhere we read: a
            // derive, a macro, a blanket implementation from elsewhere.
            return Resolved::unresolved(Some(format!("{}::{name}", segments.join("::"))));
        }
        self.decide(found)
    }

    /// A call written with no receiver and no qualifier.
    fn free(&self, name: &str) -> Resolved {
        if self.site.construct
            && let Some(resolved) = self.construct(name)
        {
            return resolved;
        }
        let free = |accept: &dyn Fn(&Scope) -> bool| -> Vec<SeamId> {
            self.filter(
                self.tables
                    .named(name)
                    .iter()
                    .copied()
                    .filter(|id| self.tables.scopes.get(id).is_some_and(accept))
                    .collect(),
            )
        };
        if self.scope.implicit_this
            && let Some(found) = self.own_members(name)
        {
            return self.decide(found);
        }
        // A Rust `mod tests` inside a file shadows the file's own names, so there the
        // module comes first. Where a module can span files — a Kotlin package, a Swift
        // target — a file-private helper beats its namesake elsewhere, so the file does.
        let in_module = || {
            free(&|scope| {
                scope.owner.is_none() && scope.module.is_some() && scope.module == self.scope.module
            })
        };
        let in_file = || free(&|scope| scope.owner.is_none() && scope.file == self.scope.file);
        let tiers: [&dyn Fn() -> Vec<SeamId>; 2] = if self.scope.implicit_this {
            [&in_file, &in_module]
        } else {
            [&in_module, &in_file]
        };
        for tier in tiers {
            let found = tier();
            if !found.is_empty() {
                return self.decide(found);
            }
        }
        if let Some(resolved) = self.imported(name) {
            return resolved;
        }
        if let Some(resolved) = self.construct(name) {
            return resolved;
        }
        let in_package =
            free(&|scope| scope.owner.is_none() && scope.package == self.scope.package);
        if !in_package.is_empty() {
            return self.decide(in_package);
        }
        // Another package's function needs an import to be in scope, and none named it —
        // so even a lone match here is only a candidate.
        let anywhere = free(&|scope| scope.owner.is_none());
        if !anywhere.is_empty() {
            return self.ambiguous(anywhere);
        }
        Resolved::unresolved(Some(name.to_owned()))
    }

    /// A free name the caller's file imports, explicitly or through a glob.
    fn imported(&self, name: &str) -> Option<Resolved> {
        let imports = self.tables.imports(self.scope.file);
        if let Some(import) = imports.iter().find(|import| import.binds() == Some(name)) {
            let target = import.segments.last().map_or(name, String::as_str);
            let parent = import.segments.iter().rev().nth(1);
            let found = self.filter(
                self.tables
                    .named(target)
                    .iter()
                    .copied()
                    .filter(|id| parent.is_none_or(|parent| self.tables.under_named(*id, parent)))
                    .collect(),
            );
            if found.is_empty() {
                return Some(Resolved::external(import.segments.join("::")));
            }
            return Some(self.decide(found));
        }
        let globbed: Vec<SeamId> = imports
            .iter()
            .filter(|import| import.glob)
            .filter_map(|import| import.segments.last())
            .flat_map(|module| {
                self.tables
                    .named(name)
                    .iter()
                    .copied()
                    .filter(|id| self.tables.under_named(*id, module))
            })
            .collect();
        let globbed = self.filter(globbed);
        (!globbed.is_empty()).then(|| self.decide(globbed))
    }

    /// A call naming a type: its constructor, or the type itself when none is written.
    fn construct(&self, name: &str) -> Option<Resolved> {
        let types: Vec<SeamId> = self
            .tables
            .containers_named(name)
            .iter()
            .copied()
            .filter(|id| {
                self.tables
                    .index
                    .node(*id)
                    .is_some_and(|node| node.kind == NodeKind::Type)
                    && self.tables.fits(*id, self.scope.family)
            })
            .collect();
        if types.is_empty() {
            return None;
        }
        let constructors: Vec<SeamId> = types
            .iter()
            .flat_map(|ty| {
                self.tables
                    .index
                    .children(*ty)
                    .iter()
                    .copied()
                    .filter(|child| {
                        self.tables.scopes.contains_key(child)
                            && self.tables.index.node(*child).is_some_and(|node| {
                                CONSTRUCTORS.contains(&node.name.as_str()) || node.name == name
                            })
                    })
            })
            .collect();
        let constructors = self.filter(constructors);
        if constructors.is_empty() {
            // A type with no constructor written is still where the call goes — a
            // struct literal by another name — and it carries no lines of its own.
            return Some(match types.as_slice() {
                [only] => Resolved::one(Endpoint::Resolved(*only), Resolution::Exact),
                _ => self.ambiguous(types),
            });
        }
        Some(self.decide(constructors))
    }

    /// Replace a leading segment bound by an import with the path it was imported from.
    fn expand(&self, segments: Vec<String>) -> Vec<String> {
        let Some(first) = segments.first() else {
            return segments;
        };
        match self
            .tables
            .imports(self.scope.file)
            .iter()
            .find(|import| import.binds() == Some(first.as_str()))
        {
            Some(import) => import
                .segments
                .iter()
                .take(import.bound_prefix())
                .chain(segments.iter().skip(1))
                .cloned()
                .collect(),
            None => segments,
        }
    }

    /// Narrow candidates by family, liveness, and definition over declaration.
    fn filter(&self, candidates: Vec<SeamId>) -> Vec<SeamId> {
        let scopes = &self.tables.scopes;
        let mut kept: Vec<SeamId> = candidates
            .into_iter()
            .filter(|id| {
                scopes
                    .get(id)
                    .is_some_and(|scope| scope.family == self.scope.family)
            })
            .collect();
        kept.sort_unstable();
        kept.dedup();
        // Twins gated for different hosts are one function on any build that has either.
        if kept.iter().any(|id| scopes.get(id).is_some_and(|s| s.live)) {
            kept.retain(|id| scopes.get(id).is_some_and(|s| s.live));
        }
        // A prototype and its definition are one function, and the body is the one with
        // lines. A contract's declaration is not a prototype: it is what dispatch reaches.
        if kept
            .iter()
            .any(|id| scopes.get(id).is_some_and(|s| s.has_body))
        {
            kept.retain(|id| scopes.get(id).is_some_and(|s| s.has_body || s.in_contract));
        }
        kept
    }

    /// One candidate is exact, several are ambiguous.
    fn decide(&self, found: Vec<SeamId>) -> Resolved {
        match found.as_slice() {
            [only] => Resolved::one(Endpoint::Resolved(*only), Resolution::Exact),
            _ => self.ambiguous(found),
        }
    }

    /// Every candidate as an upper-bound edge, unless there are too many to mean anything.
    fn ambiguous(&self, found: Vec<SeamId>) -> Resolved {
        if found.len() > MAX_CANDIDATES {
            let name = self.site.name.as_deref().unwrap_or_default();
            return Resolved::one(
                Endpoint::Unresolved {
                    hint: Some(format!("{name} ×{}", found.len())),
                    resolvable: true,
                },
                Resolution::Ambiguous,
            );
        }
        Resolved {
            targets: found
                .into_iter()
                .map(|id| (Endpoint::Resolved(id), Resolution::Ambiguous))
                .collect(),
        }
    }

    /// Turn an exact call through a contract into its declaration plus implementations.
    fn dispatch(&self, resolved: Resolved) -> Resolved {
        let [(Endpoint::Resolved(target), Resolution::Exact)] = resolved.targets.as_slice() else {
            return resolved;
        };
        let index = self.tables.index;
        let Some(node) = index.node(*target) else {
            return resolved;
        };
        let Some(contract) = node.parent.and_then(|parent| index.node(parent)) else {
            return resolved;
        };
        // A contract's member always dispatches; a type's only when the language marks it
        // as replaceable. Other substitution facets — an `impl Trait` return, a generic
        // bound — say nothing about who answers the call.
        let overridable = contract.kind == NodeKind::Interface
            || (contract.kind == NodeKind::Type
                && OVERRIDABLE
                    .iter()
                    .any(|subtype| node.has_subtype(Lens::Substitution, subtype)));
        if !overridable {
            return resolved;
        }
        let siblings: Vec<SeamId> = self
            .members(&node.name, |owner| owner != Some(contract.id))
            .into_iter()
            .filter(|id| id != target)
            .collect();
        let naming: Vec<SeamId> = siblings
            .iter()
            .copied()
            .filter(|id| self.mentions(*id, &contract.name))
            .collect();
        let implementations = self.filter(if naming.is_empty() { siblings } else { naming });
        let mut targets = vec![(Endpoint::Resolved(*target), Resolution::Dispatch)];
        if implementations.len() <= MAX_CANDIDATES {
            targets.extend(
                implementations
                    .into_iter()
                    .map(|id| (Endpoint::Resolved(id), Resolution::DispatchImpl)),
            );
        } else {
            // Too many to mean anything, but not silently none: the marker says how many.
            targets.push((
                Endpoint::Unresolved {
                    hint: Some(format!(
                        "{} ×{} implementations",
                        node.name,
                        implementations.len()
                    )),
                    resolvable: true,
                },
                Resolution::DispatchImpl,
            ));
        }
        Resolved { targets }
    }

    /// Whether `id`'s implementation or type names `contract` as what it implements.
    fn mentions(&self, id: SeamId, contract: &str) -> bool {
        let index = self.tables.index;
        index.ancestors(id).iter().take(2).any(|up| {
            index.node(*up).is_some_and(|node| {
                (node.kind == NodeKind::Implementation && node.name.contains(contract))
                    // A type that records what it derives from, as Java and C++ do.
                    || (node.kind == NodeKind::Type
                        && node.detail.as_deref().is_some_and(|d| d.contains(contract)))
                    || node
                        .facets_for(Lens::Substitution)
                        .filter_map(|facet| facet.detail.as_deref())
                        .any(|detail| detail.contains(contract))
            })
        })
    }
}

/// A site that is only probably a call is never exact.
fn weaken(resolved: Resolved) -> Resolved {
    Resolved {
        targets: resolved
            .targets
            .into_iter()
            .map(|(endpoint, resolution)| match resolution {
                Resolution::Exact | Resolution::Dispatch => (endpoint, Resolution::Ambiguous),
                other => (endpoint, other),
            })
            .collect(),
    }
}
