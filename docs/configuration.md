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

Loading **never fails**. At startup, a missing file is skipped; a malformed file — a
JSONC syntax error, an unknown key, or a wrong-typed value — degrades the affected
*section* to its defaults and raises a notification pointing at the problem, leaving the
rest of your settings in effect. During live reload, karet instead keeps that layer's
last valid value until the edit becomes valid again.

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
| `graphicalCursor` | bool\|null | `null` | Draw a graphical caret when supported; `true` reports a visible error if incompatible. |
| `scrollOff` | number | `3` | Lines kept visible above/below the caret. |
| `rulers` | number[] | `[]` | Columns to draw vertical rulers at. |
| `wordWrap` | bool\|null | `null` | Override long-line handling; `null` uses the file-type default, `true` wraps, and `false` scrolls horizontally. |
| `stickyScroll` | bool | `true` | Pin the active semantic block hierarchy above scrolled text. Multi-line signatures collapse to one row with an ellipsis. |
| `trimTrailingWhitespace` | bool | `true` | Strip trailing whitespace on save. |
| `insertFinalNewline` | bool | `true` | Ensure a trailing newline on save. |
| `formatOnSave` | bool | `false` | Run the formatter on save. |
| `semanticComments` | object | enabled | Codetag highlighting (`enabled`, `tags`). |
| `completion` | object | enabled | LSP completion (`enabled`, `autoTrigger`). |

#### Per-language editor settings

Every `editor` key can be overridden for one language with a `[language]` selector in
the same object. Selectors are matched case-insensitively against the language reported
for the open document:

```jsonc
{
  "editor": {
    "tabSize": 4,
    "wordWrap": true,
    "[rust]": {
      "tabSize": 2,
      "wordWrap": false,
      "completion": { "autoTrigger": false }
    },
    "[markdown]": {
      "semanticComments": { "enabled": false },
      "stickyScroll": false
    }
  }
}
```

The three configuration layers are merged first, including nested selector objects;
the matching language patch is then applied over the merged global `editor` settings.
This means language specificity wins over layer specificity: a system-level `[rust]`
value still beats a project-level global value for a Rust document. Within the same
selector, the normal layer precedence still applies. Arrays such as `rulers` and
`semanticComments.tags` replace rather than concatenate. Explicit `null` for
`wordWrap` or `graphicalCursor` restores that setting's automatic behavior.

### `files`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `autoSave` | `"off"`\|`"afterDelay"`\|`"onFocusChange"` | `"off"` | When dirty buffers auto-save. |
| `autoSaveDelay` | number | `1000` | Delay (ms) for `afterDelay`. |
| `encoding` | string | `"utf-8"` | Default encoding label. |
| `eol` | `"auto"`\|`"lf"`\|`"crlf"` | `"auto"` | Line-ending style on save. |
| `exclude` | string[] | `[]` | Globs hidden from the file explorer. |
| `watcherExclude` | string[] | `[]` | Globs excluded from the filesystem watcher. |
| `backup` | bool | `true` | Keep crash-recovery backups of unsaved buffers (see below). |
| `backupInterval` | number | `30000` | Milliseconds a buffer stays dirty before its backup is written. |
| `confirmOnExit` | bool | `true` | Prompt to save unsaved changes when quitting. |

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
| `enabled` | bool | `false` | Opt in to spell-checking for editable files. |
| `language` | string | `"en_US"` | Dictionary language: `en_US` or `en_GB`. |
| `words` | string[] | `[]` | Extra correctly-spelled words. |
| `documents` | bool | `true` | Check prose in Markdown, plain text, reStructuredText, AsciiDoc, and TeX. |
| `comments` | bool | `true` | Check comments and documentation prose in source files. |
| `strings` | bool | `false` | Check source-code string literals. |
| `identifiers` | bool | `false` | Check class/type, function, method, and property names while the file parses cleanly. |
| `debounceMs` | number | `500` | Quiet period after the latest token update before checking (clamped to 50–5000 ms). |

Enable the feature in any system, user, or project `setting.jsonc` layer:

```jsonc
{
  "spellcheck": {
    "enabled": true,
    "language": "en_US"
  }
}
```

Spellcheck uses system Hunspell dictionaries at runtime rather than bundling them in
the binary. This keeps the optional feature small. Install both the `.aff` and `.dic`
files for the selected locale in your platform's Hunspell directory, set `DICPATH`, or
copy them into karet's platform data directory under `dictionaries/`. The status bar
shows `English (US)` or `English (UK)` beside the detected file language when active.

A repository can choose between the supported dictionaries without opting users in:

```ini
# .editorconfig
[*.md]
spelling_language = en-GB
```

