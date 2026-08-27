//! Putting nodes where they belong, once every file has been read.
//!
//! Containment built from syntax alone puts a Rust `impl` beside the type it implements
//! rather than inside it, and the same is true of a Swift `extension`, a Kotlin extension
//! function, and a JavaScript `X.prototype.y = …`. The result is a tree in which "show me
//! everything about `Widget`" is not a question that can be asked.
//!
//! A language cannot fix this itself — it sees one syntax node and never the index — so it
//! reports [`Owner`] candidates and this resolves them. Resolution runs *after* the whole
//! package is in, because a type and the block implementing it routinely live in different
//! files, and the file holding the type may be read second.
//!
//! # What it refuses to do
//!
//! A name that resolves to nothing, or to more than one thing, leaves its node exactly
//! where it was written. Guessing which `Widget` was meant would produce a tree that looks
//! authoritative and is wrong, and the whole value of this index is that a reader can tell
//! what it does not know. The same goes for anything that would not leave a tree: a node
//! never moves into its own subtree, and never across a package boundary.

use std::collections::HashMap;

use crate::id::SeamId;
use crate::index::SeamIndex;
use crate::lang::Owner;
use crate::model::NodeKind;

/// Kinds that can own members written elsewhere.
///
/// A function cannot acquire members and a constant has none, so a candidate resolving to
/// one is a mis-resolution rather than an unusual program.
const OWNING: [NodeKind; 3] = [NodeKind::Type, NodeKind::Interface, NodeKind::Module];

/// Resolve every ownership hint against `index`, moving what resolves.
///
/// Hints are applied in the order they were extracted, which is source order within a
/// file and read order across files, so the tree a package produces does not depend on
/// hash iteration. Each move renumbers the ids beneath it, so the queue is remapped as it
/// goes rather than left holding identities that have since been replaced.
pub fn apply(index: &mut SeamIndex, hints: Vec<(SeamId, Vec<Owner>)>) {
    let mut pending = hints;
    let mut at = 0usize;
    while at < pending.len() {
        let Some((id, owners)) = pending.get(at).cloned() else {
            break;
        };
        at += 1;
        let Some(remap) = attach(index, id, &owners) else {
            continue;
        };
        if remap.is_empty() {
            continue;
        }
        for (held, _) in pending.iter_mut().skip(at) {
            *held = remap.get(held).copied().unwrap_or(*held);
        }
    }
}

/// Try each candidate in turn, applying the first that resolves.
///
/// Returns the id remap the move produced, or `None` when nothing resolved.
fn attach(index: &mut SeamIndex, id: SeamId, owners: &[Owner]) -> Option<HashMap<SeamId, SeamId>> {
    index.node(id)?;
    let owner = owners
        .iter()
        .find_map(|owner| resolve(index, id, &owner.name).map(|target| (owner, target)));
    let (owner, target) = owner?;
    if owner.dissolve {
        Some(dissolve(index, id, target))
    } else {
        Some(index.relocate(id, target, owner.rename.clone()))
    }
}

/// Lift `id`'s children into `target` and drop `id` itself.
///
/// What an inherent `impl` block or a merged `interface` declaration deserves: it is where
/// members were written, not something anyone navigates to, and keeping it would cost a
/// whole column of the spine to say nothing.
fn dissolve(index: &mut SeamIndex, id: SeamId, target: SeamId) -> HashMap<SeamId, SeamId> {
    let mut remap = HashMap::new();
    // By id rather than by iterator: each move renumbers the siblings still to come.
    let mut children = index.children(id).to_vec();
    while let Some(child) = children.first().copied() {
        let moved = index.relocate(child, target, None);
        children.remove(0);
        for held in &mut children {
            *held = moved.get(held).copied().unwrap_or(*held);
        }
        remap.extend(moved);
    }
    index.discard(id);
    remap.insert(id, target);
    remap
}

