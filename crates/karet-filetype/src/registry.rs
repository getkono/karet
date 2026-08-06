//! The static catalogue of recognized file types and the path → [`FileType`]
//! resolver.
//!
//! One table is the single source of truth, keyed by well-known **filename**
//! (matched first, case-insensitively) and by lowercase **extension**. Adding a
//! format is a one-line edit here; see [`docs/file-formats.md`] for the rendered
//! catalogue.
//!
//! [`docs/file-formats.md`]: https://github.com/getkono/karet/blob/master/docs/file-formats.md

use std::path::Path;

use crate::icon::Category;
use crate::icon::IconStyle;

/// The default long-line behavior for an editable file type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrapMode {
    /// Soft-wrap long lines to the editor viewport.
    Wrap,
    /// Keep logical lines intact and allow horizontal scrolling.
    Overflow,
}

/// Static presentation metadata for one recognized file type.
///
/// Resolve one from a path with [`file_type_for_path`]. Icons are resolved per
/// [`IconStyle`] via [`FileType::icon`]: Nerd Font uses a per-type glyph (falling
/// back to the [`Category`]'s glyph), while the Unicode/ASCII tiers use the
/// category's fallback glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileType {
    name: &'static str,
    grammar: Option<&'static str>,
    lsp_language_id: Option<&'static str>,
    config_selector: Option<&'static str>,
    category: Category,
    nerd: Option<char>,
    extensions: &'static [&'static str],
    filenames: &'static [&'static str],
    wrap_mode: WrapMode,
}

impl FileType {
    /// The human-readable display name (e.g. `"Rust"`, `"Markdown"`).
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The tree-sitter grammar identity for this file type, when syntax parsing
    /// is meaningful. This is independent from the display name and does not
    /// imply that the grammar was compiled into a consumer's feature set.
    #[must_use]
    pub fn grammar(&self) -> Option<&'static str> {
        self.grammar
    }

    /// The protocol `languageId` sent to language servers for this file type.
    #[must_use]
    pub fn lsp_language_id(&self) -> Option<&'static str> {
        self.lsp_language_id
    }

    /// The stable selector used by per-language configuration and server routing.
    ///
    /// Selectors intentionally preserve the keys accepted before language
    /// identities were separated (for example, `"c++"`, `"c#"`, and `"tsx"`).
    #[must_use]
    pub fn config_selector(&self) -> Option<&'static str> {
        self.config_selector
    }

    /// The coarse [`Category`] of this file type.
    #[must_use]
    pub fn category(&self) -> Category {
        self.category
    }

    /// Whether this is a recognized type (as opposed to the `"File"` fallback
    /// returned for unknown paths).
    #[must_use]
    pub fn is_recognized(&self) -> bool {
        !matches!(self.category, Category::Unknown)
    }

    /// The icon glyph for this file type in the given [`IconStyle`].
    #[must_use]
    pub fn icon(&self, style: IconStyle) -> char {
        match style {
            IconStyle::NerdFont => self.nerd.unwrap_or(self.category.nerd_icon()),
            IconStyle::Unicode => self.category.unicode_icon(),
            IconStyle::Ascii => self.category.ascii_icon(),
        }
    }

    /// The default long-line behavior for this file type.
    #[must_use]
    pub fn wrap_mode(&self) -> WrapMode {
        self.wrap_mode
    }
}

/// The fallback for an unrecognized file.
const UNKNOWN: FileType = FileType {
    name: "File",
    grammar: None,
    lsp_language_id: None,
    config_selector: None,
    category: Category::Unknown,
    nerd: None,
    extensions: &[],
    filenames: &[],
    wrap_mode: WrapMode::Overflow,
};