The `spelling_language` property selects between `en_US` and `en_GB`; it does **not**
enable spellcheck. It is applied only when `spellcheck.enabled` is `true` in a
`setting.jsonc` layer.
URLs, email-like text, numeric/qualified identifiers, code spans, links, and likely
proper names are ignored. Warnings are token-ranged and preserve syntax colours.

When a misspelling has close dictionary matches, they appear in the completion popup
after the debounced warning reaches a stationary caret; `Ctrl+Space` also opens them
when automatic completion is disabled. Double-clicking a spelling squiggle opens the
same replacements in a correction menu. A warning without close matches instead shows
a muted `No similar words found` row, while still offering the dictionary action.

`Add “…” to Project Dictionary` appends the word to `spellcheck.words` in the project
layer. An existing `$GIT_ROOT/.karet/setting.jsonc` is updated in place while retaining
its comments and unrelated settings. If that file does not exist, karet requires the
user to type `create` before it creates the `.karet` settings tree; it never silently
falls back to a user or system dictionary.

### `latex`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `buildOnSave` | bool | `false` | Rebuild a TeX root after successfully saving an editable `.tex` file (manual or automatic). |
| `command` | string | `"latexmk"` | External compiler executable, launched directly without a shell. |
| `args` | string[] | latexmk PDF recipe | Compiler arguments; `{file}`, `{fileDir}`, and `{outputDir}` are substituted inside each argument. |
| `outputDirectory` | string | `""` | Absolute output path, or a path relative to the workspace; empty uses karet's per-workspace platform cache. |
| `timeoutMs` | number | `120000` | Compiler timeout in milliseconds (clamped to 1000–600000). |

The default recipe runs `latexmk -pdf` in nonstop, SyncTeX, and file-line-error
modes. karet never bundles a TeX distribution: install `latexmk` plus the TeX
packages used by the document, or replace `command` and `args` with another engine.
Arguments are passed as an argv array, so paths containing spaces remain one argument
and configuration values are not evaluated by a shell.

Run **LaTeX: Build and Open PDF Preview** from the command palette while an editable
`.tex` file is active. The preview tab appears immediately, the compiler runs off the
editor thread, and closing the pending tab terminates the build. Compiler messages in
`file.tex:line: message` form become editor diagnostics. A source can select a root
file with a TeX magic comment in its first 40 lines; chained directives are supported:

```tex
% !TeX root = ../main.tex
```

The generated PDF opens in karet's existing PDF viewer when the `pdf` application
feature is enabled (it is enabled in default builds). `texlab` is the built-in TeX
language-server default; installing it adds completion and document symbols without
additional configuration. Both the compiler and language server remain external.

### `git`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `decorations` | bool | `true` | Gutter change decorations + file-tree status colouring. |
| `blame` | bool | `true` | Muted cursor-line attribution; click it or press `Alt+B` to open the commit. |

`Ctrl+Shift+B` toggles inline blame and persists this field in the user JSONC file
while retaining comments and unrelated settings. Files without committed Git history
show no attribution and do not report an error.

> An explicit `--icons` flag (or the `KARET_ICONS` environment variable) overrides
> `workbench.iconStyle`.

## Live reload

karet watches every discovered system, user, and project configuration path, including
files that do not exist yet and hidden `.karet` directories. Creating, editing,
atomically replacing, or deleting one of those files reloads only that layer; the other
layers stay in memory and are not reread. The newly merged snapshot is then shared by
the backend and every open pane.

Malformed live edits raise a notification and keep the affected layer's last valid
value, avoiding a temporary reset while a file is half-written. Saving a valid version
applies it immediately; deleting a layer falls back to the remaining layers and
defaults. `workbench.startupPanel` is intentionally not replayed during reload, and
command-line overrides such as `--icons` remain authoritative.

## Crash recovery (unsaved-change backups)

karet keeps your unsaved work safe even if it crashes or a save fails. When a buffer
has been dirty for `files.backupInterval` (and immediately whenever a save fails), its
contents are written to a **swap file** in the platform data directory
(`…/karet/swaps`). Each swap records the original path and a fingerprint of the file it
would overwrite.

- **On a successful save**, the swap is removed.
- **On close** (skipping a save), the swap is removed.
- **On quit with unsaved changes** (when `files.confirmOnExit` is on), karet asks
  whether to save all and quit, discard and quit, or cancel.
- **On the next launch**, if any swaps were left behind (a crash, or a discard-and-quit),
  karet offers to recover them — restoring each buffer as unsaved. If the underlying
  file changed on disk in the meantime, the prompt says so, since recovering would
  discard those on-disk changes.

Set `files.backup` to `false` to disable the whole mechanism.

## Regenerating the schema

The JSON Schema is emitted from the `Settings` type; regenerate it after changing the
schema with:

```sh
cargo run -p karet-session --example settings-schema > settings.schema.json
```

A test (`karet-session`'s `config::tests::checked_in_schema_is_current`) fails if the
committed file drifts.
