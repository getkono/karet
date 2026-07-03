# Configuration

karet reads a **JSONC** configuration file (`setting.jsonc`) — JSON with `//` and
`/* */` comments and trailing commas. Every setting has a sane default, so the file is
entirely optional; you only override what you want to change.

## File locations & precedence

karet merges up to three files. **More specific layers win**, and every layer is merged
over the built-in defaults. Within a layer, nested objects merge key-by-key while arrays
replace wholesale.

| Precedence | Layer | Path |
|---|---|---|
| 1 (highest) | Project | `$GIT_ROOT/.karet/setting.jsonc` |
| 2 | User | `$XDG_CONFIG_HOME/karet/setting.jsonc` (`~/.config/karet/…` on Linux) |
| 3 (lowest) | System | `<system config dir>/karet/setting.jsonc` (`/etc/xdg/karet/…` on Unix) |

So a repository can pin project-wide conventions in `.karet/setting.jsonc`, a user can set
personal preferences under XDG, and an administrator can set machine-wide defaults.

## Schema & validation

Loading **never fails**. A missing file is skipped; a malformed file — a JSONC syntax
error, an unknown key, or a wrong-typed value — degrades the affected *section* to its
defaults and raises a startup notification pointing at the problem, leaving the rest of
your settings in effect.

An external JSON Schema is published at
[`settings.schema.json`](../settings.schema.json) and generated from the same Rust types
that verify the file, so it can never drift. Reference it for editor autocomplete:

```jsonc
{
  "$schema": "https://raw.githubusercontent.com/getkono/karet/master/settings.schema.json",
  "editor": { "tabSize": 2 }
}
```

## Settings

Keys use the VS Code / Zed camelCase style. Defaults shown.

### `editor`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `tabSize` | number | `4` | Columns per indent level. |
| `insertSpaces` | bool | `true` | Insert spaces instead of a hard tab. |
| `lineNumbers` | `"on"`\|`"off"`\|`"relative"` | `"on"` | Line-number gutter mode. |
| `cursorLine` | bool | `true` | Highlight the caret's line. |
| `scrollOff` | number | `3` | Lines kept visible above/below the caret. |
| `rulers` | number[] | `[]` | Columns to draw vertical rulers at. |
| `wordWrap` | bool | `false` | Soft-wrap long lines. |
| `trimTrailingWhitespace` | bool | `true` | Strip trailing whitespace on save. |
| `insertFinalNewline` | bool | `true` | Ensure a trailing newline on save. |
| `formatOnSave` | bool | `false` | Run the formatter on save. |

### `files`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `autoSave` | `"off"`\|`"afterDelay"`\|`"onFocusChange"` | `"off"` | When dirty buffers auto-save. |
| `autoSaveDelay` | number | `1000` | Delay (ms) for `afterDelay`. |
| `encoding` | string | `"utf-8"` | Default encoding label. |
| `eol` | `"auto"`\|`"lf"`\|`"crlf"` | `"auto"` | Line-ending style on save. |
| `exclude` | string[] | `[]` | Globs hidden from the file explorer. |
| `watcherExclude` | string[] | `[]` | Globs excluded from the filesystem watcher. |

### `workbench`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `colorTheme` | string | `"dark"` | Built-in `"dark"`, or a path to a `.tmTheme` / VS Code `.json` theme. |
| `iconStyle` | `"nerdFont"`\|`"unicode"`\|`"ascii"` | `"nerdFont"` | File-tree / activity-bar glyphs. |
| `startupPanel` | `"explorer"`\|`"search"`\|`"sourceControl"`\|`"none"` | `"explorer"` | Sidebar panel shown at startup. |

### `search`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `exclude` | string[] | `[]` | Globs excluded from workspace search. |
| `useIgnoreFiles` | bool | `true` | Honour `.gitignore` / `.ignore`. |
| `smartCase` | bool | `true` | Case-insensitive unless the query has an uppercase letter. |

### `spellcheck`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | bool | `false` | Spell-check comments and strings. |
| `language` | string | `"en_US"` | Dictionary language. |
| `words` | string[] | `[]` | Extra correctly-spelled words. |

### `git`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `decorations` | bool | `true` | Gutter change decorations + file-tree status colouring. |
| `blame` | bool | `false` | Inline blame for the current line. |

> An explicit `--icons` flag (or the `KARET_ICONS` environment variable) overrides
> `workbench.iconStyle`.

## Regenerating the schema

The JSON Schema is emitted from the `Settings` type; regenerate it after changing the
schema with:

```sh
cargo run -p karet-session --example settings-schema > settings.schema.json
```

A test (`karet-session`'s `config::tests::checked_in_schema_is_current`) fails if the
committed file drifts.