/// Compact constructor for an overflow-mode registry entry.
const fn overflow(
    name: &'static str,
    category: Category,
    nerd: Option<char>,
    extensions: &'static [&'static str],
    filenames: &'static [&'static str],
) -> FileType {
    FileType {
        name,
        grammar: None,
        lsp_language_id: None,
        config_selector: None,
        category,
        nerd,
        extensions,
        filenames,
        wrap_mode: WrapMode::Overflow,
    }
}

/// Compact constructor for a source/config format with explicit identities.
const fn language(
    name: &'static str,
    category: Category,
    nerd: Option<char>,
    extensions: &'static [&'static str],
    filenames: &'static [&'static str],
    identities: (Option<&'static str>, &'static str, &'static str),
) -> FileType {
    let (grammar, lsp_language_id, config_selector) = identities;
    FileType {
        name,
        grammar,
        lsp_language_id: Some(lsp_language_id),
        config_selector: Some(config_selector),
        category,
        nerd,
        extensions,
        filenames,
        wrap_mode: WrapMode::Overflow,
    }
}

/// Compact constructor when grammar, protocol, and configuration identities match.
const fn language_named(
    name: &'static str,
    category: Category,
    nerd: Option<char>,
    extensions: &'static [&'static str],
    filenames: &'static [&'static str],
    identity: &'static str,
) -> FileType {
    language(
        name,
        category,
        nerd,
        extensions,
        filenames,
        (Some(identity), identity, identity),
    )
}

/// Compact constructor for a soft-wrapping language with one shared identity.
const fn language_wrap(
    name: &'static str,
    category: Category,
    nerd: Option<char>,
    extensions: &'static [&'static str],
    filenames: &'static [&'static str],
    identity: &'static str,
) -> FileType {
    let mut file_type = language_named(name, category, nerd, extensions, filenames, identity);
    file_type.wrap_mode = WrapMode::Wrap;
    file_type
}

/// Compact constructor for a soft-wrapping registry entry.
const fn wrap(
    name: &'static str,
    category: Category,
    nerd: Option<char>,
    extensions: &'static [&'static str],
    filenames: &'static [&'static str],
) -> FileType {
    FileType {
        name,
        grammar: None,
        lsp_language_id: None,
        config_selector: None,
        category,
        nerd,
        extensions,
        filenames,
        wrap_mode: WrapMode::Wrap,
    }
}

use Category::Archive;
use Category::Binary;
use Category::Code;
use Category::Config;
use Category::Data;
use Category::Document;
use Category::Image;
use Category::Markup;
use Category::Shell;

