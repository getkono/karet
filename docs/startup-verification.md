# Startup Verification

Last updated: 2026-07-08.

This note records the startup coverage for large workspaces and git roots.

## Automated Coverage

- `karet-session::tests::session_new_does_not_walk_large_tree_on_caller_thread`
  builds a 1,200-directory git workspace and asserts, through the event stream
  rather than a stopwatch, that `Session::new` queues no initial status (the
  full `git status` walk) and that `Session::start` delivers it.
- Watch registration stays off the caller's first-frame path by construction:
  `karet_watch::Watcher::spawn_with_paths` enumerates directories on its own
  worker thread. No test times it.
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
