# karet-filetype

The single source of truth for **file-type classification and presentation
metadata** in the [karet](https://github.com/getkono/karet) editor toolkit.

Given a path (and optionally its leading bytes), it answers three questions that
were previously scattered across the workspace:

- **What is it?** — [`file_type_for_path`] resolves a path to a [`FileType`]
  (display name + [`Category`]), matching well-known filenames first
  (`Dockerfile`, `Makefile`, `Cargo.toml`, …) then extension.
- **How should it look?** — [`icon_for_path`], [`directory_icon`], and
  [`chevron`] return glyphs for an [`IconStyle`] (`NerdFont` / `Unicode` /
  `Ascii`), so the file tree and other widgets render consistent icons.
- **How should it open?** — [`classify`] returns a [`FileKind`]
  (`Text`/`Markdown`/`Image`/`Pdf`/`Binary`/`TooLarge`) for renderer routing,
  using extension plus magic-byte sniffing.

The crate is headless and dependency-free (only `std`); presentation crates
(`karet-widgets`, the `karet` app) and the parse host (`karet-treesitter`)
consume it.

The catalogue of recognized formats lives in
[`docs/file-formats.md`](../../docs/file-formats.md).