/// The recognized file types. Filenames win over extensions; first match wins, so
/// keep entries unambiguous (no two entries should claim the same extension).
static REGISTRY: &[FileType] = &[
    // --- programming languages ---
    language_named("Rust", Code, Some('\u{e7a8}'), &["rs"], &[], "rust"),
    language_named(
        "Python",
        Code,
        Some('\u{e606}'),
        &["py", "pyi", "pyw"],
        &[],
        "python",
    ),
    language_named("C", Code, Some('\u{e61e}'), &["c", "h"], &[], "c"),
    language(
        "C++",
        Code,
        Some('\u{e61d}'),
        &["cc", "cpp", "cxx", "hpp", "hh", "hxx"],
        &[],
        (Some("cpp"), "cpp", "c++"),
    ),
    language(
        "C#",
        Code,
        None,
        &["cs"],
        &[],
        (Some("c_sharp"), "csharp", "c#"),
    ),
    language_named("Java", Code, Some('\u{e738}'), &["java"], &[], "java"),
    language_named("Kotlin", Code, None, &["kt", "kts"], &[], "kotlin"),
    language_named("Go", Code, Some('\u{e627}'), &["go"], &[], "go"),
    language_named("Ruby", Code, Some('\u{e739}'), &["rb"], &[], "ruby"),
    language_named("PHP", Code, Some('\u{e73d}'), &["php"], &[], "php"),
    language_named("Swift", Code, None, &["swift"], &[], "swift"),
    language_named("Scala", Code, None, &["scala", "sbt", "sc"], &[], "scala"),
    language_named("Lua", Code, Some('\u{e620}'), &["lua"], &[], "lua"),
    language_named("Haskell", Code, None, &["hs", "lhs"], &[], "haskell"),
    language_named("OCaml", Code, None, &["ml"], &[], "ocaml"),
    language(
        "OCaml",
        Code,
        None,
        &["mli"],
        &[],
        (Some("ocaml-interface"), "ocaml", "ocaml"),
    ),
    language_named("Elixir", Code, None, &["ex", "exs"], &[], "elixir"),
    language_named("Erlang", Code, None, &["erl", "hrl"], &[], "erlang"),
    language_named("Dart", Code, None, &["dart"], &[], "dart"),
    language_named("R", Code, None, &["r"], &[], "r"),
    language_named("Zig", Code, None, &["zig"], &[], "zig"),
    language_named("Perl", Code, None, &["pl", "pm"], &[], "perl"),
    language_named(
        "Clojure",
        Code,
        None,
        &["clj", "cljs", "cljc"],
        &[],
        "clojure",
    ),
    language_named("EDN", Data, None, &["edn"], &[], "edn"),
    language_named("Emacs Lisp", Code, None, &["el"], &[], "elisp"),
    language_named("Vim script", Code, None, &["vim"], &[], "vim"),
    // --- web ---
    language_named(
        "JavaScript",
        Code,
        Some('\u{e74e}'),
        &["js", "mjs", "cjs"],
        &[],
        "javascript",
    ),
    language(
        "JSX",
        Code,
        Some('\u{e7ba}'),
        &["jsx"],
        &[],
        (Some("javascript"), "javascriptreact", "jsx"),
    ),
    language_named(
        "TypeScript",
        Code,
        Some('\u{e628}'),
        &["ts", "mts", "cts"],
        &[],
        "typescript",
    ),
    language(
        "TSX",
        Code,
        Some('\u{e7ba}'),
        &["tsx"],
        &[],
        (Some("tsx"), "typescriptreact", "tsx"),
    ),
    language_named(
        "HTML",
        Markup,
        Some('\u{e736}'),
        &["html", "htm"],
        &[],
        "html",
    ),
    language(
        "HTML",
        Markup,
        Some('\u{e736}'),
        &["xhtml"],
        &[],
        (Some("html"), "html", "html"),
    ),
    language_named("CSS", Markup, Some('\u{e749}'), &["css"], &[], "css"),
    language(
        "Sass",
        Markup,
        Some('\u{e74b}'),
        &["scss"],
        &[],
        (Some("scss"), "scss", "sass"),
    ),
    language(
        "Sass",
        Markup,
        Some('\u{e74b}'),
        &["sass"],
        &[],
        (Some("sass"), "sass", "sass"),
    ),
    language_named("Less", Markup, None, &["less"], &[], "less"),
    language_named("Vue", Markup, None, &["vue"], &[], "vue"),
    language_named("Svelte", Markup, None, &["svelte"], &[], "svelte"),
    language_named("Astro", Markup, None, &["astro"], &[], "astro"),
    language_named("ERB", Markup, Some('\u{e739}'), &["erb"], &[], "erb"),
    // --- data / config ---
    language_named("JSON", Data, Some('\u{e60b}'), &["json"], &[], "json"),
    language(
        "JSON",
        Data,
        Some('\u{e60b}'),
        &["jsonc"],
        &[],
        (Some("json"), "jsonc", "json"),
    ),
    language_named("JSON5", Data, Some('\u{e60b}'), &["json5"], &[], "json5"),
    language_named("YAML", Config, None, &["yml", "yaml"], &[], "yaml"),
    language_named("TOML", Config, None, &["toml"], &[], "toml"),
    language_named("INI", Config, None, &["ini", "cfg", "conf"], &[], "ini"),
    language_named(
        "Properties",
        Config,
        None,
        &["properties"],
        &[],
        "properties",
    ),
    language_named("Pkl", Config, None, &["pkl"], &[], "pkl"),
    language_named("XML", Markup, None, &["xml"], &[], "xml"),
    language(
        "SVG",
        Markup,
        None,
        &["svg"],
        &[],
        (Some("xml"), "xml", "svg"),
    ),
    overflow("CSV", Data, None, &["csv", "tsv"], &[]),
    language_named("SQL", Data, Some('\u{f1c0}'), &["sql"], &[], "sql"),
    language_named("GraphQL", Data, None, &["graphql", "gql"], &[], "graphql"),
    language_named("Protobuf", Data, None, &["proto"], &[], "protobuf"),
    language_named("CBOR", Data, None, &["cbor"], &[], "cbor"),
    overflow("Lockfile", Config, Some('\u{f023}'), &["lock"], &[]),
    // --- shell ---
    language(
        "Shell",
        Shell,
        Some('\u{f489}'),
        &["sh", "bash"],
        &[],
        (Some("bash"), "shellscript", "shell"),
    ),
    language_named("Zsh", Shell, Some('\u{f489}'), &["zsh"], &[], "zsh"),
    language_named("Fish", Shell, Some('\u{f489}'), &["fish"], &[], "fish"),
    // Ksh remains a labelled text format, but has no maintained, compatible
    // grammar. Do not claim a Bash-compatible parser or protocol identity.
    overflow("Ksh", Shell, Some('\u{f489}'), &["ksh"], &[]),
    language_named(
        "PowerShell",
        Shell,
        None,
        &["ps1", "psm1"],
        &[],
        "powershell",
    ),
    language_named("Batch", Shell, None, &["bat", "cmd"], &[], "batch"),
    // --- docs / prose ---
    FileType {
        name: "Markdown",
        grammar: Some("markdown"),
        lsp_language_id: Some("markdown"),
        config_selector: Some("markdown"),
        category: Markup,
        nerd: Some('\u{e73e}'),
        extensions: &["md", "markdown", "mdown", "mkd"],
        filenames: &[],
        wrap_mode: WrapMode::Wrap,
    },
    FileType {
        name: "MDX",
        grammar: Some("mdx"),
        lsp_language_id: Some("mdx"),
        config_selector: Some("mdx"),
        category: Markup,
        nerd: Some('\u{e73e}'),
        extensions: &["mdx"],
        filenames: &[],
        wrap_mode: WrapMode::Wrap,
    },
    wrap(
        "Plain Text",
        Document,
        Some('\u{f15c}'),
        &["txt", "text"],
        &[],
    ),
    language_wrap(
        "reStructuredText",
        Markup,
        None,
        &["rst"],
        &[],
        "restructuredtext",
    ),
    language_wrap(
        "AsciiDoc",
        Markup,
        None,
        &["adoc", "asciidoc"],
        &[],
        "asciidoc",
    ),
    language(
        "TeX",
        Document,
        None,
        &["tex", "sty", "cls"],
        &[],
        (Some("latex"), "latex", "tex"),
    ),
    overflow("PDF", Document, Some('\u{f1c1}'), &["pdf"], &[]),
    wrap(
        "Word",
        Document,
        Some('\u{f1c2}'),
        &["doc", "docx", "odt", "rtf"],
        &[],
    ),
    overflow(
        "Spreadsheet",
        Data,
        Some('\u{f1c3}'),
        &["xls", "xlsx", "ods"],
        &[],
    ),
    overflow(
        "Presentation",
        Document,
        Some('\u{f1c4}'),
        &["ppt", "pptx", "odp"],
        &[],
    ),
    // --- images ---
    overflow(
        "Image",
        Image,
        Some('\u{f1c5}'),
        &[
            "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tiff", "tif",
        ],
        &[],
    ),
    // --- archives ---
    overflow(
        "Archive",
        Archive,
        Some('\u{f1c6}'),
        &[
            "zip", "tar", "gz", "tgz", "bz2", "xz", "7z", "rar", "zst", "jar", "war",
        ],
        &[],
    ),
    // --- media / binary ---
    overflow(
        "Audio",
        Binary,
        Some('\u{f1c7}'),
        &["mp3", "wav", "flac", "ogg", "m4a", "aac"],
        &[],
    ),
    overflow(
        "Video",
        Binary,
        Some('\u{f1c8}'),
        &["mp4", "mkv", "mov", "avi", "webm", "wmv"],
        &[],
    ),
    overflow(
        "Font",
        Binary,
        Some('\u{f031}'),
        &["ttf", "otf", "woff", "woff2", "eot"],
        &[],
    ),
    overflow(
        "Database",
        Data,
        Some('\u{f1c0}'),
        &["db", "sqlite", "sqlite3"],
        &[],
    ),
    overflow(
        "Binary",
        Binary,
        None,
        &[
            "exe", "dll", "so", "dylib", "o", "a", "bin", "wasm", "class",
        ],
        &[],
    ),
    // --- special filenames (matched before extensions) ---
    language_named(
        "Dockerfile",
        Config,
        Some('\u{e7b0}'),
        &[],
        &["Dockerfile", "Containerfile"],
        "dockerfile",
    ),
    language_named(
        "Makefile",
        Config,
        None,
        &["mk"],
        &["Makefile", "GNUmakefile", "makefile"],
        "make",
    ),
    language_named(
        "CMake",
        Config,
        None,
        &["cmake"],
        &["CMakeLists.txt"],
        "cmake",
    ),
    language_wrap(
        "Markdown",
        Markup,
        Some('\u{e73e}'),
        &[],
        &["README"],
        "markdown",
    ),
    overflow(
        "Git config",
        Config,
        Some('\u{f1d3}'),
        &[],
        &[".gitignore", ".gitattributes", ".gitmodules", ".gitkeep"],
    ),
    wrap(
        "License",
        Document,
        Some('\u{f02d}'),
        &[],
        &["LICENSE", "LICENCE", "COPYING", "AUTHORS"],
    ),
    overflow("EditorConfig", Config, None, &[], &[".editorconfig"]),
    overflow("Environment", Config, None, &[], &[".env"]),
];