/// Find the node `name` refers to, seen from `from`.
///
/// Two steps, in the order a reader would take them. Look outward through the enclosing
/// scopes, nearest first, since a name written next to its owner almost always means the
/// one next to it. Failing that, take a package-wide match if there is exactly one — which
/// is what an imported name amounts to, without needing to have read the import.
///
/// Deliberately not a type resolver. It does not follow aliases, does not understand
/// generic arguments, and gives up rather than choosing between two candidates.
fn resolve(index: &SeamIndex, from: SeamId, name: &str) -> Option<SeamId> {
    if name.is_empty() {
        return None;
    }
    let subtree = index.subtree(from);
    let usable = |candidate: SeamId| -> bool {
        candidate != from
            && !subtree.contains(&candidate)
            && index
                .node(candidate)
                .is_some_and(|node| node.name == name && OWNING.contains(&node.kind))
    };

    let mut scope = index.node(from).and_then(|node| node.parent);
    while let Some(here) = scope {
        if let Some(found) = index.children(here).iter().copied().find(|c| usable(*c)) {
            return Some(found);
        }
        scope = index.node(here).and_then(|node| node.parent);
    }

    // Package-wide, and only when the answer is unambiguous. Two types of the same name
    // in one package is legal and common; picking one of them would be a coin toss
    // dressed up as a fact.
    let package = index.path(from)?.package()?.to_owned();
    let mut found = None;
    for node in index.nodes() {
        if !usable(node.id) {
            continue;
        }
        if index.path(node.id).and_then(crate::id::SeamPath::package) != Some(package.as_str()) {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some(node.id);
    }
    found
}

#[cfg(test)]
mod tests {
    use karet_core::Range;
    use karet_core::Span;

    use super::*;
    use crate::id::SeamPath;
    use crate::model::ConfigMembership;
    use crate::model::FileId;
    use crate::model::Node;
    use crate::model::SeamLocation;
    use crate::rollup::Rollups;

    /// Add a node at `path`, at byte `start` of file zero, so source order is testable.
    fn add(index: &mut SeamIndex, path: &str, kind: NodeKind, start: usize) -> SeamId {
        let parsed: SeamPath = path.parse().unwrap_or_default();
        let id = index.intern(parsed.clone());
        let parent = parsed.parent().and_then(|up| index.resolve(&up));
        let name = parsed.leaf().unwrap_or_default().to_owned();
        index.insert(Node {
            id,
            kind,
            name,
            detail: None,
            location: SeamLocation {
                file: FileId(0),
                range: Range::default(),
                span: Span {
                    start: karet_core::BytePos(start),
                    end: karet_core::BytePos(start + 1),
                },
                selection: Range::default(),
                header: Range::default(),
            },
            parent,
            children: Vec::new(),
            facets: Vec::new(),
            visibility: None,
            rollups: Rollups::new(),
            membership: ConfigMembership::Active,
            provisional: false,
        });
        id
    }

    /// Every path in the index, sorted, for whole-shape assertions.
    fn paths(index: &SeamIndex) -> Vec<String> {
        let mut out: Vec<String> = index
            .nodes()
            .filter_map(|node| index.path(node.id).map(ToString::to_string))
            .collect();
        out.sort();
        out
    }

    /// An index holding a module with a type and a block of members beside it.
    fn split_type() -> (SeamIndex, SeamId) {
        let mut index = SeamIndex::new();
        add(&mut index, "pkg", NodeKind::Package, 0);
        add(&mut index, "pkg::m", NodeKind::Module, 10);
        add(&mut index, "pkg::m::Widget", NodeKind::Type, 20);
        let block = add(&mut index, "pkg::m::{block}", NodeKind::Implementation, 30);
        add(&mut index, "pkg::m::{block}::render", NodeKind::Member, 40);
        add(&mut index, "pkg::m::{block}::area", NodeKind::Member, 50);
        (index, block)
    }

    #[test]
    fn a_dissolved_block_leaves_its_members_under_the_owner() {
        let (mut index, block) = split_type();
        apply(&mut index, vec![(block, vec![Owner::dissolved("Widget")])]);
        assert_eq!(
            paths(&index),
            [
                "pkg",
                "pkg::m",
                "pkg::m::Widget",
                "pkg::m::Widget::area",
                "pkg::m::Widget::render",
            ]
        );
        // The block itself is gone, not merely emptied. Its *path* still interns — the
        // interner is append-only — so the tree is what has to be asked.
        let held = index.resolve(&"pkg::m::{block}".parse().unwrap_or_default());
        assert!(held.and_then(|id| index.node(id)).is_none());
    }

    #[test]
    fn a_nested_block_keeps_its_level_and_can_be_renamed() {
        let (mut index, block) = split_type();
        apply(
            &mut index,
            vec![(
                block,
                vec![Owner::nested("Widget").renamed("binding", "{binding}")],
            )],
        );
        assert!(paths(&index).contains(&"pkg::m::Widget::{binding}::render".to_owned()));
        let moved = index
            .resolve(&"pkg::m::Widget::{binding}".parse().unwrap_or_default())
            .and_then(|id| index.node(id));
        assert_eq!(moved.map(|node| node.name.as_str()), Some("binding"));
    }

    #[test]
    fn the_owner_takes_its_new_children_in_source_order() {
        let mut index = SeamIndex::new();
        add(&mut index, "pkg", NodeKind::Package, 0);
        let widget = add(&mut index, "pkg::Widget", NodeKind::Type, 10);
        add(&mut index, "pkg::Widget::id", NodeKind::Member, 20);
        let block = add(&mut index, "pkg::{block}", NodeKind::Implementation, 30);
        add(&mut index, "pkg::{block}::render", NodeKind::Member, 40);
        apply(&mut index, vec![(block, vec![Owner::dissolved("Widget")])]);
        let names: Vec<&str> = index
            .children(widget)
            .iter()
            .filter_map(|id| index.node(*id))
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(names, ["id", "render"]);
    }

    #[test]
    fn a_name_that_resolves_to_nothing_leaves_the_node_where_it_was() {
        let (mut index, block) = split_type();
        let before = paths(&index);
        apply(&mut index, vec![(block, vec![Owner::dissolved("Absent")])]);
        assert_eq!(paths(&index), before);
    }

    #[test]
    fn an_ambiguous_name_is_refused_rather_than_guessed() {
        // Two `Widget`s in one package, neither in the block's own scope. Choosing one
        // would be a coin toss presented as a fact.
        let mut index = SeamIndex::new();
        add(&mut index, "pkg", NodeKind::Package, 0);
        add(&mut index, "pkg::a", NodeKind::Module, 10);
        add(&mut index, "pkg::a::Widget", NodeKind::Type, 20);
        add(&mut index, "pkg::b", NodeKind::Module, 30);
        add(&mut index, "pkg::b::Widget", NodeKind::Type, 40);
        add(&mut index, "pkg::c", NodeKind::Module, 50);
        let block = add(&mut index, "pkg::c::{block}", NodeKind::Implementation, 60);
        let before = paths(&index);
        apply(&mut index, vec![(block, vec![Owner::dissolved("Widget")])]);
        assert_eq!(paths(&index), before);
    }

    #[test]
    fn a_name_in_the_nearest_scope_beats_one_further_out() {
        let mut index = SeamIndex::new();
        add(&mut index, "pkg", NodeKind::Package, 0);
        add(&mut index, "pkg::Widget", NodeKind::Type, 10);
        add(&mut index, "pkg::inner", NodeKind::Module, 20);
        add(&mut index, "pkg::inner::Widget", NodeKind::Type, 30);
        let block = add(
            &mut index,
            "pkg::inner::{block}",
            NodeKind::Implementation,
            40,
        );
        add(
            &mut index,
            "pkg::inner::{block}::render",
            NodeKind::Member,
            50,
        );
        apply(&mut index, vec![(block, vec![Owner::dissolved("Widget")])]);
        assert!(paths(&index).contains(&"pkg::inner::Widget::render".to_owned()));
    }

    #[test]
    fn a_unique_name_elsewhere_in_the_package_is_found() {
        // What an import amounts to, without having had to read the import.
        let mut index = SeamIndex::new();
        add(&mut index, "pkg", NodeKind::Package, 0);
        add(&mut index, "pkg::a", NodeKind::Module, 10);
        add(&mut index, "pkg::a::Widget", NodeKind::Type, 20);
        add(&mut index, "pkg::b", NodeKind::Module, 30);
        let block = add(&mut index, "pkg::b::{block}", NodeKind::Implementation, 40);
        add(&mut index, "pkg::b::{block}::render", NodeKind::Member, 50);
        apply(&mut index, vec![(block, vec![Owner::dissolved("Widget")])]);
        assert!(paths(&index).contains(&"pkg::a::Widget::render".to_owned()));
    }

    #[test]
    fn a_name_in_another_package_is_not_reached_for() {
        let mut index = SeamIndex::new();
        add(&mut index, "one", NodeKind::Package, 0);
        add(&mut index, "one::Widget", NodeKind::Type, 10);
        add(&mut index, "two", NodeKind::Package, 20);
        let block = add(&mut index, "two::{block}", NodeKind::Implementation, 30);
        let before = paths(&index);
        apply(&mut index, vec![(block, vec![Owner::dissolved("Widget")])]);
        assert_eq!(paths(&index), before);
    }

    #[test]
    fn a_node_never_moves_into_its_own_subtree() {
        let mut index = SeamIndex::new();
        add(&mut index, "pkg", NodeKind::Package, 0);
        let outer = add(&mut index, "pkg::Outer", NodeKind::Type, 10);
        add(&mut index, "pkg::Outer::Inner", NodeKind::Type, 20);
        let before = paths(&index);
        apply(&mut index, vec![(outer, vec![Owner::nested("Inner")])]);
        assert_eq!(paths(&index), before);
    }

    #[test]
    fn a_second_block_landing_on_the_same_owner_takes_an_ordinal() {
        let mut index = SeamIndex::new();
        add(&mut index, "pkg", NodeKind::Package, 0);
        add(&mut index, "pkg::Widget", NodeKind::Type, 10);
        let first = add(&mut index, "pkg::{b}", NodeKind::Implementation, 20);
        let second = add(&mut index, "pkg::{b}#2", NodeKind::Implementation, 30);
        apply(
            &mut index,
            vec![
                (first, vec![Owner::nested("Widget")]),
                (second, vec![Owner::nested("Widget")]),
            ],
        );
        let paths = paths(&index);
        assert!(paths.contains(&"pkg::Widget::{b}".to_owned()), "{paths:?}");
        assert!(
            paths.contains(&"pkg::Widget::{b}#2".to_owned()),
            "{paths:?}"
        );
    }

    #[test]
    fn a_later_hint_still_finds_its_node_after_an_earlier_move() {
        // The first move renumbers the second block's id; a queue holding the old one
        // would silently skip it.
        let mut index = SeamIndex::new();
        add(&mut index, "pkg", NodeKind::Package, 0);
        add(&mut index, "pkg::Widget", NodeKind::Type, 10);
        let outer = add(&mut index, "pkg::{b}", NodeKind::Implementation, 20);
        let inner = add(&mut index, "pkg::{b}::{c}", NodeKind::Implementation, 30);
        add(&mut index, "pkg::{b}::{c}::deep", NodeKind::Member, 40);
        apply(
            &mut index,
            vec![
                (outer, vec![Owner::nested("Widget")]),
                (inner, vec![Owner::dissolved("Widget")]),
            ],
        );
        assert!(paths(&index).contains(&"pkg::Widget::deep".to_owned()));
    }

    #[test]
    fn the_first_candidate_that_resolves_wins() {
        let (mut index, block) = split_type();
        apply(
            &mut index,
            vec![(
                block,
                vec![Owner::dissolved("Absent"), Owner::dissolved("Widget")],
            )],
        );
        assert!(paths(&index).contains(&"pkg::m::Widget::render".to_owned()));
    }

    #[test]
    fn a_candidate_naming_something_that_cannot_own_members_is_refused() {
        let mut index = SeamIndex::new();
        add(&mut index, "pkg", NodeKind::Package, 0);
        add(&mut index, "pkg::Widget", NodeKind::Function, 10);
        let block = add(&mut index, "pkg::{block}", NodeKind::Implementation, 20);
        let before = paths(&index);
        apply(&mut index, vec![(block, vec![Owner::dissolved("Widget")])]);
        assert_eq!(paths(&index), before);
    }

    #[test]
    fn applying_no_hints_changes_nothing() {
        let (mut index, _) = split_type();
        let before = paths(&index);
        apply(&mut index, Vec::new());
        assert_eq!(paths(&index), before);
    }
}
