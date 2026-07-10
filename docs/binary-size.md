# Binary size & the lean build

The `karet` app gates its two heavy media renderers behind default-on Cargo
features so a build can drop their dependency trees (issue #23):

- **`images`** pulls the [`image`](https://crates.io/crates/image) raster codec
  stack (PNG/JPEG/GIF/WebP/AVIF/EXR/TIFF/QOI…).
- **`pdf`** pulls [`karet-pdf`] → [`hayro`], a pure-Rust PDF interpreter/renderer
  (`hayro_interpret`, `hayro_syntax`, `vello_cpu`, and the `skrifa`/`read-fonts`
  font stack).

Both are **on by default**, so the shipped binary is unchanged. A
`--no-default-features` build compiles both out; a disabled file type falls
through to the existing placeholder ("Image preview unavailable" / "PDF
document"). See `mise run build-lean` and the CI "Lean build" step.

## How to reproduce

```bash
# Full (default) vs lean release binaries
cargo build --release -p karet
cargo build --release -p karet --no-default-features

# Dependency closures
cargo tree -p karet -e normal
cargo tree -p karet --no-default-features -e normal
cargo tree -p karet -e normal -i hayro   # present by default, absent when lean
cargo tree -p karet -e normal -i image

# Code-size breakdown
cargo bloat --release -p karet --crates
cargo bloat --release -p karet --no-default-features --crates
```

## Measurements

Measured on macOS (aarch64-apple-darwin), release profile, `karet` at 0.2.1.

### Compiled binary size

| binary            | default (all on) | lean (`--no-default-features`) | delta            |
| ----------------- | ---------------: | -----------------------------: | ---------------- |
| stripped          |         41.19 MB |                       34.44 MB | −6.75 MB (−16 %) |
| unstripped        |         45.79 MB |                       37.55 MB | −8.24 MB (−18 %) |
| `.text` (from bloat) |        13.7 MiB |                        9.6 MiB | −4.1 MiB (−30 %) |

The top `.text` contributors that disappear in the lean build are exactly the
media crates: `image` (~506 KiB), `hayro_interpret` (~514 KiB), `hayro_syntax`
(~357 KiB), `skrifa` (~318 KiB), plus `karet-fileview`'s raster code.

### Dependency closure

| build   | normal deps (`cargo tree -e normal`, unique) |
| ------- | -------------------------------------------: |
| default |                                          506 |
| lean    |                                          419 |

**87 crates** drop out — the whole `hayro`/`vello_cpu`/font tree (`hayro-*`,
`vello_common`, `vello_cpu`, `skrifa`, `read-fonts`, `font-types`, `kurbo`,
`peniko`, `moxcms`, `pxfm`…) and the `image` codec closure (`png`, `gif`,
`tiff`, `exr`, `qoi`, `image-webp`, `zune-jpeg`, `ravif`/`rav1e`/`av1-grain`,
`avif-serialize`, `brotli`, `flate2`, `weezl`…). `karet-pdf` itself is gone.

### Runtime load

Startup RSS ("maximum resident set size", via `/usr/bin/time -l karet
--version`) is a proxy for the load footprint before a document is opened:

| build   | max RSS |
| ------- | ------: |
| default | 8.55 MB |
| lean    | 8.08 MB |

The startup delta is modest (~475 KB) because `--version` never rasterizes
anything; the win is the ~6–8 MB smaller on-disk/mmapped image and the codec
data those crates would otherwise map in on first use.

## Conclusion

Gating the two media renderers removes 87 transitive crates and ~16 % of the
stripped binary at zero cost to the default experience — a clear win for a
regularly-launched TUI. The numbers do **not** yet justify the daemon split the
issue floats as a fallback: the remaining footprint is dominated by unavoidable
core deps (`std`, `gix`, `rustls`/`reqwest`, `regex`), not by anything a
process boundary would shed. Reassess if additional heavy, optional subsystems
(e.g. a real DOCX/HTML renderer) land — they should follow this same
feature-gated pattern.

[`karet-pdf`]: ../crates/karet-pdf
[`hayro`]: https://crates.io/crates/hayro
