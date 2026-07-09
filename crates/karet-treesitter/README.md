# karet-treesitter

> Shared tree-sitter parse host (incremental parsing, tree cache, query running) for karet.

A single, reusable tree-sitter front end — parser pooling, incremental re-parsing, and query
execution — so syntax highlighting, diffing and structural navigation all share one parse of
each buffer. Tree-sitter is the sole syntax backend, by design.

It also resolves **language injection**: `LayeredParser` parses a document as a root tree
plus one tree per embedded region, so a markdown code fence is parsed as the language it
names, an HTML `<script>` as JavaScript, and a Rust doc comment as markdown (whose own
` ```rust ` fences then nest back into Rust). Injected layers are parsed over the shared
source with `set_included_ranges`, so every node already carries document byte offsets and a
consumer can merge their queries with no coordinate translation.

Part of the [karet](https://github.com/getkono/karet) workspace.

## License

Licensed under either of MIT or Apache-2.0 at your option.
