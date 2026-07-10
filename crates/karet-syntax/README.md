# karet-syntax

Tree-sitter-powered syntactic analysis for [karet](https://github.com/getkono/karet)
editors. It produces **data, not rendering**: highlight spans tagged with semantic
`TokenId`s that a consumer colors with a theme (`karet-theme`) and paints itself.

This is the crate behind the standalone "highlight a code snippet" use case.
Highlighting runs a grammar's query through `karet-treesitter`'s single parse host;
fold regions, bracket pairs, and structural selection round out the API.

`Highlighter` highlights one language. `LayeredHighlighter` highlights a `LayeredTree` —
a document together with the languages it embeds — merging every layer's captures into one
span list, so a markdown code fence is coloured as the language it names and a Rust doc
comment as the markdown it is. `Highlights::translate` rides an edit, keeping spans aligned
while an asynchronous re-highlight is in flight.

Part of the karet workspace; released in lockstep with the other `karet-*` crates.
