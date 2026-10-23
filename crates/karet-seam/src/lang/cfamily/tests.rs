//! C and C++ mapping tests, through discovery, extraction, regrouping and linking.

use crate::edge::EdgeKind;
use crate::edge::Endpoint;
use crate::edge::Resolution;
use crate::id::SeamPath;
use crate::index::SeamIndex;
use crate::model::Lens;
use crate::model::NodeKind;
use crate::model::Visibility;
use crate::package::IndexOptions;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Index a tree of files written under a temporary root.
fn index(files: &[(&str, &str)]) -> Result<Option<SeamIndex>, Box<dyn std::error::Error>> {
    if super::language_ids().len() < 2 {
        return Ok(None);
    }
    let dir = tempfile::tempdir()?;
    for (path, text) in files {
        let path = dir.path().join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)?;
    }
    Ok(Some(crate::package::index_workspace(
        dir.path(),
        IndexOptions::default(),
    )?))
}

fn node<'a>(index: &'a SeamIndex, path: &str) -> Result<&'a crate::model::Node, String> {
    let parsed: SeamPath = path.parse().map_err(|_| format!("bad path {path}"))?;
    index
        .resolve(&parsed)
        .and_then(|id| index.node(id))
        .ok_or_else(|| format!("no node at {path}; have {:?}", paths(index)))
}

fn paths(index: &SeamIndex) -> Vec<String> {
    let mut out: Vec<String> = index
        .nodes()
        .filter_map(|n| index.path(n.id).map(ToString::to_string))
        .collect();
    out.sort();
    out
}

/// Calls leaving `path`, as `(target or display, resolution)`, sorted.
fn calls(index: &SeamIndex, path: &str) -> Result<Vec<(String, Resolution)>, String> {
    let from = node(index, path)?.id;
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

fn api(index: &SeamIndex, path: &str) -> Result<Vec<&'static str>, String> {
    Ok(node(index, path)?
        .facets_for(Lens::Api)
        .map(|facet| facet.subtype.name())
        .collect())
}

const CMAKE: (&str, &str) = ("lib/CMakeLists.txt", "add_library(lib widget.c)\n");

#[test]
fn c_declarations_map_onto_kinds_and_linkage() -> TestResult {
    let Some(index) = index(&[
        CMAKE,
        (
            "lib/widget.c",
            "#include \"widget.h\"\n\
             struct point { int x; int y; };\n\
             typedef struct { int w; } size_t2;\n\
             typedef struct named { int h; } named;\n\
             static int helper(void) { return 1; }\n\
             int area(struct point *p) { return p->x * helper(); }\n",
        ),
    ])?
    else {
        return Ok(());
    };
    assert_eq!(node(&index, "lib::widget.c::point")?.kind, NodeKind::Type);
    assert_eq!(
        node(&index, "lib::widget.c::point::x")?.kind,
        NodeKind::Member
    );
    // C struct members are public: no keyword makes them anything else.
    assert_eq!(api(&index, "lib::widget.c::point::x")?, ["public"]);
    assert_eq!(node(&index, "lib::widget.c::size_t2")?.kind, NodeKind::Type);
    // `typedef struct named {…} named` is one type, not a type inside a type.
    assert!(node(&index, "lib::widget.c::named::named").is_err());
    assert_eq!(
        node(&index, "lib::widget.c::named::h")?.kind,
        NodeKind::Member
    );
    assert_eq!(
        node(&index, "lib::widget.c::area")?.kind,
        NodeKind::Function
    );
    assert_eq!(api(&index, "lib::widget.c::helper")?, ["static"]);
    assert_eq!(
        node(&index, "lib::widget.c::helper")?.visibility,
        Some(Visibility::Private)
    );
    assert_eq!(api(&index, "lib::widget.c::area")?, ["extern"]);
    Ok(())
}

