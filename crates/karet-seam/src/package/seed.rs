//! Turning a discovered package into the queue its walk starts from.
//!
//! Two shapes, and the difference is where the module tree comes from. Cargo *declares*
//! its tree — `mod net;` says where to look next — so seeding hands over the crate roots
//! and the walk finds the rest as it goes. Every other ecosystem here has its tree on
//! disk, so seeding walks the filesystem up front and creates a node per module before a
//! line of source is read.
//!
//! That ordering is the whole subtlety of the second shape: each module node is the parent
//! its own file's contents attach to, so the nodes must exist first, parents before
//! children.

use std::path::Path;
use std::path::PathBuf;

use super::Pending;
use crate::discover::Discovered;
use crate::discover::PackageKind;
use crate::id::SeamId;
use crate::id::SeamPath;
use crate::id::SeamSegment;
use crate::index::SeamIndex;
use crate::model::ConfigMembership;
use crate::model::Node;
use crate::model::NodeKind;
use crate::model::SeamLocation;
use crate::rollup::Rollups;

/// The queue entries a Cargo package starts from.
fn seed_cargo(entry_points: Vec<PathBuf>, root_id: SeamId) -> Vec<Pending> {
    entry_points
        .into_iter()
        .filter_map(|file| pending_for(file, root_id, true))
        .collect()
}

/// Build a Python package's module nodes, and the queue entries that fill them.
///
/// The nodes have to exist before their files are read: the walk hands back parents before
/// children, and each module node is the parent its own file's contents attach to.
fn seed_python(index: &mut SeamIndex, package: &Discovered, root_id: SeamId) -> Vec<Pending> {
    let mut queue = Vec::new();
    for module in crate::modules::python::walk(&package.root) {
        // An empty import path is the package's own `__init__.py`, whose contents belong
        // to the package node itself rather than to a child module named `__init__`.
        let parent = if module.segments.is_empty() {
            root_id
        } else {
            add_module_node(index, root_id, &module.segments, &module.file)
        };
        queue.extend(pending_for(module.file, parent, false));
    }
    // The drain pops from the back, so reversing keeps files in walk order — parents
    // before children, which is the order their nodes were created in.
    queue.reverse();
    queue
}

/// A queue entry, or `None` when no compiled-in grammar reads this file.
pub(super) fn pending_for(file: PathBuf, parent: SeamId, crate_root: bool) -> Option<Pending> {
    let language = karet_treesitter::language_id_from_path(&file)?;
    Some(Pending {
        file,
        parent,
        language,
        crate_root,
    })
}

/// Add a Python module's node, located in the file it owns.
fn add_module_node(
    index: &mut SeamIndex,
    root_id: SeamId,
    segments: &[String],
    file: &Path,
) -> SeamId {
    let Some(root_segment) = index
        .path(root_id)
        .and_then(|path| path.segments().first().cloned())
    else {
        return root_id;
    };
    let mut full = vec![root_segment];
    full.extend(segments.iter().map(SeamSegment::new));
    let id = index.intern(SeamPath::new(full.clone()));
    let parent = SeamPath::new(full[..full.len() - 1].to_vec());
    let file_id = index.intern_file(file);
    index.insert(Node {
        id,
        kind: NodeKind::Module,
        name: segments.last().cloned().unwrap_or_default(),
        detail: None,
        location: SeamLocation {
            file: file_id,
            range: karet_core::Range::default(),
            span: karet_core::Span::default(),
            selection: karet_core::Range::default(),
            header: karet_core::Range::default(),
        },
        parent: index.resolve(&parent),
        children: Vec::new(),
        facets: Vec::new(),
        visibility: None,
        rollups: Rollups::new(),
        membership: ConfigMembership::Active,
        provisional: false,
    });
    id
}

/// Build a file-tree package's module nodes, and the queue entries that fill them.
///
/// Node, Swift and Gradle share this: their modules are files and their namespaces are
/// directories, so the tree is read off disk rather than out of the source. A directory
/// with no module file of its own still becomes a node — it is a namespace either way —
/// and is marked so its stand-in file is not extracted into it, which would double every
/// node that file already contributes as a module of its own.
fn seed_file_tree(index: &mut SeamIndex, package: &Discovered, root_id: SeamId) -> Vec<Pending> {
    let mut queue = Vec::new();
    let modules = crate::modules::files::walk(
        &package.root,
        package.kind.extensions(),
        package.kind.index_names(),
    );
    for module in modules {
        // An empty namespace path is the root's own index file, whose contents belong to
        // the package node itself rather than to a child module named after it.
        let parent = if module.segments.is_empty() {
            root_id
        } else {
            add_module_node(index, root_id, &module.segments, &module.file)
        };
        if module.extract {
            queue.extend(pending_for(module.file, parent, false));
        }
    }
    // The drain pops from the back, so reversing keeps files in walk order — parents
    // before children, which is the order their nodes were created in.
    queue.reverse();
    queue
}

/// The queue a discovered package starts from, by its ecosystem's rules.
pub(crate) fn seed(
    index: &mut SeamIndex,
    package: &Discovered,
    root_id: SeamId,
    entry_points: Vec<PathBuf>,
) -> Vec<Pending> {
    match package.kind {
        PackageKind::Cargo => seed_cargo(entry_points, root_id),
        PackageKind::Python => seed_python(index, package, root_id),
        PackageKind::Node | PackageKind::Swift | PackageKind::Gradle => {
            seed_file_tree(index, package, root_id)
        },
    }
}
