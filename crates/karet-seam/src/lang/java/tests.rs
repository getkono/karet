//! Java mapping tests, through discovery, extraction and linking.

use crate::edge::EdgeKind;
use crate::edge::Endpoint;
use crate::edge::Resolution;
use crate::index::SeamIndex;
use crate::model::Lens;
use crate::model::NodeKind;
use crate::model::Visibility;
use crate::package::IndexOptions;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Index a Maven module holding `files` under `src/main/java`.
fn index(files: &[(&str, &str)]) -> Result<Option<SeamIndex>, Box<dyn std::error::Error>> {
    if super::language_id().is_none() {
        return Ok(None);
    }
    let dir = tempfile::tempdir()?;
    let module = dir.path().join("shop");
    std::fs::create_dir_all(module.join("src/main/java/shop"))?;
    std::fs::write(module.join("pom.xml"), "<project></project>\n")?;
    for (name, text) in files {
        std::fs::write(module.join("src/main/java/shop").join(name), text)?;
    }
    Ok(Some(crate::package::index_workspace(
        dir.path(),
        IndexOptions::default(),
    )?))
}

fn node<'a>(index: &'a SeamIndex, suffix: &str) -> Result<&'a crate::model::Node, String> {
    let mut found: Vec<&crate::model::Node> = index
        .nodes()
        .filter(|n| {
            index
                .path(n.id)
                .is_some_and(|path| path.to_string().ends_with(suffix))
        })
        .collect();
    found.sort_by_key(|n| n.id);
    found.first().copied().ok_or_else(|| {
        let mut all: Vec<String> = index
            .nodes()
            .filter_map(|n| index.path(n.id).map(ToString::to_string))
            .collect();
        all.sort();
        format!("nothing ends with {suffix}; have {all:?}")
    })
}

fn calls(index: &SeamIndex, suffix: &str) -> Result<Vec<(String, Resolution)>, String> {
    let from = node(index, suffix)?.id;
    let mut out: Vec<(String, Resolution)> = index
        .edges()
        .from_of_kind(from, EdgeKind::Calls)
        .map(|edge| {
            let target = match &edge.to {
                Endpoint::Resolved(to) => {
                    index.path(*to).map(ToString::to_string).unwrap_or_default()
                },
                other => format!("~{}", other.display_hint().unwrap_or("?")),
            };
            (target, edge.resolution.unwrap_or(Resolution::Unresolved))
        })
        .collect();
    out.sort();
    Ok(out)
}

#[test]
fn declarations_map_onto_kinds_and_visibility() -> TestResult {
    let Some(index) = index(&[(
        "Cart.java",
        "package shop;\n\
         public class Cart {\n\
             private int count;\n\
             protected Cart() {}\n\
             int size() { return count; }\n\
             public static final int LIMIT = 9;\n\
         }\n\
         interface Priced { long price(); }\n\
         enum Tier { GOLD, SILVER }\n\
         record Line(String sku) {}\n",
    )])?
    else {
        return Ok(());
    };
    assert_eq!(node(&index, "Cart::Cart")?.kind, NodeKind::Type);
    assert_eq!(
        node(&index, "Cart::count")?.visibility,
        Some(Visibility::Private)
    );
    assert_eq!(
        node(&index, "Cart::Cart::Cart")?.visibility,
        Some(Visibility::Super)
    );
    assert_eq!(
        node(&index, "Cart::size")?.visibility,
        Some(Visibility::Crate)
    );
    assert_eq!(
        node(&index, "Cart::LIMIT")?.visibility,
        Some(Visibility::Public)
    );
    let priced = node(&index, "Cart::Priced")?;
    assert_eq!(priced.kind, NodeKind::Interface);
    // An interface's members are public without saying so.
    assert_eq!(
        node(&index, "Priced::price")?.visibility,
        Some(Visibility::Public)
    );
    assert_eq!(node(&index, "Tier::GOLD")?.kind, NodeKind::Member);
    assert_eq!(node(&index, "Cart::Line")?.kind, NodeKind::Type);
    let api: Vec<&str> = node(&index, "Cart::size")?
        .facets_for(Lens::Api)
        .map(|facet| facet.subtype.name())
        .collect();
    assert_eq!(api, ["package-private"]);
    assert_eq!(
        index.unread_lenses(priced.id),
        [
            Lens::Substitution,
            Lens::Variation,
            Lens::Boundary,
            Lens::Hazard
        ]
    );
    Ok(())
}

#[test]
fn calls_resolve_through_this_implicit_receivers_and_interfaces() -> TestResult {
    let Some(index) = index(&[
        (
            "Shape.java",
            "package shop;\npublic interface Shape {\n    double area();\n}\n",
        ),
        (
            "Square.java",
            "package shop;\npublic class Square implements Shape {\n    public double area() {\n        return side() * this.side();\n    }\n    double side() { return 2; }\n}\n",
        ),
        (
            "Circle.java",
            "package shop;\npublic class Circle implements Shape {\n    public double area() {\n        return Math.max(1, 2);\n    }\n}\n",
        ),
        (
            "Report.java",
            "package shop;\nimport java.util.List;\npublic class Report {\n    double total(Shape s) {\n        Square q = new Square();\n        return s.area() + q.side();\n    }\n}\n",
        ),
    ])?
    else {
        return Ok(());
    };
    let area = calls(&index, "Square::Square::area")?;
    assert_eq!(area.len(), 2, "{area:?}");
    assert!(
        area.iter()
            .all(|(t, r)| t.ends_with("Square::side") && *r == Resolution::Exact)
    );

    assert_eq!(
        calls(&index, "Circle::Circle::area")?,
        [("~Math::max".to_owned(), Resolution::External)]
    );

    let total = calls(&index, "Report::Report::total")?;
    // `s.area()` is a call through the contract: its declaration, and every implementation.
    let dispatch = total
        .iter()
        .filter(|(t, r)| t.ends_with("Shape::area") && *r == Resolution::Dispatch)
        .count();
    let implementations = total
        .iter()
        .filter(|(_, r)| *r == Resolution::DispatchImpl)
        .count();
    assert_eq!((dispatch, implementations), (1, 2), "{total:?}");
    assert!(
        total.iter().any(|(t, _)| t.ends_with("Square::Square")),
        "a constructor call reaches the type: {total:?}"
    );
    Ok(())
}

#[test]
fn a_base_whose_name_merely_contains_the_contract_implements_nothing() -> TestResult {
    let Some(index) = index(&[
        (
            "Node.java",
            "package shop;\npublic interface Node {\n    void visit();\n}\n",
        ),
        (
            "Leaf.java",
            "package shop;\npublic class Leaf implements Node {\n    public void visit() {}\n}\n",
        ),
        (
            "TreeNodeBase.java",
            "package shop;\npublic abstract class TreeNodeBase {}\n",
        ),
        (
            "Branch.java",
            "package shop;\npublic class Branch extends TreeNodeBase {\n    public void visit() {}\n}\n",
        ),
        (
            "Walk.java",
            "package shop;\npublic class Walk {\n    void go(Node n) {\n        n.visit();\n    }\n}\n",
        ),
    ])?
    else {
        return Ok(());
    };
    let go = calls(&index, "Walk::Walk::go")?;
    // `Branch.visit` implements no `Node`, so the call may be to it: nothing is established.
    assert!(
        go.iter().all(|(_, r)| *r == Resolution::Ambiguous),
        "{go:?}"
    );
    assert!(
        go.iter().any(|(t, _)| t.ends_with("Branch::visit")),
        "{go:?}"
    );
    Ok(())
}