#[test]
fn a_header_and_its_source_are_two_modules_and_the_call_reaches_the_definition() -> TestResult {
    let Some(index) = index(&[
        CMAKE,
        ("lib/widget.h", "int area(int w);\n"),
        (
            "lib/widget.c",
            "#include \"widget.h\"\nstatic int sq(int w) { return w * w; }\nint area(int w) {\n    return sq(w);\n}\n",
        ),
        (
            "lib/main.c",
            "#include <stdio.h>\n#include \"widget.h\"\nint main(void) {\n    printf(\"%d\", area(2));\n    return 0;\n}\n",
        ),
    ])?
    else {
        return Ok(());
    };
    // Same stem, different files: neither replaces the other.
    let prototype = node(&index, "lib::widget.h::area")?;
    let definition = node(&index, "lib::widget.c::area")?;
    assert_eq!(
        index
            .file_path(definition.location.file)
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str()),
        Some("widget.c")
    );
    assert!(
        index
            .calls()
            .callable(definition.id)
            .is_some_and(|c| c.has_body)
    );
    assert!(
        index
            .calls()
            .callable(prototype.id)
            .is_some_and(|c| !c.has_body)
    );
    assert_eq!(
        calls(&index, "lib::widget.c::area")?,
        [("lib::widget.c::sq".to_owned(), Resolution::Exact)]
    );

    let found = calls(&index, "lib::main.c::main")?;
    assert!(
        found.contains(&("lib::widget.c::area".to_owned(), Resolution::Exact)),
        "{found:?}"
    );
    assert!(
        found.contains(&("~printf".to_owned(), Resolution::External)),
        "{found:?}"
    );
    Ok(())
}

#[test]
fn a_call_through_a_function_pointer_is_unresolved_not_external() -> TestResult {
    let Some(index) = index(&[
        CMAKE,
        (
            "lib/dev.c",
            "struct ops { int (*read)(int); };\nint pull(struct ops *o) {\n    return o->read(1);\n}\n",
        ),
    ])?
    else {
        return Ok(());
    };
    assert_eq!(
        calls(&index, "lib::dev.c::pull")?,
        [("~?".to_owned(), Resolution::Unresolved)]
    );
    Ok(())
}

#[test]
fn a_variable_holding_a_function_is_not_a_function() -> TestResult {
    let Some(index) = index(&[
        CMAKE,
        (
            "lib/hook.c",
            "int (*handler)(int) = 0;\nint fire(void) {\n    void (*local)(void) = 0;\n    return handler(1);\n}\n",
        ),
    ])?
    else {
        return Ok(());
    };
    assert!(node(&index, "lib::hook.c::handler").is_err());
    assert!(node(&index, "lib::hook.c::fire::local").is_err());
    // Nothing named `handler` was written as a function, so the call reaches nothing.
    assert_eq!(
        calls(&index, "lib::hook.c::fire")?,
        [("~handler".to_owned(), Resolution::Unresolved)]
    );
    Ok(())
}

#[test]
fn a_directory_whose_first_source_is_vendored_still_holds_its_own() -> TestResult {
    // Qt's layout: `src/` has no file of its own, and `3rdparty` sorts first.
    let Some(index) = index(&[
        ("lib/CMakeLists.txt", "add_subdirectory(src)\n"),
        (
            "lib/src/3rdparty/z/z.c",
            "int inflate(void) { return 0; }\n",
        ),
        ("lib/src/engine/x.c", "int run(void) { return 1; }\n"),
    ])?
    else {
        return Ok(());
    };
    let run = node(&index, "lib::src::engine::x.c::run")?;
    assert_eq!(index.roots().len(), 1, "{:?}", paths(&index));
    assert_eq!(index.ancestors(run.id).last(), index.roots().first());
    assert!(paths(&index).iter().all(|path| !path.contains("3rdparty")));
    Ok(())
}

#[test]
fn vendored_code_is_not_part_of_the_project() -> TestResult {
    let Some(index) = index(&[
        CMAKE,
        ("lib/a.c", "int f(void) { return 0; }\n"),
        (
            "lib/third_party/zlib/CMakeLists.txt",
            "add_library(z z.c)\n",
        ),
        (
            "lib/third_party/zlib/z.c",
            "int inflate(void) { return 0; }\n",
        ),
    ])?
    else {
        return Ok(());
    };
    assert!(
        paths(&index).iter().all(|path| !path.contains("inflate")),
        "{:?}",
        paths(&index)
    );
    Ok(())
}

