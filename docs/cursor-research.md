# Cursor Rendering Research

Last updated: 2026-07-08.

The current editor caret is cell-based. A true GUI-style caret between terminal
grid cells would require an application-controlled protocol for sub-cell cursor
placement that composites over already-rendered text.

## Finding

No implementation change is recommended now.

Current public terminal mechanisms cover:

- cell-aligned cursor shape and movement, such as DECSCUSR-style block/bar/
  underline cursors;
- raster/image protocols, such as Kitty graphics or iTerm2 inline images;
- terminal capability probing patterns already used in karet for Kitty graphics
  and OSC 22 pointer-shape support.

Those mechanisms do not provide a portable application-controlled sub-cell text
caret overlay. Image protocols can place pixels, but they are not a text-aware
cursor compositing primitive and would degrade poorly through tmux, screen, and
SSH.

## Decision

Keep the current cell-based caret rendering. If a terminal family publishes a
real sub-cell cursor compositing protocol, add it behind the same
probe-then-confirm capability model used for graphics and pointer-shape support,
with graceful fallback to the current renderer.

References:

- <https://sw.kovidgoyal.net/kitty/graphics-protocol/>
- <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>
- <https://iterm2.com/documentation-images.html>
- <https://wezterm.org/escape-sequences.html>
