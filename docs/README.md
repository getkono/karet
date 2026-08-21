# karet documentation

User-facing reference lives here; workspace policy (design principles, crate
table, quality gates, testing) lives in [`AGENTS.md`](../AGENTS.md).

| Document | What it answers |
|---|---|
| [`configuration.md`](configuration.md) | Every setting: the three `setting.jsonc` layers, per-key reference, per-language overrides. |
| [`language-servers.md`](language-servers.md) | **The canonical language support matrix**: which languages get which LSP providers, precedence, managed installs, caveats (Java/jdtls included). |
| [`file-formats.md`](file-formats.md) | What opens how: bundled tree-sitter grammars per extension, icon-only recognition, media/document formats, planned formats. |
| [`scope.md`](scope.md) | Deliberate non-goals — TUI theming, terminal graphics protocols, syntax backends. |
| [`visualizations.md`](visualizations.md) | Graph lenses (dependency map via `dependable`, …) and their status. |
| [`binary-size.md`](binary-size.md) | How the app's default-on features (`images`/`pdf`/`docx`) map to dependency subtrees, with measured lean-build deltas. |
| [`cursor-research.md`](cursor-research.md) | Design note: terminal cursor rendering research. |
| [`startup-verification.md`](startup-verification.md) | Design note: startup performance verification. |