/// Resolve a path to its [`FileType`], or the `"File"` fallback when unrecognized.
///
/// Matches a well-known filename first (case-insensitively), then a lowercase
/// extension.
#[must_use]
pub fn file_type_for_path(path: &Path) -> FileType {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        for entry in REGISTRY {
            if entry.filenames.iter().any(|f| f.eq_ignore_ascii_case(name)) {
                return *entry;
            }
        }
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext = ext.to_ascii_lowercase();
        for entry in REGISTRY {
            if entry.extensions.iter().any(|e| *e == ext) {
                return *entry;
            }
        }
    }
    UNKNOWN
}

/// The icon glyph for a path in the given [`IconStyle`] — a convenience wrapper
/// over [`file_type_for_path`] + [`FileType::icon`].
#[must_use]
pub fn icon_for_path(path: &Path, style: IconStyle) -> char {
    file_type_for_path(path).icon(style)
}

/// The coarse [`Category`] for a path — a convenience wrapper over
/// [`file_type_for_path`] + [`FileType::category`], used by renderers to tint icons.
#[must_use]
pub fn category_for_path(path: &Path) -> Category {
    file_type_for_path(path).category()
}

#[cfg(test)]
mod tests {
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
    fn unsupported_ksh_is_labelled_without_claiming_a_parser() {
        let ksh = file_type_for_path(Path::new("script.ksh"));
        assert_eq!(ksh.name(), "Ksh");
        assert_eq!(ksh.grammar(), None);
        assert_eq!(ksh.lsp_language_id(), None);
        assert_eq!(ksh.config_selector(), None);
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
}
