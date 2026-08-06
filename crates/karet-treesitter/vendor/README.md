# Vendored grammars

These generated parsers are kept inside `karet-treesitter` because no compatible
crates.io package exists. This preserves the crate's independently publishable
dependency closure while keeping each grammar behind an off-by-default feature.

| directory | upstream | revision | license |
|---|---|---|---|
| `tree-sitter-sass` | <https://github.com/bajrangCoder/tree-sitter-sass> | `fb280c41b070657e4ff4d4e5e6eea6cb19efd9b8` | MIT |
| `tree-sitter-mdx` | <https://github.com/srazzak/tree-sitter-mdx> | `3aa29e8de1bf0213948a04fe953039b6ab73777b` | MIT |

To update one, copy its generated `src/parser.c`, `src/scanner.c`,
`src/node-types.json`, `src/tree_sitter/*.h`, selected queries, and license at a
reviewed commit. Then run the all-feature query tests, package audit, and workspace
verification before changing the pinned revision above.
