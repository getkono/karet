# Scope — deliberate non-goals

What karet has decided **not** to build, so the decision is written down once
instead of being relitigated per issue. Each row is a standing decision: the
feature is out of scope until this document says otherwise. (Pattern borrowed
from `dependable`'s `docs/SCOPE.md`.)

For what karet *is*, see the [README](../README.md) feature tour and the
[docs index](README.md).

## TUI theming

**Out of scope.** karet ships one built-in dark theme and loads
[VS Code JSON themes](configuration.md#workbench) via `workbench.colorTheme` —
that is the whole theming surface.

Deliberately not built:

- **TextMate `.tmTheme` loading.** VS Code JSON is the one blessed interchange
  format; a second loader would double the palette-mapping surface for a format
  the target audience has largely migrated off. (Earlier docs claimed `.tmTheme`
  support; no such loader ever shipped, and the claim is now retired.)
- **A karet-native theme format.** Author themes for VS Code, load them here.
- **Chrome/UI theming beyond the palette** — configurable borders, chrome
  layouts, per-panel styling. The `TokenId`/`ThemeRole` vocabulary in
  `karet-core` (32 tokens, 30 roles) is the complete customization surface, and
  widgets resolve every color through it.
- **A light built-in theme.** Load a light VS Code theme instead; the contrast
  checker in `karet-theme` keeps it legible.

## Terminal graphics

Kitty graphics protocol plus a truecolor half-block fallback, detected at
runtime — **sixel and iTerm2 protocols are out of scope**
(`karet-fileview/src/image.rs` states this at the module level), as is
`ratatui-image` (its build script needs the system C library `chafa`, which the
[no-system-deps rule](../AGENTS.md#design-principles) forbids).

## Syntax backends

Tree-sitter only. No syntect, no TextMate grammars, no dual-backend
abstraction — see "commit to one best backend" in
[AGENTS.md](../AGENTS.md#design-principles).
