//! Typed identities of the bundled grammars.
//!
//! One constant per grammar, `Some` exactly when the `lang-*` feature that
//! compiles the grammar is on *in this crate*. The value is decided here rather
//! than by a `cfg` in the consumer because a grammar's feature is often switched
//! on by another crate in the build (the `karet` app turns on
//! `karet-treesitter/all-languages` while depending on `karet-syntax` with none of
//! its pass-through features), and a consumer's own `cfg` cannot see that.
//!
//! [`LanguageId`] has no public constructor, so these constants and the resolvers
//! are the only way to hold one, and every value names a grammar compiled into
//! this build. Engines compare against these instead of resolving a string per
//! call; [`language_id_from_injection_name`](crate::language_id_from_injection_name)
//! stays for names that arrive *inside* source text (an injection query, a
//! markdown fence's info string), and
//! [`language_id_from_path`](crate::language_id_from_path) for file paths.
//!
//! The numbers behind these constants are stable and are **never** renumbered once
//! shipped.

use crate::LanguageId;

/// Declares every grammar once: the public `Option` constant, and the
/// crate-private [`ids`] constant the registry tables are keyed by.
macro_rules! grammars {
    ($($(#[doc = $doc:literal])* $name:ident = $number:literal, $feature:literal;)*) => {
        $(
            $(#[doc = $doc])*
            #[doc = ""]
            #[doc = concat!("`Some` when the `", $feature, "` feature compiles the grammar in.")]
            pub const $name: Option<LanguageId> = if cfg!(feature = $feature) {
                Some(LanguageId($number))
            } else {
                None
            };
        )*

        /// The registry's own keys: each grammar's id, present only when its
        /// grammar is compiled in, so a table entry cannot name a missing grammar.
        pub(crate) mod ids {
            #[allow(
                unused_imports,
                reason = "a build with no `lang-*` feature compiles no id to type"
            )]
            use crate::LanguageId;
            $(
                #[cfg(feature = $feature)]
                pub(crate) const $name: LanguageId = LanguageId($number);
            )*
        }
    };
}

grammars! {
    /// Rust.
    RUST = 1, "lang-rust";
    /// Python.
    PYTHON = 2, "lang-python";
    /// JavaScript (including JSX).
    JAVASCRIPT = 3, "lang-javascript";
    /// TypeScript.
    TYPESCRIPT = 4, "lang-typescript";
    /// TSX, TypeScript's JSX dialect.
    TSX = 5, "lang-typescript";
    /// JSON (and JSONC).
    JSON = 6, "lang-json";
    /// Go.
    GO = 7, "lang-go";
    /// C.
    C = 8, "lang-c";
    /// C++.
    CPP = 9, "lang-cpp";
    /// C#.
    CSHARP = 10, "lang-csharp";
    /// Java.
    JAVA = 11, "lang-java";
    /// Ruby.
    RUBY = 12, "lang-ruby";
    /// PHP.
    PHP = 13, "lang-php";
    /// Bash.
    BASH = 14, "lang-bash";
    /// TOML.
    TOML = 15, "lang-toml";
    /// HTML.
    HTML = 16, "lang-html";
    /// CSS.
    CSS = 17, "lang-css";
    /// YAML.
    YAML = 18, "lang-yaml";
    /// Markdown's block grammar, the entry point for a markdown file.
    MARKDOWN = 19, "lang-markdown";
    /// tree-sitter-md's companion *inline* grammar. Never resolved from a path —
    /// the block grammar reaches it only through a `markdown_inline` injection.
    MARKDOWN_INLINE = 20, "lang-markdown";
    /// TeX and LaTeX.
    LATEX = 21, "lang-latex";
    /// Zig.
    ZIG = 22, "lang-zig";
    /// XML (including SVG).
    XML = 23, "lang-xml";
    /// Svelte.
    SVELTE = 24, "lang-svelte";
    /// Astro.
    ASTRO = 25, "lang-astro";
    /// Vue single-file components.
    VUE = 26, "lang-vue";
    /// SQL.
    SQL = 27, "lang-sql";
    /// GraphQL.
    GRAPHQL = 28, "lang-graphql";
    /// Protocol Buffers.
    PROTOBUF = 29, "lang-protobuf";
    /// Dockerfile / Containerfile.
    CONTAINERFILE = 30, "lang-containerfile";
    /// Makefile.
    MAKE = 31, "lang-make";
    /// CMake.
    CMAKE = 32, "lang-cmake";
    /// reStructuredText.
    RST = 33, "lang-rst";
    /// AsciiDoc.
    ASCIIDOC = 34, "lang-asciidoc";
    /// Zsh.
    ZSH = 35, "lang-zsh";
    /// Fish.
    FISH = 36, "lang-fish";
    /// PowerShell.
    POWERSHELL = 37, "lang-powershell";
    /// Windows batch files.
    BATCH = 38, "lang-batch";
    /// Kotlin.
    KOTLIN = 39, "lang-kotlin";
    /// Swift.
    SWIFT = 40, "lang-swift";
    /// Scala.
    SCALA = 41, "lang-scala";
    /// Lua.
    LUA = 42, "lang-lua";
    /// Haskell.
    HASKELL = 43, "lang-haskell";
    /// OCaml implementation files (`.ml`).
    OCAML = 44, "lang-ocaml";
    /// OCaml interface files (`.mli`).
    OCAML_INTERFACE = 45, "lang-ocaml";
    /// Elixir.
    ELIXIR = 46, "lang-elixir";
    /// Erlang.
    ERLANG = 47, "lang-erlang";
    /// Dart.
    DART = 48, "lang-dart";
    /// R.
    R = 49, "lang-r";
    /// Perl.
    PERL = 50, "lang-perl";
    /// Clojure.
    CLOJURE = 51, "lang-clojure";
    /// Emacs Lisp.
    ELISP = 52, "lang-elisp";
    /// Vim script.
    VIM = 53, "lang-vim";
    /// SCSS.
    SCSS = 54, "lang-scss";
    /// Sass (indented syntax).
    SASS = 55, "lang-sass";
    /// Less.
    LESS = 56, "lang-less";
    /// ERB templates.
    ERB = 57, "lang-erb";
    /// MDX.
    MDX = 58, "lang-mdx";
    /// JSON5.
    JSON5 = 59, "lang-json5";
    /// INI.
    INI = 60, "lang-ini";
    /// Java properties and dotenv files.
    PROPERTIES = 61, "lang-properties";
    /// EDN.
    EDN = 62, "lang-edn";
    /// CBOR diagnostic notation.
    CBOR = 63, "lang-cbor";
    /// Yarn lockfiles.
    LOCKFILE = 64, "lang-lockfile";
}
