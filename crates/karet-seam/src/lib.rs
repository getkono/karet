//! `karet-seam` — a queryable index of a package's **seams**.
//!
//! A seam, generalizing Feathers, is any location where behavior can be observed,
//! substituted, or varied *without editing the code at that location*. Reading a package
//! by its seams answers a different question than reading it by its files: what is
//! exposed, what can be swapped, what varies before compiling, what crosses the package
//! line, and where that is dangerous.
//!
//! # The shape of the model
//!
//! Containment is a [tree](SeamIndex) and everything else is an [edge](edge::Edge). The
//! two are never merged. A [`Node`] has exactly one parent; a relation that points
//! sideways — an implementation binding a trait, a re-export republishing a name — is an
//! edge, stored and queried separately.
//!
//! Seam properties attach to nodes as [`Facet`]s, each belonging to exactly one of five
//! [`Lens`]es. That set is **closed**: a new language maps into it rather than extending
//! it, which is what makes `substitution` mean the same thing in Rust and in Python.
//! Subtypes within a lens are open, so a language names its own.
//!
//! # What the model refuses to do
//!
//! - **No severity, no score.** A facet is present or absent. Ranking seams would decide
//!   for the reader which ones matter, which is the judgement the view exists to support.
//! - **No position in identity.** A [`SeamPath`] names a node by where it sits in the
//!   hierarchy, never in a file, so view state survives editing (see [`id`]).
//! - **No pretending.** An unresolved edge, a node gated by a predicate that could not be
//!   evaluated, and an index cut short by a file cap are each represented explicitly.
//!   Absence of evidence and evidence of absence are different answers, and conflating
//!   them is the failure mode this crate is built to avoid.
//!
//! # Tiers
//!
//! Facts arrive from sources of differing latency. The **structural** tier is
//! synchronous and always available: it produces a usable tree even from a syntactically
//! invalid buffer. The **semantic** tier resolves edges and effective visibility
//! asynchronously and never gates rendering. The **manifest** tier supplies the
//! configuration set. Nothing structural ever waits on anything semantic.

pub mod config;
pub mod discover;
pub mod edge;
pub mod extract;
pub mod id;
pub mod index;
pub mod lang;
pub mod model;
pub mod modules;
pub mod package;
pub mod query;
pub mod rollup;
pub mod text;

pub use config::CfgEnv;
pub use config::CfgPredicate;
pub use config::Configuration;
pub use config::Truth;
pub use discover::Discovered;
pub use discover::DiscoveryOptions;
pub use discover::PackageKind;
pub use discover::discover;
pub use edge::Edge;
pub use edge::EdgeKind;
pub use edge::EdgeStore;
pub use edge::Endpoint;
pub use extract::ExternalModule;
pub use extract::ExtractError;
pub use extract::ExtractOutcome;
pub use extract::extract_file;
pub use id::SeamId;
pub use id::SeamInterner;
pub use id::SeamPath;
pub use id::SeamPathError;
pub use id::SeamSegment;
pub use index::SeamIndex;
pub use lang::Attribute;
pub use lang::Classified;
pub use lang::FacetContext;
pub use lang::SeamLanguage;
pub use model::ConfigMembership;
pub use model::Effective;
pub use model::Facet;
pub use model::FacetSubtype;
pub use model::FileId;
pub use model::LENSES;
pub use model::Lens;
pub use model::Node;
pub use model::NodeKind;
pub use model::SeamLocation;
pub use model::Visibility;
pub use modules::ModuleSource;
pub use modules::python::PyModule;
pub use package::IndexOptions;
pub use package::PackageError;
pub use package::index_package;
pub use package::index_workspace;
pub use package::reindex_file;
pub use query::Query;
pub use query::QueryError;
pub use query::QueryResult;
pub use query::Term;
pub use query::TermKind;
pub use rollup::Rollups;
pub use text::LineIndex;
