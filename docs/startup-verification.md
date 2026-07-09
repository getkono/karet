# Startup Verification

Last updated: 2026-07-08.

This note records the startup coverage for large workspaces and git roots.

## Automated Coverage

- `karet-session::tests::session_new_does_not_walk_large_tree_on_caller_thread`
  builds a 1,200-directory workspace and asserts `Session::new` returns in under
  one second.
- The test covers the startup invariant that directory enumeration and watch
  registration stay off the caller's first-frame path.
- The regular session and watcher suites cover external change delivery after
  startup.

## Manual Terminal Checklist

Run these in a kitty-keyboard-protocol-capable terminal:

```bash
karet ~
karet <large-git-repo>
```

Expected result:

- The first frame renders promptly.
- Initial git status arrives after the UI is already responsive.
- Editing a nested file, creating a new directory plus file, and switching
  branches are eventually reflected through file watching.

Linux verification in this environment is represented by the automated startup
regression and focused watcher/session tests. macOS terminal verification should
record terminal, repository size, and approximate first-frame latency when run.
