# karet documentation

User-facing reference lives here; workspace policy (design principles, crate
table, quality gates, testing) lives in [`AGENTS.md`](../AGENTS.md).

| Document | What it answers |
|---|---|
| [`goals.md`](goals.md) | **The design goals** karet is built to, each with a checkable rule — the standard issues and pull requests are judged against. |
| [`configuration.md`](configuration.md) | Every setting: the three `setting.jsonc` layers, per-key reference, per-language overrides. |
| [`editing-behavior.md`](editing-behavior.md) | **The normative caret and selection spec**: every motion, selection, multi-caret, clipboard, click, multi-click, drag, wheel and modifier gesture, VS Code's behaviour unless a deviation is recorded, with each row's conformance status. |
| [`debugging.md`](debugging.md) | The DAP debugger: adapters, configurations, keys, breakpoint semantics. |
| [`language-servers.md`](language-servers.md) | **The canonical language support matrix**: which languages get which LSP providers, precedence, managed installs, caveats (Java/jdtls included). |
| [`file-formats.md`](file-formats.md) | What opens how: bundled tree-sitter grammars per extension, icon-only recognition, media/document formats, what the markdown preview renders (HTML subset, images), planned formats. |
| [`scope.md`](scope.md) | What was weighed and kept in scope (integrated terminal, AI inline completion, modal editing, task runner), and the deliberate non-goals — TUI theming, terminal graphics protocols, markdown-preview HTML and images, syntax backends, agent sessions (ACP). |
| [`visualizations.md`](visualizations.md) | Graph lenses (dependency map via `dependable`, …) and their status. |
| [`binary-size.md`](binary-size.md) | How the app's default-on features (`images`/`pdf`/`docx`) map to dependency subtrees, with measured lean-build deltas. |
| [`cursor-research.md`](cursor-research.md) | Design note: terminal cursor rendering research. |
| [`startup-verification.md`](startup-verification.md) | Design note: startup performance verification. |
