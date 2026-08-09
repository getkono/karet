//! Registry unit tests (out-of-line so the data table stays under the
//! workspace 800-code-line file ceiling).

use super::*;

#[test]
fn resolves_by_extension() {
    assert_eq!(file_type_for_path(Path::new("src/main.rs")).name(), "Rust");
    assert_eq!(file_type_for_path(Path::new("a.MD")).name(), "Markdown");
    assert_eq!(file_type_for_path(Path::new("conf.pkl")).name(), "Pkl");
    assert_eq!(
        file_type_for_path(Path::new("photo.PNG")).category(),
        Category::Image
    );
}

#[test]
fn category_for_path_wraps_file_type() {
    assert_eq!(category_for_path(Path::new("src/main.rs")), Category::Code);
    assert_eq!(category_for_path(Path::new("photo.png")), Category::Image);
    assert_eq!(
        category_for_path(Path::new("mystery.qqq")),
        Category::Unknown
    );
}

#[test]
fn filename_wins_over_extension() {
    // Dockerfile has no extension; matched by name.
    assert_eq!(
        file_type_for_path(Path::new("Dockerfile")).name(),
        "Dockerfile"
    );
    assert_eq!(
        file_type_for_path(Path::new("path/to/.gitignore")).name(),
        "Git config"
    );
    // CMakeLists.txt is a filename rule even though `.txt` exists.
    assert_eq!(
        file_type_for_path(Path::new("CMakeLists.txt")).name(),
        "CMake"
    );
}

#[test]
fn language_identity_axes_are_independent() {
    let cases = [
        (
            "widget.tsx",
            "TSX",
            Some("tsx"),
            Some("typescriptreact"),
            Some("tsx"),
        ),
        (
            "widget.jsx",
            "JSX",
            Some("javascript"),
            Some("javascriptreact"),
            Some("jsx"),
        ),
        ("main.cpp", "C++", Some("cpp"), Some("cpp"), Some("c++")),
        (
            "Program.cs",
            "C#",
            Some("c_sharp"),
            Some("csharp"),
            Some("c#"),
        ),
        (
            "tool.sh",
            "Shell",
            Some("bash"),
            Some("shellscript"),
            Some("shell"),
        ),
        (
            "page.xhtml",
            "HTML",
            Some("html"),
            Some("html"),
            Some("html"),
        ),
        (
            "settings.jsonc",
            "JSON",
            Some("json"),
            Some("jsonc"),
            Some("json"),
        ),
        (
            "CMakeLists.txt",
            "CMake",
            Some("cmake"),
            Some("cmake"),
            Some("cmake"),
        ),
    ];
    for (path, name, grammar, lsp, selector) in cases {
        let file_type = file_type_for_path(Path::new(path));
        assert_eq!(file_type.name(), name, "{path}");
        assert_eq!(file_type.grammar(), grammar, "{path}");
        assert_eq!(file_type.lsp_language_id(), lsp, "{path}");
        assert_eq!(file_type.config_selector(), selector, "{path}");
    }
}

#[test]
fn formats_that_need_distinct_parsers_keep_distinct_identities() {
    for (path, expected) in [
        ("component.mdx", "mdx"),
        ("view.erb", "erb"),
        ("data.json5", "json5"),
        ("shell.zsh", "zsh"),
        ("shell.fish", "fish"),
        ("data.edn", "edn"),
    ] {
        let file_type = file_type_for_path(Path::new(path));
        assert_eq!(file_type.grammar(), Some(expected), "{path}");
        assert_eq!(file_type.config_selector(), Some(expected), "{path}");
    }

    let interface = file_type_for_path(Path::new("library.mli"));
    assert_eq!(interface.grammar(), Some("ocaml-interface"));
    assert_eq!(interface.lsp_language_id(), Some("ocaml"));
    assert_eq!(interface.config_selector(), Some("ocaml"));
}

#[test]
fn extensionless_readme_uses_markdown_without_reclassifying_prose() {
    let readme = file_type_for_path(Path::new("README"));
    assert_eq!(readme.name(), "Markdown");
    assert_eq!(readme.grammar(), Some("markdown"));
    assert_eq!(readme.wrap_mode(), WrapMode::Wrap);

    for path in ["LICENSE", "COPYING", "AUTHORS", "notes"] {
        assert_ne!(
            file_type_for_path(Path::new(path)).grammar(),
            Some("markdown")
        );
    }
}

#[test]
fn unknown_extension_falls_back() {
    let ft = file_type_for_path(Path::new("mystery.zzz"));
    assert_eq!(ft.name(), "File");
    assert_eq!(ft.category(), Category::Unknown);
    assert_eq!(ft.wrap_mode(), WrapMode::Overflow);
}

#[test]
fn prose_wraps_and_source_formats_overflow() {
    for path in [
        "README",
        "notes.md",
        "notes.txt",
        "guide.rst",
        "guide.asciidoc",
        "draft.docx",
    ] {
        assert_eq!(
            file_type_for_path(Path::new(path)).wrap_mode(),
            WrapMode::Wrap,
            "{path} should wrap"
        );
    }
    for path in ["main.rs", "page.html", "config.toml", "paper.tex"] {
        assert_eq!(
            file_type_for_path(Path::new(path)).wrap_mode(),
            WrapMode::Overflow,
            "{path} should overflow"
        );
    }
}

#[test]
fn icon_varies_by_style() {
    let rust = file_type_for_path(Path::new("x.rs"));
    // Nerd Font uses the per-type glyph; Unicode/ASCII use category fallbacks.
    assert_eq!(rust.icon(IconStyle::NerdFont), '\u{e7a8}');
    assert_eq!(rust.icon(IconStyle::Unicode), Category::Code.unicode_icon());
    assert_eq!(rust.icon(IconStyle::Ascii), ' ');
}

#[test]
fn type_without_specific_glyph_uses_category() {
    let kt = file_type_for_path(Path::new("Main.kt"));
    assert_eq!(kt.icon(IconStyle::NerdFont), Category::Code.nerd_icon());
}

#[test]
fn icon_for_path_matches_resolution() {
    assert_eq!(
        icon_for_path(Path::new("a.rs"), IconStyle::NerdFont),
        file_type_for_path(Path::new("a.rs")).icon(IconStyle::NerdFont)
    );
}