#[test]
fn a_cpp_method_defined_outside_its_class_is_regrouped_and_reached() -> TestResult {
    let Some(index) = index(&[
        CMAKE,
        (
            "lib/widget.hpp",
            "namespace ui {\nclass Widget {\npublic:\n    int draw();\n    const int& name() const;\nprivate:\n    int size();\n    int cached;\n};\n}\n",
        ),
        (
            "lib/widget.cpp",
            "#include \"widget.hpp\"\n\
             int ui::Widget::size() {\n    return 4;\n}\n\
             const int& ui::Widget::name() const {\n    return cached;\n}\n\
             int ui::Widget::draw() {\n    return size() + this->size() + name();\n}\n",
        ),
    ])?
    else {
        return Ok(());
    };
    assert_eq!(node(&index, "lib::widget.hpp::ui")?.kind, NodeKind::Module);
    let widget = node(&index, "lib::widget.hpp::ui::Widget")?;
    assert_eq!(widget.kind, NodeKind::Type);
    assert_eq!(
        api(&index, "lib::widget.hpp::ui::Widget::draw")?,
        ["public"]
    );
    assert_eq!(
        api(&index, "lib::widget.hpp::ui::Widget::cached")?,
        ["private"]
    );
    // A method returning a reference is a method like any other.
    assert_eq!(
        node(&index, "lib::widget.hpp::ui::Widget::name")?.kind,
        NodeKind::Member
    );

    // The out-of-line definitions sit under the class beside their declarations, and
    // claim no access of their own.
    let defined = node(&index, "lib::widget.hpp::ui::Widget::draw#2")?;
    assert_eq!(defined.visibility, None);
    assert!(api(&index, "lib::widget.hpp::ui::Widget::draw#2")?.is_empty());

    let draw = calls(&index, "lib::widget.hpp::ui::Widget::draw#2")?;
    // A bare `size()` and `this->size()` both reach the definition, not the declaration.
    assert_eq!(
        draw,
        [
            (
                "lib::widget.hpp::ui::Widget::name#2".to_owned(),
                Resolution::Exact
            ),
            (
                "lib::widget.hpp::ui::Widget::size#2".to_owned(),
                Resolution::Exact
            ),
            (
                "lib::widget.hpp::ui::Widget::size#2".to_owned(),
                Resolution::Exact
            )
        ]
    );
    Ok(())
}

#[test]
fn a_function_in_an_anonymous_namespace_has_internal_linkage() -> TestResult {
    let Some(index) = index(&[
        CMAKE,
        (
            "lib/a.cpp",
            "namespace {\nint hidden() { return 1; }\nnamespace detail {\nint deeper() { return 2; }\n}\n}\nnamespace ui {\nint shown() { return hidden(); }\n}\n",
        ),
    ])?
    else {
        return Ok(());
    };
    let hidden = node(&index, "lib::a.cpp::{anonymous}::hidden")?;
    assert_eq!(hidden.visibility, Some(Visibility::Private));
    assert_eq!(api(&index, "lib::a.cpp::{anonymous}::hidden")?, ["static"]);
    // However deep, the anonymous namespace around it decides.
    assert_eq!(
        api(&index, "lib::a.cpp::{anonymous}::detail::deeper")?,
        ["static"]
    );
    assert_eq!(api(&index, "lib::a.cpp::ui::shown")?, ["extern"]);
    assert_eq!(
        node(&index, "lib::a.cpp::ui::shown")?.visibility,
        Some(Visibility::Public)
    );
    Ok(())
}

#[test]
fn only_a_function_carries_linkage_outside_a_class() -> TestResult {
    let Some(index) = index(&[
        CMAKE,
        (
            "lib/shape.hpp",
            "namespace geo {\nstruct Size { int w; };\nclass Box { public: int area(); };\nenum Unit { Px };\ntypedef int length;\nint measure(Size s);\n}\n",
        ),
    ])?
    else {
        return Ok(());
    };
    for path in [
        "lib::shape.hpp::geo",
        "lib::shape.hpp::geo::Size",
        "lib::shape.hpp::geo::Box",
        "lib::shape.hpp::geo::Unit",
        "lib::shape.hpp::geo::length",
    ] {
        assert!(api(&index, path)?.is_empty(), "{path}");
    }
    assert_eq!(api(&index, "lib::shape.hpp::geo::measure")?, ["extern"]);
    // Inside a class, access still applies to what it holds.
    assert_eq!(api(&index, "lib::shape.hpp::geo::Size::w")?, ["public"]);
    assert_eq!(api(&index, "lib::shape.hpp::geo::Box::area")?, ["public"]);
    Ok(())
}

