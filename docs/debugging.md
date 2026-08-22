# Debugging

karet speaks the Debug Adapter Protocol (DAP) natively — the same protocol VS
Code's debuggers use — through the `karet-dap` client and the backend's
debugger orchestration. Any conforming adapter works; nothing is bundled or
downloaded (adapters are toolchain-coupled, so installing them is deliberately
yours: the same "explicitly manual" policy as SDK-coupled language servers).

## Quick start

1. Install an adapter and make sure it is on `PATH`. Known out of the box:

   | Adapter name | Runs | Notes |
   |---|---|---|
   | `codelldb` | native (Rust/C/C++/…) | launched as `codelldb --port ${port}` over TCP |
   | `lldb-dap` | native | ships with LLVM |
   | `gdb` | native | `gdb -i dap`, GDB 14+ |
   | `debugpy` | Python | `python3 -m debugpy.adapter`; `pip install debugpy` |

2. Add a configuration (project `.karet/setting.jsonc` is the natural home):

   ```jsonc
   {
     "debug": {
       "configurations": [
         {
           "name": "run tests binary",
           "adapter": "codelldb",
           "arguments": { "program": "target/debug/app", "cwd": "." }
         }
       ]
     }
   }
   ```

   `arguments` is passed to the adapter verbatim (each adapter documents its
   own launch/attach schema — `program`, `args`, `cwd`, `env`, `pid`, …).
   There is no `launch.json` compatibility layer, and no variable
   interpolation yet: write real paths.

3. `F9` toggles a breakpoint on the caret line (or click the gutter's leading
   marker column); `F5` starts the first configuration.

Custom or differently-installed adapters get an entry under `debug.adapters`;
a configuration's `adapter` field then names it:

```jsonc
{
  "debug": {
    "adapters": {
      "my-lldb": { "command": "/opt/llvm/bin/lldb-dap", "args": [], "transport": "stdio" }
    }
  }
}
```

## Keys

| Key | Action |
|---|---|
| `F5` | Start (first configuration) / continue when stopped |
| `Shift+F5` | Stop (terminates the debuggee when the adapter supports it) |
| `F6` | Pause |
| `F9` | Toggle breakpoint on the caret line |
| `F10` | Step over |
| `F11` / `Shift+F11` | Step into / step out |

The explorer keeps its `F5` refresh while it has focus.

## What you see

- Breakpoints render in the gutter: `●` once the adapter verifies them, `○`
  while armed but unverified (set before a session, or not yet bound —
  adapters verify late, and the marker updates when they do).
- The status bar carries the session state (`⏳ debug`, `▶ debug`,
  `⏸ breakpoint`); stops jump the editor to the stopped line.
- The **Debug panel** (`Ctrl+6`, or its activity-bar icon) shows the stopped
  thread's call stack (Enter on a frame jumps to it and loads its scopes), a
  lazily-fetched variables tree (expand to fetch children; the first cheap
  scope auto-expands), the evaluate log, and the console tail with ANSI
  colors preserved. Everything per-stop clears on resume — nothing stale
  survives a `continue`.
- The stopped line carries its own background tint until the debuggee resumes.
- **Evaluate** (palette: `Debug: Evaluate Expression`) prompts for an
  expression and runs it in the selected frame's context; results (and adapter
  rejections) append to the panel's evaluate log.

## Semantics worth knowing

- Breakpoints are per-file **full-replace**: every toggle sends the file's
  whole set, exactly the protocol's model, so editor and adapter can never
  drift.
- The launch handshake is capability-gated: `setExceptionBreakpoints` is only
  sent when the adapter offers filters (default-on filters selected), and
  `configurationDone` only when supported.
- One session at a time; a second `F5` while running tells you so.
