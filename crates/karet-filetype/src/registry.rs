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

/// Static presentation metadata for one recognized file type.
///
/// Resolve one from a path with [`file_type_for_path`]. Icons are resolved per
/// [`IconStyle`] via [`FileType::icon`]: Nerd Font uses a per-type glyph (falling
/// back to the [`Category`]'s glyph), while the Unicode/ASCII tiers use the
/// category's fallback glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileType {
    name: &'static str,
    category: Category,
    nerd: Option<char>,
    extensions: &'static [&'static str],
    filenames: &'static [&'static str],
}

impl FileType {
    /// The human-readable display name (e.g. `"Rust"`, `"Markdown"`).
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.name
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
}

/// The fallback for an unrecognized file.
const UNKNOWN: FileType = FileType {
    name: "File",
    category: Category::Unknown,
    nerd: None,
    extensions: &[],
    filenames: &[],
};

/// Compact constructor for a registry entry.
const fn ft(
    name: &'static str,
    category: Category,
    nerd: Option<char>,
    extensions: &'static [&'static str],
    filenames: &'static [&'static str],
) -> FileType {
    FileType {
        name,
        category,
        nerd,
        extensions,
        filenames,
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
    ft("Rust", Code, Some('\u{e7a8}'), &["rs"], &[]),
    ft("Python", Code, Some('\u{e606}'), &["py", "pyi", "pyw"], &[]),
    ft("C", Code, Some('\u{e61e}'), &["c", "h"], &[]),
    ft(
        "C++",
        Code,
        Some('\u{e61d}'),
        &["cc", "cpp", "cxx", "hpp", "hh", "hxx"],
        &[],
    ),
    ft("C#", Code, None, &["cs"], &[]),
    ft("Java", Code, Some('\u{e738}'), &["java"], &[]),
    ft("Kotlin", Code, None, &["kt", "kts"], &[]),
    ft("Go", Code, Some('\u{e627}'), &["go"], &[]),
    ft("Ruby", Code, Some('\u{e739}'), &["rb", "erb"], &[]),
    ft("PHP", Code, Some('\u{e73d}'), &["php"], &[]),
    ft("Swift", Code, None, &["swift"], &[]),
    ft("Scala", Code, None, &["scala", "sbt", "sc"], &[]),
    ft("Lua", Code, Some('\u{e620}'), &["lua"], &[]),
    ft("Haskell", Code, None, &["hs", "lhs"], &[]),
    ft("OCaml", Code, None, &["ml", "mli"], &[]),
    ft("Elixir", Code, None, &["ex", "exs"], &[]),
    ft("Erlang", Code, None, &["erl", "hrl"], &[]),
    ft("Dart", Code, None, &["dart"], &[]),
    ft("R", Code, None, &["r"], &[]),
    ft("Zig", Code, None, &["zig"], &[]),
    ft("Perl", Code, None, &["pl", "pm"], &[]),
    ft("Clojure", Code, None, &["clj", "cljs", "cljc", "edn"], &[]),
    ft("Emacs Lisp", Code, None, &["el"], &[]),
    ft("Vim script", Code, None, &["vim"], &[]),
    // --- web ---
    ft(
        "JavaScript",
        Code,
        Some('\u{e74e}'),
        &["js", "mjs", "cjs"],
        &[],
    ),
    ft("JSX", Code, Some('\u{e7ba}'), &["jsx"], &[]),
    ft(
        "TypeScript",
        Code,
        Some('\u{e628}'),
        &["ts", "mts", "cts"],
        &[],
    ),
    ft("TSX", Code, Some('\u{e7ba}'), &["tsx"], &[]),
    ft(
        "HTML",
        Markup,
        Some('\u{e736}'),
        &["html", "htm", "xhtml"],
        &[],
    ),
    ft("CSS", Markup, Some('\u{e749}'), &["css"], &[]),
    ft("Sass", Markup, Some('\u{e74b}'), &["scss", "sass"], &[]),
    ft("Less", Markup, None, &["less"], &[]),
    ft("Vue", Markup, None, &["vue"], &[]),
    ft("Svelte", Markup, None, &["svelte"], &[]),
    // --- data / config ---
    ft(
        "JSON",
        Data,
        Some('\u{e60b}'),
        &["json", "jsonc", "json5"],
        &[],
    ),
    ft("YAML", Config, None, &["yml", "yaml"], &[]),
    ft("TOML", Config, None, &["toml"], &[]),
    ft("INI", Config, None, &["ini", "cfg", "conf"], &[]),
    ft("Properties", Config, None, &["properties"], &[]),
    ft("Pkl", Config, None, &["pkl"], &[]),
    ft("XML", Markup, None, &["xml"], &[]),
    ft("SVG", Markup, None, &["svg"], &[]),
    ft("CSV", Data, None, &["csv", "tsv"], &[]),
    ft("SQL", Data, Some('\u{f1c0}'), &["sql"], &[]),
    ft("GraphQL", Data, None, &["graphql", "gql"], &[]),
    ft("Protobuf", Data, None, &["proto"], &[]),
    ft("CBOR", Data, None, &["cbor"], &[]),
    ft("Lockfile", Config, Some('\u{f023}'), &["lock"], &[]),
    // --- shell ---
    ft(
        "Shell",
        Shell,
        Some('\u{f489}'),
        &["sh", "bash", "zsh", "fish", "ksh"],
        &[],
    ),
    ft("PowerShell", Shell, None, &["ps1", "psm1"], &[]),
    ft("Batch", Shell, None, &["bat", "cmd"], &[]),
    // --- docs / prose ---
    ft(
        "Markdown",
        Markup,
        Some('\u{e73e}'),
        &["md", "markdown", "mdown", "mkd", "mdx"],
        &[],
    ),
    ft(
        "Plain Text",
        Document,
        Some('\u{f15c}'),
        &["txt", "text"],
        &[],
    ),
    ft("reStructuredText", Markup, None, &["rst"], &[]),
    ft("AsciiDoc", Markup, None, &["adoc", "asciidoc"], &[]),
    ft("TeX", Document, None, &["tex"], &[]),
    ft("PDF", Document, Some('\u{f1c1}'), &["pdf"], &[]),
    ft(
        "Word",
        Document,
        Some('\u{f1c2}'),
        &["doc", "docx", "odt", "rtf"],
        &[],
    ),
    ft(
        "Spreadsheet",
        Data,
        Some('\u{f1c3}'),
        &["xls", "xlsx", "ods"],
        &[],
    ),
    ft(
        "Presentation",
        Document,
        Some('\u{f1c4}'),
        &["ppt", "pptx", "odp"],
        &[],
    ),
    // --- images ---
    ft(
        "Image",
        Image,
        Some('\u{f1c5}'),
        &[
            "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tiff", "tif",
        ],
        &[],
    ),
    // --- archives ---
    ft(
        "Archive",
        Archive,
        Some('\u{f1c6}'),
        &[
            "zip", "tar", "gz", "tgz", "bz2", "xz", "7z", "rar", "zst", "jar", "war",
        ],
        &[],
    ),
    // --- media / binary ---
    ft(
        "Audio",
        Binary,
        Some('\u{f1c7}'),
        &["mp3", "wav", "flac", "ogg", "m4a", "aac"],
        &[],
    ),
    ft(
        "Video",
        Binary,
        Some('\u{f1c8}'),
        &["mp4", "mkv", "mov", "avi", "webm", "wmv"],
        &[],
    ),
    ft(
        "Font",
        Binary,
        Some('\u{f031}'),
        &["ttf", "otf", "woff", "woff2", "eot"],
        &[],
    ),
    ft(
        "Database",
        Data,
        Some('\u{f1c0}'),
        &["db", "sqlite", "sqlite3"],
        &[],
    ),
    ft(
        "Binary",
        Binary,
        None,
        &[
            "exe", "dll", "so", "dylib", "o", "a", "bin", "wasm", "class",
        ],
        &[],
    ),
    // --- special filenames (matched before extensions) ---
    ft(
        "Dockerfile",
        Config,
        Some('\u{e7b0}'),
        &[],
        &["Dockerfile", "Containerfile"],
    ),
    ft(
        "Makefile",
        Config,
        None,
        &["mk"],
        &["Makefile", "GNUmakefile", "makefile", "CMakeLists.txt"],
    ),
    ft(
        "Git config",
        Config,
        Some('\u{f1d3}'),
        &[],
        &[".gitignore", ".gitattributes", ".gitmodules", ".gitkeep"],
    ),
    ft(
        "License",
        Document,
        Some('\u{f02d}'),
        &[],
        &["LICENSE", "LICENCE", "COPYING", "README", "AUTHORS"],
    ),
    ft("EditorConfig", Config, None, &[], &[".editorconfig"]),
    ft("Environment", Config, None, &[], &[".env"]),
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
            "Makefile"
        );
    }

    #[test]
    fn unknown_extension_falls_back() {
        let ft = file_type_for_path(Path::new("mystery.zzz"));
        assert_eq!(ft.name(), "File");
        assert_eq!(ft.category(), Category::Unknown);
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
