use std::fs;
use std::sync::Mutex;
use std::time::Instant;

use tempfile::TempDir;

use super::*;

/// Native watcher backends are process-global on some platforms, so the
/// integration-style watcher tests must not compete for one event stream.
static WATCH_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn ignores_build_and_vcs_dirs() {
    assert!(is_ignored(Path::new("/repo/target/debug/x")));
    assert!(is_ignored(Path::new("/repo/.git/HEAD")));
    assert!(is_ignored(Path::new("/repo/node_modules/p/i.js")));
    assert!(!is_ignored(Path::new("/repo/src/main.rs")));
}

#[test]
fn convert_maps_kinds_and_filters_ignored() {
    use notify::event::CreateKind;
    let src = vec![PathBuf::from("/r/src/a.rs")];
    let roots = vec![PathBuf::from("/r")];
    let exact = BTreeSet::new();
    let got = convert(
        EventKind::Create(CreateKind::File),
        &src,
        &roots,
        &[],
        &exact,
    );
    assert!(got.is_some());
    if let Some(got) = got {
        assert_eq!(got.kind, FsEventKind::Created);
        assert_eq!(got.paths, src);
    }

    // An event only touching an ignored path is dropped entirely.
    let ignored = vec![PathBuf::from("/r/target/a")];
    assert!(
        convert(
            EventKind::Create(CreateKind::File),
            &ignored,
            &roots,
            &[],
            &exact,
        )
        .is_none()
    );

    // Access events are not changes.
    assert!(
        convert(
            EventKind::Access(notify::event::AccessKind::Read),
            &src,
            &roots,
            &[],
            &exact,
        )
        .is_none()
    );
}

#[test]
fn exact_path_bypasses_workspace_and_hidden_filters() {
    let exact_path = PathBuf::from("/outside/.karet/setting.jsonc");
    let exact = BTreeSet::from([exact_path.clone()]);
    assert!(keep_path(&exact_path, &[], &[], &exact));
    assert!(!keep_path(
        Path::new("/outside/.karet/other.jsonc"),
        &[],
        &[],
        &exact
    ));
}

#[test]
fn git_meta_allowlist() {
    // Kept: the index, HEAD family, and refs.
    assert!(is_interesting_git_meta(Path::new("index")));
    assert!(is_interesting_git_meta(Path::new("HEAD")));
    assert!(is_interesting_git_meta(Path::new("packed-refs")));
    assert!(is_interesting_git_meta(Path::new("MERGE_HEAD")));
    assert!(is_interesting_git_meta(Path::new("refs/heads/main")));
    // Dropped: lock files, the object/log trees, and scratch files.
    assert!(!is_interesting_git_meta(Path::new("index.lock")));
    assert!(!is_interesting_git_meta(Path::new("objects/ab/cdef")));
    assert!(!is_interesting_git_meta(Path::new("logs/HEAD")));
    assert!(!is_interesting_git_meta(Path::new("FETCH_HEAD")));
    assert!(!is_interesting_git_meta(Path::new("COMMIT_EDITMSG")));
}

#[test]
fn keep_path_applies_git_allowlist_under_git_dir() {
    let roots = vec![PathBuf::from("/repo")];
    let git_dirs = vec![PathBuf::from("/repo/.git")];
    let exact = BTreeSet::new();
    // Inside the git dir: only allowlisted metadata survives.
    assert!(keep_path(
        Path::new("/repo/.git/index"),
        &roots,
        &git_dirs,
        &exact
    ));
    assert!(keep_path(
        Path::new("/repo/.git/refs/heads/main"),
        &roots,
        &git_dirs,
        &exact
    ));
    assert!(!keep_path(
        Path::new("/repo/.git/objects/ab/cd"),
        &roots,
        &git_dirs,
        &exact
    ));
    assert!(!keep_path(
        Path::new("/repo/.git/index.lock"),
        &roots,
        &git_dirs,
        &exact
    ));
    // Outside the git dir: the usual ignore rules apply.
    assert!(keep_path(
        Path::new("/repo/src/main.rs"),
        &roots,
        &git_dirs,
        &exact
    ));
    assert!(!keep_path(
        Path::new("/repo/target/x"),
        &roots,
        &git_dirs,
        &exact
    ));
    // With no git dirs tracked, `.git` is dropped wholesale as before.
    assert!(!keep_path(
        Path::new("/repo/.git/index"),
        &roots,
        &[],
        &exact
    ));
}

