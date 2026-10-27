//! Typed identities of the bundled grammars.
//!
//! One constant per grammar, each gated on the `lang-*` feature that compiles the
//! grammar in, so naming a grammar this build lacks is a compile error rather than
//! a runtime miss. These are the only [`LanguageId`]s that exist: the type has no
//! public constructor, so a value always names a registered grammar.
//!
//! Engines compare against these instead of resolving a string per call. The
//! string resolver [`language_id_from_injection_name`](crate::language_id_from_injection_name)
//! stays for names that arrive *inside* source text (an injection query, a
//! markdown fence's info string), and
//! [`language_id_from_path`](crate::language_id_from_path) for file paths.
//!
//! The numbers behind these constants are stable and are **never** renumbered once
//! shipped.

#[allow(
    unused_imports,
    reason = "a build with no `lang-*` feature compiles no constant to type"
)]
use crate::LanguageId;

/// Rust.
#[cfg(feature = "lang-rust")]
pub const RUST: LanguageId = LanguageId(1);
/// Python.
#[cfg(feature = "lang-python")]
pub const PYTHON: LanguageId = LanguageId(2);
/// JavaScript (including JSX).
#[cfg(feature = "lang-javascript")]
pub const JAVASCRIPT: LanguageId = LanguageId(3);
/// TypeScript.
#[cfg(feature = "lang-typescript")]
pub const TYPESCRIPT: LanguageId = LanguageId(4);
/// TSX, TypeScript's JSX dialect (compiled by `lang-typescript`).
#[cfg(feature = "lang-typescript")]
pub const TSX: LanguageId = LanguageId(5);
/// JSON (and JSONC).
#[cfg(feature = "lang-json")]
pub const JSON: LanguageId = LanguageId(6);
/// Go.
#[cfg(feature = "lang-go")]
pub const GO: LanguageId = LanguageId(7);
/// C.
#[cfg(feature = "lang-c")]
pub const C: LanguageId = LanguageId(8);
/// C++.
#[cfg(feature = "lang-cpp")]
pub const CPP: LanguageId = LanguageId(9);
/// C#.
#[cfg(feature = "lang-csharp")]
pub const CSHARP: LanguageId = LanguageId(10);
/// Java.
#[cfg(feature = "lang-java")]
pub const JAVA: LanguageId = LanguageId(11);
/// Ruby.
#[cfg(feature = "lang-ruby")]
pub const RUBY: LanguageId = LanguageId(12);
/// PHP.
#[cfg(feature = "lang-php")]
pub const PHP: LanguageId = LanguageId(13);
/// Bash.
#[cfg(feature = "lang-bash")]
pub const BASH: LanguageId = LanguageId(14);
/// TOML.
#[cfg(feature = "lang-toml")]
pub const TOML: LanguageId = LanguageId(15);
/// HTML.
#[cfg(feature = "lang-html")]
pub const HTML: LanguageId = LanguageId(16);
/// CSS.
#[cfg(feature = "lang-css")]
pub const CSS: LanguageId = LanguageId(17);
/// YAML.
#[cfg(feature = "lang-yaml")]
pub const YAML: LanguageId = LanguageId(18);
/// Markdown's block grammar, the entry point for a markdown file.
#[cfg(feature = "lang-markdown")]
pub const MARKDOWN: LanguageId = LanguageId(19);
/// tree-sitter-md's companion *inline* grammar. Never resolved from a path — the
/// block grammar reaches it only through a `markdown_inline` injection.
#[cfg(feature = "lang-markdown")]
pub const MARKDOWN_INLINE: LanguageId = LanguageId(20);
/// TeX and LaTeX.
#[cfg(feature = "lang-latex")]
pub const LATEX: LanguageId = LanguageId(21);
/// Zig.
#[cfg(feature = "lang-zig")]
pub const ZIG: LanguageId = LanguageId(22);
/// XML (including SVG).
#[cfg(feature = "lang-xml")]
pub const XML: LanguageId = LanguageId(23);
/// Svelte.
#[cfg(feature = "lang-svelte")]
pub const SVELTE: LanguageId = LanguageId(24);
/// Astro.
#[cfg(feature = "lang-astro")]
pub const ASTRO: LanguageId = LanguageId(25);
/// Vue single-file components.
#[cfg(feature = "lang-vue")]
pub const VUE: LanguageId = LanguageId(26);
/// SQL.
#[cfg(feature = "lang-sql")]
pub const SQL: LanguageId = LanguageId(27);
/// GraphQL.
#[cfg(feature = "lang-graphql")]
pub const GRAPHQL: LanguageId = LanguageId(28);
/// Protocol Buffers.
#[cfg(feature = "lang-protobuf")]
pub const PROTOBUF: LanguageId = LanguageId(29);
/// Dockerfile / Containerfile.
#[cfg(feature = "lang-containerfile")]
pub const CONTAINERFILE: LanguageId = LanguageId(30);
/// Makefile.
#[cfg(feature = "lang-make")]
pub const MAKE: LanguageId = LanguageId(31);
/// CMake.
#[cfg(feature = "lang-cmake")]
pub const CMAKE: LanguageId = LanguageId(32);
/// reStructuredText.
#[cfg(feature = "lang-rst")]
pub const RST: LanguageId = LanguageId(33);
/// AsciiDoc.
#[cfg(feature = "lang-asciidoc")]
pub const ASCIIDOC: LanguageId = LanguageId(34);
/// Zsh.
#[cfg(feature = "lang-zsh")]
pub const ZSH: LanguageId = LanguageId(35);
/// Fish.
#[cfg(feature = "lang-fish")]
pub const FISH: LanguageId = LanguageId(36);
/// PowerShell.
#[cfg(feature = "lang-powershell")]
pub const POWERSHELL: LanguageId = LanguageId(37);
/// Windows batch files.
#[cfg(feature = "lang-batch")]
pub const BATCH: LanguageId = LanguageId(38);
/// Kotlin.
#[cfg(feature = "lang-kotlin")]
pub const KOTLIN: LanguageId = LanguageId(39);
/// Swift.
#[cfg(feature = "lang-swift")]
pub const SWIFT: LanguageId = LanguageId(40);
/// Scala.
#[cfg(feature = "lang-scala")]
pub const SCALA: LanguageId = LanguageId(41);
/// Lua.
#[cfg(feature = "lang-lua")]
pub const LUA: LanguageId = LanguageId(42);
/// Haskell.
#[cfg(feature = "lang-haskell")]
pub const HASKELL: LanguageId = LanguageId(43);
/// OCaml implementation files (`.ml`).
#[cfg(feature = "lang-ocaml")]
pub const OCAML: LanguageId = LanguageId(44);
/// OCaml interface files (`.mli`, compiled by `lang-ocaml`).
#[cfg(feature = "lang-ocaml")]
pub const OCAML_INTERFACE: LanguageId = LanguageId(45);
/// Elixir.
#[cfg(feature = "lang-elixir")]
pub const ELIXIR: LanguageId = LanguageId(46);
/// Erlang.
#[cfg(feature = "lang-erlang")]
pub const ERLANG: LanguageId = LanguageId(47);
/// Dart.
#[cfg(feature = "lang-dart")]
pub const DART: LanguageId = LanguageId(48);
/// R.
#[cfg(feature = "lang-r")]
pub const R: LanguageId = LanguageId(49);
/// Perl.
#[cfg(feature = "lang-perl")]
pub const PERL: LanguageId = LanguageId(50);
/// Clojure.
#[cfg(feature = "lang-clojure")]
pub const CLOJURE: LanguageId = LanguageId(51);
/// Emacs Lisp.
#[cfg(feature = "lang-elisp")]
pub const ELISP: LanguageId = LanguageId(52);
/// Vim script.
#[cfg(feature = "lang-vim")]
pub const VIM: LanguageId = LanguageId(53);
/// SCSS.
#[cfg(feature = "lang-scss")]
pub const SCSS: LanguageId = LanguageId(54);
/// Sass (indented syntax).
#[cfg(feature = "lang-sass")]
pub const SASS: LanguageId = LanguageId(55);
/// Less.
#[cfg(feature = "lang-less")]
pub const LESS: LanguageId = LanguageId(56);
/// ERB templates.
#[cfg(feature = "lang-erb")]
pub const ERB: LanguageId = LanguageId(57);
/// MDX.
#[cfg(feature = "lang-mdx")]
pub const MDX: LanguageId = LanguageId(58);
/// JSON5.
#[cfg(feature = "lang-json5")]
pub const JSON5: LanguageId = LanguageId(59);
/// INI.
#[cfg(feature = "lang-ini")]
pub const INI: LanguageId = LanguageId(60);
/// Java properties and dotenv files.
#[cfg(feature = "lang-properties")]
pub const PROPERTIES: LanguageId = LanguageId(61);
/// EDN.
#[cfg(feature = "lang-edn")]
pub const EDN: LanguageId = LanguageId(62);
/// CBOR diagnostic notation.
#[cfg(feature = "lang-cbor")]
pub const CBOR: LanguageId = LanguageId(63);
/// Yarn lockfiles.
#[cfg(feature = "lang-lockfile")]
pub const LOCKFILE: LanguageId = LanguageId(64);
