//! The registry rows: every recognized file type with its identities, icon,
//! wrap mode, comment tokens and auto-closing pairs, set per row so a type's
//! editing syntax can never drift from its name.

use super::FileType;
use super::WrapMode;
use crate::comment::C_STYLE;
use crate::comment::CommentTokens;
use crate::comment::HASH;
use crate::comment::MARKUP;
use crate::comment::block;
use crate::comment::both;
use crate::comment::line;
use crate::icon::Category;
use crate::pairs::BACKTICK;
use crate::pairs::DOUBLE;
use crate::pairs::MARKDOWN;
use crate::pairs::QUOTES;

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
        comments: CommentTokens::NONE,
        pairs: DOUBLE,
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
        comments: CommentTokens::NONE,
        pairs: DOUBLE,
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
        comments: CommentTokens::NONE,
        pairs: DOUBLE,
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
pub(super) static REGISTRY: &[FileType] = &[
    // --- programming languages ---
    language_named("Rust", Code, Some('\u{e7a8}'), &["rs"], &[], "rust").syntax(C_STYLE, DOUBLE),
    language_named(
        "Python",
        Code,
        Some('\u{e606}'),
        &["py", "pyi", "pyw"],
        &[],
        "python",
    )
    .syntax(HASH, QUOTES),
    language_named("C", Code, Some('\u{e61e}'), &["c", "h"], &[], "c").syntax(C_STYLE, QUOTES),
    language(
        "C++",
        Code,
        Some('\u{e61d}'),
        &["cc", "cpp", "cxx", "hpp", "hh", "hxx"],
        &[],
        (Some("cpp"), "cpp", "c++"),
    )
    .syntax(C_STYLE, QUOTES),
    language(
        "C#",
        Code,
        None,
        &["cs"],
        &[],
        (Some("c_sharp"), "csharp", "c#"),
    )
    .syntax(C_STYLE, QUOTES),
    language_named("Java", Code, Some('\u{e738}'), &["java"], &[], "java").syntax(C_STYLE, QUOTES),
    language_named("Kotlin", Code, None, &["kt", "kts"], &[], "kotlin").syntax(C_STYLE, BACKTICK),
    language_named("Go", Code, Some('\u{e627}'), &["go"], &[], "go").syntax(C_STYLE, BACKTICK),
    language_named("Ruby", Code, Some('\u{e739}'), &["rb"], &[], "ruby").syntax(HASH, QUOTES),
    language_named("PHP", Code, Some('\u{e73d}'), &["php"], &[], "php").syntax(C_STYLE, QUOTES),
    language_named("Swift", Code, None, &["swift"], &[], "swift").syntax(C_STYLE, QUOTES),
    language_named("Scala", Code, None, &["scala", "sbt", "sc"], &[], "scala")
        .syntax(C_STYLE, BACKTICK),
    language_named("Lua", Code, Some('\u{e620}'), &["lua"], &[], "lua")
        .syntax(both("--", "--[[", "]]"), QUOTES),
    language_named("Haskell", Code, None, &["hs", "lhs"], &[], "haskell")
        .syntax(both("--", "{-", "-}"), DOUBLE),
    language_named("OCaml", Code, None, &["ml"], &[], "ocaml").syntax(block("(*", "*)"), DOUBLE),
    language(
        "OCaml",
        Code,
        None,
        &["mli"],
        &[],
        (Some("ocaml-interface"), "ocaml", "ocaml"),
    )
    .syntax(block("(*", "*)"), DOUBLE),
    language_named("Elixir", Code, None, &["ex", "exs"], &[], "elixir").syntax(HASH, QUOTES),
    language_named("Erlang", Code, None, &["erl", "hrl"], &[], "erlang").syntax(line("%"), QUOTES),
    language_named("Dart", Code, None, &["dart"], &[], "dart").syntax(C_STYLE, BACKTICK),
    language_named("R", Code, None, &["r"], &[], "r").syntax(HASH, QUOTES),
    language_named("Zig", Code, None, &["zig"], &[], "zig").syntax(line("//"), QUOTES),
    language_named("Perl", Code, None, &["pl", "pm"], &[], "perl").syntax(HASH, QUOTES),
    language_named(
        "Clojure",
        Code,
        None,
        &["clj", "cljs", "cljc"],
        &[],
        "clojure",
    )
    .syntax(line(";"), DOUBLE),
    language_named("EDN", Data, None, &["edn"], &[], "edn").syntax(line(";"), DOUBLE),
    language_named("Emacs Lisp", Code, None, &["el"], &[], "elisp").syntax(line(";"), DOUBLE),
    language_named("Vim script", Code, None, &["vim"], &[], "vim").syntax(line("\""), DOUBLE),
    // --- web ---
    language_named(
        "JavaScript",
        Code,
        Some('\u{e74e}'),
        &["js", "mjs", "cjs"],
        &[],
        "javascript",
    )
    .syntax(C_STYLE, BACKTICK),
    language(
        "JSX",
        Code,
        Some('\u{e7ba}'),
        &["jsx"],
        &[],
        (Some("javascript"), "javascriptreact", "jsx"),
    )
    .syntax(C_STYLE, BACKTICK),
    language_named(
        "TypeScript",
        Code,
        Some('\u{e628}'),
        &["ts", "mts", "cts"],
        &[],
        "typescript",
    )
    .syntax(C_STYLE, BACKTICK),
    language(
        "TSX",
        Code,
        Some('\u{e7ba}'),
        &["tsx"],
        &[],
        (Some("tsx"), "typescriptreact", "tsx"),
    )
    .syntax(C_STYLE, BACKTICK),
    language_named(
        "HTML",
        Markup,
        Some('\u{e736}'),
        &["html", "htm"],
        &[],
        "html",
    )
    .syntax(MARKUP, QUOTES),
    language(
        "HTML",
        Markup,
        Some('\u{e736}'),
        &["xhtml"],
        &[],
        (Some("html"), "html", "html"),
    )
    .syntax(MARKUP, QUOTES),
    language_named("CSS", Markup, Some('\u{e749}'), &["css"], &[], "css")
        .syntax(block("/*", "*/"), QUOTES),
    language(
        "Sass",
        Markup,
        Some('\u{e74b}'),
        &["scss"],
        &[],
        (Some("scss"), "scss", "sass"),
    )
    .syntax(C_STYLE, QUOTES),
    language(
        "Sass",
        Markup,
        Some('\u{e74b}'),
        &["sass"],
        &[],
        (Some("sass"), "sass", "sass"),
    )
    .syntax(C_STYLE, QUOTES),
    language_named("Less", Markup, None, &["less"], &[], "less").syntax(C_STYLE, QUOTES),
    language_named("Vue", Markup, None, &["vue"], &[], "vue").syntax(MARKUP, BACKTICK),
    language_named("Svelte", Markup, None, &["svelte"], &[], "svelte").syntax(MARKUP, BACKTICK),
    language_named("Astro", Markup, None, &["astro"], &[], "astro").syntax(MARKUP, BACKTICK),
    language_named("ERB", Markup, Some('\u{e739}'), &["erb"], &[], "erb")
        .syntax(block("<%#", "%>"), QUOTES),
    // --- data / config ---
    language_named("JSON", Data, Some('\u{e60b}'), &["json"], &[], "json"),
    language(
        "JSON",
        Data,
        Some('\u{e60b}'),
        &["jsonc"],
        &[],
        (Some("json"), "jsonc", "json"),
    )
    .syntax(C_STYLE, DOUBLE),
    language_named("JSON5", Data, Some('\u{e60b}'), &["json5"], &[], "json5")
        .syntax(C_STYLE, QUOTES),
    language_named("YAML", Config, None, &["yml", "yaml"], &[], "yaml").syntax(HASH, QUOTES),
    language_named("TOML", Config, None, &["toml"], &[], "toml").syntax(HASH, QUOTES),
    language_named("INI", Config, None, &["ini", "cfg", "conf"], &[], "ini")
        .syntax(line(";"), QUOTES),
    language_named(
        "Properties",
        Config,
        None,
        &["properties"],
        &[],
        "properties",
    )
    .syntax(HASH, QUOTES),
    // No tree-sitter grammar for Pkl is bundled: recognized (name, LSP id,
    // config selector) but explicitly grammar-less until one exists.
    language("Pkl", Config, None, &["pkl"], &[], (None, "pkl", "pkl")).syntax(C_STYLE, QUOTES),
    // Mermaid sources open as plain text; the markdown preview is where
    // diagrams render (no dedicated grammar is bundled).
    wrap("Mermaid", Markup, None, &["mmd", "mermaid"], &[]).syntax(line("%%"), DOUBLE),
    language_named("XML", Markup, None, &["xml"], &[], "xml").syntax(MARKUP, QUOTES),
    language(
        "SVG",
        Markup,
        None,
        &["svg"],
        &[],
        (Some("xml"), "xml", "svg"),
    )
    .syntax(MARKUP, QUOTES),
    overflow("CSV", Data, None, &["csv", "tsv"], &[]),
    language_named("SQL", Data, Some('\u{f1c0}'), &["sql"], &[], "sql")
        .syntax(both("--", "/*", "*/"), BACKTICK),
    language_named(
        "GraphQL",
        Data,
        None,
        &["graphql", "gql", "graphqls"],
        &[],
        "graphql",
    )
    .syntax(HASH, QUOTES),
    language_named("Protobuf", Data, None, &["proto"], &[], "protobuf").syntax(C_STYLE, QUOTES),
    language_named("CBOR", Data, None, &["cbor"], &[], "cbor"),
    language(
        "Cargo lockfile",
        Config,
        Some('\u{f023}'),
        &[],
        &["Cargo.lock", "poetry.lock"],
        (Some("toml"), "toml", "toml"),
    )
    .syntax(HASH, QUOTES),
    language(
        "JSON lockfile",
        Config,
        Some('\u{f023}'),
        &[],
        &["package-lock.json", "composer.lock", "Pipfile.lock"],
        (Some("json"), "json", "json"),
    ),
    language(
        "YAML lockfile",
        Config,
        Some('\u{f023}'),
        &[],
        &["pnpm-lock.yaml"],
        (Some("yaml"), "yaml", "yaml"),
    )
    .syntax(HASH, QUOTES),
    language(
        "Yarn lockfile",
        Config,
        Some('\u{f023}'),
        &[],
        &["yarn.lock"],
        (Some("lockfile"), "lockfile", "lockfile"),
    )
    .syntax(HASH, DOUBLE),
    overflow("Lockfile", Config, Some('\u{f023}'), &["lock"], &[]),
    // --- shell ---
    language(
        "Shell",
        Shell,
        Some('\u{f489}'),
        &["sh", "bash"],
        &[],
        (Some("bash"), "shellscript", "shell"),
    )
    .syntax(HASH, BACKTICK),
    language_named("Zsh", Shell, Some('\u{f489}'), &["zsh"], &[], "zsh").syntax(HASH, BACKTICK),
    language_named("Fish", Shell, Some('\u{f489}'), &["fish"], &[], "fish").syntax(HASH, BACKTICK),
    // Ksh remains a labelled text format, but has no maintained, compatible
    // grammar. Do not claim a Bash-compatible parser or protocol identity.
    overflow("Ksh", Shell, Some('\u{f489}'), &["ksh"], &[]).syntax(HASH, BACKTICK),
    language_named(
        "PowerShell",
        Shell,
        None,
        &["ps1", "psm1"],
        &[],
        "powershell",
    )
    .syntax(both("#", "<#", "#>"), QUOTES),
    language_named("Batch", Shell, None, &["bat", "cmd"], &[], "batch").syntax(line("REM"), DOUBLE),
    // --- docs / prose ---
    FileType {
        name: "Markdown",
        grammar: Some("markdown"),
        lsp_language_id: Some("markdown"),
        config_selector: Some("markdown"),
        category: Markup,
        nerd: Some('\u{e73e}'),
        extensions: crate::classify::MARKDOWN_EXTENSIONS,
        filenames: &[],
        wrap_mode: WrapMode::Wrap,
        comments: MARKUP,
        pairs: MARKDOWN,
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
        comments: block("{/*", "*/}"),
        pairs: MARKDOWN,
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
    )
    .syntax(line(".."), DOUBLE),
    language_wrap(
        "AsciiDoc",
        Markup,
        None,
        &["adoc", "asciidoc"],
        &[],
        "asciidoc",
    )
    .syntax(line("//"), DOUBLE),
    language(
        "TeX",
        Document,
        None,
        &["tex", "sty", "cls"],
        &[],
        (Some("latex"), "latex", "tex"),
    )
    .syntax(line("%"), DOUBLE),
    overflow("PDF", Document, Some('\u{f1c1}'), &["pdf"], &[]),
    wrap(
        "Word",
        Document,
        Some('\u{f1c2}'),
        &["doc", "docx", "odt", "rtf"],
        &[],
    ),
    wrap(
        "Jupyter notebook",
        Document,
        Some('\u{e678}'),
        &["ipynb"],
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
        crate::classify::IMAGE_EXTENSIONS,
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
    )
    .syntax(HASH, QUOTES),
    language_named(
        "Makefile",
        Config,
        None,
        &["mk"],
        &["Makefile", "GNUmakefile", "makefile"],
        "make",
    )
    .syntax(HASH, QUOTES),
    language_named(
        "CMake",
        Config,
        None,
        &["cmake"],
        &["CMakeLists.txt"],
        "cmake",
    )
    .syntax(both("#", "#[[", "]]"), QUOTES),
    language_wrap(
        "Markdown",
        Markup,
        Some('\u{e73e}'),
        &[],
        &["README"],
        "markdown",
    )
    .syntax(MARKUP, MARKDOWN),
    overflow(
        "Git config",
        Config,
        Some('\u{f1d3}'),
        &[],
        &[".gitignore", ".gitattributes", ".gitkeep"],
    )
    .syntax(HASH, DOUBLE),
    wrap(
        "License",
        Document,
        Some('\u{f02d}'),
        &[],
        &["LICENSE", "LICENCE", "COPYING", "AUTHORS"],
    ),
    language(
        "Git modules",
        Config,
        Some('\u{f1d3}'),
        &[],
        &[".gitmodules"],
        (Some("ini"), "gitconfig", "gitconfig"),
    )
    .syntax(HASH, QUOTES),
    language(
        "EditorConfig",
        Config,
        None,
        &[],
        &[".editorconfig"],
        (Some("ini"), "editorconfig", "editorconfig"),
    )
    .syntax(HASH, QUOTES),
    language(
        "Environment",
        Config,
        None,
        &[],
        &[".env"],
        (Some("properties"), "dotenv", "dotenv"),
    )
    .syntax(HASH, QUOTES),
];