#[test]
fn keep_path_handles_worktree_git_dirs() {
    let roots = vec![PathBuf::from("/main")];
    // The per-worktree git dir (longest) is listed before the common dir.
    let git_dirs = vec![
        PathBuf::from("/main/.git/worktrees/wt"),
        PathBuf::from("/main/.git"),
    ];
    let exact = BTreeSet::new();
    assert!(keep_path(
        Path::new("/main/.git/worktrees/wt/index"),
        &roots,
        &git_dirs,
        &exact
    ));
    assert!(keep_path(
        Path::new("/main/.git/worktrees/wt/HEAD"),
        &roots,
        &git_dirs,
        &exact
    ));
    assert!(keep_path(
        Path::new("/main/.git/refs/heads/main"),
        &roots,
        &git_dirs,
        &exact
    ));
    assert!(!keep_path(
        Path::new("/main/.git/worktrees/wt/ORIG_HEAD.lock"),
        &roots,
        &git_dirs,
        &exact
    ));
}

#[test]
fn error_displays() {
    assert_eq!(
        WatchError::Backend("nope".to_string()).to_string(),
        "filesystem watch error: nope"
    );
}

#[test]
fn enumerate_dirs_skips_ignored_hidden_and_gitignored() -> Result<(), Box<dyn std::error::Error>> {
    let root = TempDir::new()?;
    let root = root.path();
    fs::create_dir_all(root.join("src"))?;
    fs::create_dir_all(root.join("node_modules/x"))?;
    fs::create_dir_all(root.join("target/debug"))?;
    fs::create_dir_all(root.join(".hidden"))?;
    fs::create_dir_all(root.join("build"))?;
    fs::write(root.join(".gitignore"), "build/\n")?;

    let dirs = enumerate_dirs(root);
    assert!(dirs.contains(&root.to_path_buf()));
    assert!(dirs.contains(&root.join("src")));
    assert!(
        !dirs
            .iter()
            .any(|d| d.starts_with(root.join("node_modules")))
    );
    assert!(!dirs.iter().any(|d| d.starts_with(root.join("target"))));
    assert!(!dirs.contains(&root.join(".hidden")));
    assert!(!dirs.contains(&root.join("build")));
    Ok(())
}

#[test]
fn remove_watched_under_prunes_descendants_only() {
    let mut watched = BTreeSet::from([
        PathBuf::from("/repo"),
        PathBuf::from("/repo/src"),
        PathBuf::from("/repo/src/nested"),
        PathBuf::from("/repo2/src"),
    ]);

    let removed = remove_watched_under(&mut watched, Path::new("/repo/src"));

    assert_eq!(
        removed,
        vec![
            PathBuf::from("/repo/src"),
            PathBuf::from("/repo/src/nested"),
        ]
    );
    assert!(watched.contains(Path::new("/repo")));
    assert!(watched.contains(Path::new("/repo2/src")));
    assert!(!watched.contains(Path::new("/repo/src")));
}