#[test]
fn a_qualified_definition_in_a_namespace_is_a_free_function() -> TestResult {
    let Some(index) = index(&[
        CMAKE,
        (
            "lib/geo.hpp",
            "namespace geo {\nint scale(int x);\nclass Box { public: int area(); };\n}\n",
        ),
        (
            "lib/geo.cpp",
            "#include \"geo.hpp\"\nint geo::scale(int x) { return x * 2; }\nint geo::Box::area() { return geo::scale(2); }\n",
        ),
    ])?
    else {
        return Ok(());
    };
    let scale = node(&index, "lib::geo.hpp::geo::scale#2")?;
    assert_eq!(scale.kind, NodeKind::Function);
    assert_eq!(scale.visibility, Some(Visibility::Public));
    assert_eq!(api(&index, "lib::geo.hpp::geo::scale#2")?, ["extern"]);
    // Owned by a class, the same shape stays a member that claims no access.
    let area = node(&index, "lib::geo.hpp::geo::Box::area#2")?;
    assert_eq!(area.kind, NodeKind::Member);
    assert_eq!(area.visibility, None);
    Ok(())
}

#[test]
fn a_struct_template_keeps_struct_access_whatever_its_parameters_say() -> TestResult {
    let Some(index) = index(&[
        CMAKE,
        (
            "lib/box.hpp",
            "// A helper class for boxes.\ntemplate <class T> struct Box {\n    T value;\n};\nclass Lid {\n    int seal;\n};\n",
        ),
    ])?
    else {
        return Ok(());
    };
    assert_eq!(api(&index, "lib::box.hpp::Box::value")?, ["public"]);
    assert_eq!(api(&index, "lib::box.hpp::Lid::seal")?, ["private"]);
    Ok(())
}

#[test]
fn braces_and_labels_in_comments_and_literals_do_not_change_member_access() -> TestResult {
    let Some(index) = index(&[
        CMAKE,
        (
            "lib/lexer.hpp",
            "class Lexer {\n    // a closing } and a public: label in a comment\n    /* } public: */\n    bool closes(char c) { return c == '}'; }\n    const char *tag = \"} public:\";\n    int depth;\nprotected:\n    int line;\n};\n",
        ),
    ])?
    else {
        return Ok(());
    };
    assert_eq!(api(&index, "lib::lexer.hpp::Lexer::closes")?, ["private"]);
    assert_eq!(api(&index, "lib::lexer.hpp::Lexer::tag")?, ["private"]);
    assert_eq!(api(&index, "lib::lexer.hpp::Lexer::depth")?, ["private"]);
    assert_eq!(api(&index, "lib::lexer.hpp::Lexer::line")?, ["protected"]);
    Ok(())
}

#[test]
fn a_qualified_call_into_the_standard_library_is_external() -> TestResult {
    let Some(index) = index(&[
        CMAKE,
        (
            "lib/sort.cpp",
            "#include <algorithm>\nvoid order(int *xs, int n) {\n    std::sort(xs, xs + n);\n}\n",
        ),
    ])?
    else {
        return Ok(());
    };
    assert_eq!(
        calls(&index, "lib::sort.cpp::order")?,
        [("~std::sort".to_owned(), Resolution::External)]
    );
    Ok(())
}

#[test]
fn only_containment_and_visibility_are_read() -> TestResult {
    let Some(index) = index(&[CMAKE, ("lib/a.c", "int f(void) { return 0; }\n")])? else {
        return Ok(());
    };
    let f = node(&index, "lib::a.c::f")?;
    assert_eq!(
        index.unread_lenses(f.id),
        [
            Lens::Substitution,
            Lens::Variation,
            Lens::Boundary,
            Lens::Hazard
        ]
    );
    // A package sits in its build file and was read by no single language.
    let root = index.roots().first().copied().ok_or("no root")?;
    assert!(index.unread_lenses(root).is_empty());
    Ok(())
}
