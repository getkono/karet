# karet-theme

Color tokens, theme loading, and contrast checking for
[karet](https://github.com/getkono/karet). It maps the semantic `TokenId` /
`ThemeRole` vocabulary (from `karet-core`) to concrete `Rgba` colors, independent of
any renderer.

Ships a built-in dark theme (`Theme::dark`, also `Theme::default`). Features:
`tmtheme` loads TextMate `.tmTheme` files, `vscode` loads VS Code JSON themes, and
`view` converts colors into ratatui values.

Part of the karet workspace; released in lockstep with the other `karet-*` crates.