/// Poll `rx` until an event matching `pred` arrives or `deadline` elapses.
fn wait_for_event(
    rx: &mut UnboundedReceiver<FsEvent>,
    deadline: Duration,
    pred: impl Fn(&FsEvent) -> bool,
) -> bool {
    let start = Instant::now();
    while start.elapsed() < deadline {
        match rx.try_recv() {
            Ok(event) if pred(&event) => return true,
            Ok(_) => {},
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    false
}

#[test]
fn watcher_surfaces_nested_source_file() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = WATCH_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = TempDir::new()?;
    let deep = root.path().join("src/deep");
    fs::create_dir_all(&deep)?;

    let (_watcher, mut rx) = Watcher::spawn(&[root.path().to_path_buf()], &[])?;
    // Let the background worker finish registering watches.
    std::thread::sleep(Duration::from_secs(1));

    let file = deep.join("new.rs");
    fs::write(&file, "fn main() {}")?;

    assert!(wait_for_event(&mut rx, Duration::from_secs(5), |e| {
        e.kind == FsEventKind::Created && e.paths.contains(&file)
    }));
    Ok(())
}

#[test]
fn watcher_covers_dynamically_created_dir() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = WATCH_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = TempDir::new()?;
    fs::create_dir_all(root.path().join("src"))?;

    let (_watcher, mut rx) = Watcher::spawn(&[root.path().to_path_buf()], &[])?;
    std::thread::sleep(Duration::from_secs(1));

    let sub = root.path().join("src/added/sub");
    fs::create_dir_all(&sub)?;
    // Give the worker a moment to notice the new directory and watch it before a
    // file lands inside it.
    std::thread::sleep(Duration::from_secs(1));
    let file = sub.join("f.rs");
    fs::write(&file, "fn main() {}")?;

    assert!(wait_for_event(&mut rx, Duration::from_secs(10), |e| {
        e.paths.contains(&file)
    }));
    Ok(())
}

#[test]
fn watcher_surfaces_exact_hidden_file_outside_roots() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = WATCH_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = TempDir::new()?;
    let config_dir = root.path().join(".karet");
    fs::create_dir_all(&config_dir)?;
    let config = config_dir.join("setting.jsonc");
    fs::write(&config, "{}")?;

    let (_watcher, mut rx) = Watcher::spawn_with_paths(&[], &[], std::slice::from_ref(&config))?;
    std::thread::sleep(Duration::from_secs(1));
    fs::write(&config, r#"{ "editor": {} }"#)?;

    assert!(wait_for_event(&mut rx, Duration::from_secs(5), |e| {
        e.paths.contains(&config)
    }));
    Ok(())
}

#[test]
fn watcher_follows_missing_exact_path_and_atomic_replace() -> Result<(), Box<dyn std::error::Error>>
{
    let _guard = WATCH_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = TempDir::new()?;
    let config_dir = root.path().join("config/karet");
    let config = config_dir.join("setting.jsonc");

    let (_watcher, mut rx) = Watcher::spawn_with_paths(&[], &[], std::slice::from_ref(&config))?;
    std::thread::sleep(Duration::from_secs(1));

    fs::create_dir_all(&config_dir)?;
    std::thread::sleep(Duration::from_millis(500));
    let unrelated = config_dir.join("other.jsonc");
    fs::write(&unrelated, "{}")?;
    assert!(!wait_for_event(&mut rx, Duration::from_millis(600), |e| e
        .paths
        .contains(&unrelated)));

    let temporary = config_dir.join("setting.jsonc.tmp");
    fs::write(&temporary, r#"{ "editor": { "tabSize": 2 } }"#)?;
    fs::rename(&temporary, &config)?;
    assert!(wait_for_event(&mut rx, Duration::from_secs(5), |e| {
        e.paths.contains(&config)
    }));
    Ok(())
}

#[test]
fn watcher_filters_build_dirs() -> Result<(), Box<dyn std::error::Error>> {
    let _guard = WATCH_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = TempDir::new()?;
    fs::create_dir_all(root.path().join("target"))?;
    fs::create_dir_all(root.path().join("node_modules"))?;

    let (_watcher, mut rx) = Watcher::spawn(&[root.path().to_path_buf()], &[])?;
    std::thread::sleep(Duration::from_secs(1));

    fs::write(root.path().join("target/x"), "x")?;
    fs::write(root.path().join("node_modules/y"), "y")?;

    assert!(!wait_for_event(&mut rx, Duration::from_millis(800), |_| {
        true
    }));
    Ok(())
}
