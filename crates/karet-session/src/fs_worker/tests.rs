//! Filesystem-worker tests.
//!
//! Every case goes through [`answer`](super::answer) — the same function the
//! worker thread calls — so what is tested is the job-to-event contract, not a
//! helper that happens to sit beside it.

use std::path::Path;

use karet_core::DirEntry;

use super::*;

/// What every test returns: a missing setup or answer fails the test through `?`
/// (the workspace lint floor bans `expect` in tests too).
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A scratch tree with `files` written into it.
///
/// `None` when the platform cannot provide one; callers turn that into a test
/// failure with `.ok_or(..)?`, as they do every other `None` helper answer.
fn tree(files: &[(&str, &[u8])]) -> Option<tempfile::TempDir> {
    let dir = tempfile::tempdir().ok()?;
    for (name, bytes) in files {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::write(&path, bytes).ok()?;
    }
    Some(dir)
}

fn classify_path(path: &Path, ignore_size: bool) -> Option<Result<PathClass, String>> {
    let (_, event) = answer(FsJob::Classify {
        id: RequestId(1),
        path: path.to_path_buf(),
        ignore_size,
    });
    match event {
        Event::PathClassified { result, .. } => Some(result),
        _ => None,
    }
}

fn read_bytes(path: &Path, offset: u64, len: u64) -> Option<Result<FileChunk, String>> {
    let (_, event) = answer(FsJob::ReadBytes {
        id: RequestId(1),
        path: path.to_path_buf(),
        offset,
        len,
    });
    match event {
        Event::FileBytes { result, .. } => Some(result),
        _ => None,
    }
}

fn list_dir(
    path: &Path,
    show_hidden: bool,
    respect_gitignore: bool,
) -> Option<Result<Vec<DirEntry>, String>> {
    let (_, event) = answer(FsJob::ReadDirectory {
        id: RequestId(1),
        path: path.to_path_buf(),
        show_hidden,
        respect_gitignore,
    });
    match event {
        Event::DirectoryListed { result, .. } => Some(result),
        _ => None,
    }
}

fn labels(entries: &[DirEntry]) -> Vec<String> {
    entries.iter().map(|e| e.label().to_owned()).collect()
}

fn mutate(mutation: PathMutation) -> Option<(PathMutation, Result<(), String>)> {
    let (_, event) = answer(FsJob::Mutate {
        id: RequestId(1),
        mutation,
    });
    match event {
        Event::PathMutated { mutation, result } => Some((mutation, result)),
        _ => None,
    }
}

#[test]
fn a_text_file_classifies_as_text_with_its_length_and_head() -> TestResult {
    let dir = tree(&[("main.rs", b"fn main() {}\n")]).ok_or("a scratch tree")?;

    let class =
        classify_path(&dir.path().join("main.rs"), false).ok_or("a classification answer")??;

    assert_eq!(class.kind, karet_filetype::FileKind::Text);
    assert_eq!(class.len, 13);
    assert_eq!(class.head, b"fn main() {}\n");
    Ok(())
}

/// Classification is defined over whatever leading bytes exist. An empty file
/// must classify rather than error, or opening a freshly created file fails.
#[test]
fn an_empty_file_still_classifies() -> TestResult {
    let dir = tree(&[("new.txt", b"")]).ok_or("a scratch tree")?;

    let class =
        classify_path(&dir.path().join("new.txt"), false).ok_or("a classification answer")??;

    assert_eq!(class.len, 0);
    assert!(class.head.is_empty());
    Ok(())
}

#[test]
fn classifying_a_missing_path_reports_why() -> TestResult {
    let dir = tree(&[]).ok_or("a scratch tree")?;

    let result =
        classify_path(&dir.path().join("absent.rs"), false).ok_or("a classification answer")?;

    assert!(result.is_err());
    Ok(())
}

#[test]
fn reading_bytes_answers_a_chunk_that_knows_the_total_length() -> TestResult {
    let dir = tree(&[("data.bin", &[0, 1, 2, 3, 4, 5, 6, 7])]).ok_or("a scratch tree")?;

    let chunk = read_bytes(&dir.path().join("data.bin"), 2, 3).ok_or("a read answer")??;

    assert_eq!(chunk.offset, 2);
    assert_eq!(chunk.bytes, vec![2, 3, 4]);
    assert_eq!(chunk.total_len, 8);
    assert!(!chunk.is_final());
    Ok(())
}

/// A client walking a file to its end asks for one range past it. That must
/// terminate the read, not error — otherwise the last chunk never arrives.
#[test]
fn reading_past_the_end_answers_an_empty_final_chunk() -> TestResult {
    let dir = tree(&[("data.bin", &[0, 1, 2, 3])]).ok_or("a scratch tree")?;

    let chunk = read_bytes(&dir.path().join("data.bin"), 4, 16).ok_or("a read answer")??;

    assert!(chunk.bytes.is_empty());
    assert!(chunk.is_final());
    Ok(())
}

/// The chunk cap keeps a large PDF from monopolizing the stream ahead of an
/// interactive edit, so an oversized request must be clamped, not honored.
#[test]
fn a_read_larger_than_the_chunk_cap_is_clamped() -> TestResult {
    let big = vec![7_u8; (MAX_CHUNK as usize) + 4096];
    let dir = tree(&[("big.bin", &big)]).ok_or("a scratch tree")?;

    let chunk = read_bytes(&dir.path().join("big.bin"), 0, u64::MAX).ok_or("a read answer")??;

    assert_eq!(chunk.bytes.len() as u64, MAX_CHUNK);
    assert!(!chunk.is_final());
    Ok(())
}

#[test]
fn listing_the_workspace_reports_paths_and_whether_it_was_cut_short() -> TestResult {
    let dir = tree(&[
        ("a.rs", b"x"),
        ("b.rs", b"x"),
        ("c.rs", b"x"),
        ("target/generated.rs", b"x"),
    ])
    .ok_or("a scratch tree")?;

    let (_, event) = answer(FsJob::ListFiles {
        id: RequestId(1),
        root: dir.path().to_path_buf(),
        limit: 100,
    });

    let Event::FilesListed { files, truncated } = event else {
        return Err("a workspace listing answers FilesListed".into());
    };
    let names: Vec<String> = files
        .iter()
        .filter_map(|path| Some(path.file_name()?.to_str()?.to_owned()))
        .collect();
    assert_eq!(names, ["a.rs", "b.rs", "c.rs"]);
    assert!(
        !truncated,
        "the pruned target/ must not count toward the cap"
    );
    Ok(())
}

#[test]
fn a_truncated_workspace_listing_says_so() -> TestResult {
    let dir = tree(&[("a.rs", b"x"), ("b.rs", b"x"), ("c.rs", b"x")]).ok_or("a scratch tree")?;

    let (_, event) = answer(FsJob::ListFiles {
        id: RequestId(1),
        root: dir.path().to_path_buf(),
        limit: 2,
    });

    let Event::FilesListed { files, truncated } = event else {
        return Err("a workspace listing answers FilesListed".into());
    };
    assert_eq!(files.len(), 2);
    assert!(truncated);
    Ok(())
}

#[test]
fn a_directory_listing_puts_directories_first_and_excludes_dot_git() -> TestResult {
    let dir = tree(&[
        ("zeta.rs", b"x"),
        ("alpha.rs", b"x"),
        ("src/lib.rs", b"x"),
        (".git/HEAD", b"ref: refs/heads/master\n"),
    ])
    .ok_or("a scratch tree")?;

    let entries = list_dir(dir.path(), true, false).ok_or("a listing answer")??;

    assert_eq!(labels(&entries), ["src", "alpha.rs", "zeta.rs"]);
    Ok(())
}

/// Ignored entries are flagged, never filtered: a user must still see the
/// `target/` directory their build produced, dimmed rather than absent.
#[test]
fn gitignored_entries_are_listed_and_flagged() -> TestResult {
    let dir = tree(&[
        (".gitignore", b"ignored.rs\n"),
        ("kept.rs", b"x"),
        ("ignored.rs", b"x"),
    ])
    .ok_or("a scratch tree")?;

    let entries = list_dir(dir.path(), true, true).ok_or("a listing answer")??;

    let flagged: Vec<(String, bool)> = entries
        .iter()
        .map(|entry| (entry.label().to_owned(), entry.ignored))
        .collect();
    assert!(
        flagged.contains(&("ignored.rs".to_owned(), true)),
        "{flagged:?}"
    );
    assert!(
        flagged.contains(&("kept.rs".to_owned(), false)),
        "{flagged:?}"
    );
    Ok(())
}

#[test]
fn hidden_entries_are_omitted_unless_asked_for() -> TestResult {
    let dir = tree(&[(".env", b"SECRET=1"), ("main.rs", b"x")]).ok_or("a scratch tree")?;

    let hidden = list_dir(dir.path(), false, false).ok_or("a listing answer")??;
    let shown = list_dir(dir.path(), true, false).ok_or("a listing answer")??;

    assert_eq!(labels(&hidden), ["main.rs"]);
    assert!(labels(&shown).contains(&".env".to_owned()), "{shown:?}");
    Ok(())
}

#[test]
fn listing_a_file_rather_than_a_directory_reports_why() -> TestResult {
    let dir = tree(&[("main.rs", b"x")]).ok_or("a scratch tree")?;

    let result = list_dir(&dir.path().join("main.rs"), false, false).ok_or("a listing answer")?;

    assert!(result.is_err());
    Ok(())
}

#[test]
fn creating_a_file_makes_it_and_its_parents() -> TestResult {
    let dir = tree(&[]).ok_or("a scratch tree")?;
    let path = dir.path().join("deep/nested/new.rs");

    let (echoed, result) =
        mutate(PathMutation::CreateFile { path: path.clone() }).ok_or("a mutation answer")?;

    assert_eq!(result, Ok(()));
    assert!(path.is_file());
    assert_eq!(echoed.target(), &path);
    Ok(())
}

/// "New file" must never truncate something the user forgot about.
#[test]
fn creating_a_file_that_exists_refuses_rather_than_truncating() -> TestResult {
    let dir = tree(&[("keep.rs", b"important")]).ok_or("a scratch tree")?;
    let path = dir.path().join("keep.rs");

    let (_, result) =
        mutate(PathMutation::CreateFile { path: path.clone() }).ok_or("a mutation answer")?;

    assert!(result.is_err());
    assert_eq!(std::fs::read(&path).ok(), Some(b"important".to_vec()));
    Ok(())
}

/// `fs::rename` overwrites on Unix. The explorer must not.
#[test]
fn renaming_onto_an_existing_path_refuses() -> TestResult {
    let dir = tree(&[("from.rs", b"source"), ("to.rs", b"destination")]).ok_or("a scratch tree")?;

    let (_, result) = mutate(PathMutation::Rename {
        from: dir.path().join("from.rs"),
        to: dir.path().join("to.rs"),
    })
    .ok_or("a mutation answer")?;

    assert!(result.is_err());
    assert_eq!(
        std::fs::read(dir.path().join("to.rs")).ok(),
        Some(b"destination".to_vec())
    );
    Ok(())
}

#[test]
fn renaming_moves_the_file() -> TestResult {
    let dir = tree(&[("from.rs", b"source")]).ok_or("a scratch tree")?;

    let (_, result) = mutate(PathMutation::Rename {
        from: dir.path().join("from.rs"),
        to: dir.path().join("moved/to.rs"),
    })
    .ok_or("a mutation answer")?;

    assert_eq!(result, Ok(()));
    assert!(!dir.path().join("from.rs").exists());
    assert_eq!(
        std::fs::read(dir.path().join("moved/to.rs")).ok(),
        Some(b"source".to_vec())
    );
    Ok(())
}

#[test]
fn copying_a_directory_copies_everything_under_it() -> TestResult {
    let dir = tree(&[("src/a.rs", b"a"), ("src/deep/b.rs", b"b")]).ok_or("a scratch tree")?;

    let (_, result) = mutate(PathMutation::Copy {
        from: dir.path().join("src"),
        to: dir.path().join("copy"),
    })
    .ok_or("a mutation answer")?;

    assert_eq!(result, Ok(()));
    assert_eq!(
        std::fs::read(dir.path().join("copy/a.rs")).ok(),
        Some(b"a".to_vec())
    );
    assert_eq!(
        std::fs::read(dir.path().join("copy/deep/b.rs")).ok(),
        Some(b"b".to_vec())
    );
    Ok(())
}

#[test]
fn deleting_removes_a_directory_and_its_contents() -> TestResult {
    let dir = tree(&[("gone/a.rs", b"a"), ("gone/deep/b.rs", b"b")]).ok_or("a scratch tree")?;

    let (_, result) = mutate(PathMutation::Delete {
        path: dir.path().join("gone"),
    })
    .ok_or("a mutation answer")?;

    assert_eq!(result, Ok(()));
    assert!(!dir.path().join("gone").exists());
    Ok(())
}

/// Deleting a link to a directory must remove the link, never the directory it
/// points at — the difference between tidying up and losing work.
#[cfg(unix)]
#[test]
fn deleting_a_symlink_to_a_directory_leaves_the_target() -> TestResult {
    let dir = tree(&[("real/keep.rs", b"keep")]).ok_or("a scratch tree")?;
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(dir.path().join("real"), &link)?;

    let (_, result) =
        mutate(PathMutation::Delete { path: link.clone() }).ok_or("a mutation answer")?;

    assert_eq!(result, Ok(()));
    assert!(!link.exists());
    assert_eq!(
        std::fs::read(dir.path().join("real/keep.rs")).ok(),
        Some(b"keep".to_vec())
    );
    Ok(())
}

/// A trailing separator makes the OS resolve a final link, so `link/` names the
/// directory it points at. Any client can send the path, and the delete must
/// still remove the link and leave the target whole.
#[cfg(unix)]
#[test]
fn deleting_a_directory_link_named_with_a_trailing_slash_leaves_the_target() -> TestResult {
    let dir = tree(&[("real/keep.rs", b"keep")]).ok_or("a scratch tree")?;
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(dir.path().join("real"), &link)?;

    let (_, result) = mutate(PathMutation::Delete {
        path: dir.path().join("link/"),
    })
    .ok_or("a mutation answer")?;

    assert_eq!(result, Ok(()));
    assert!(link.symlink_metadata().is_err(), "the link itself is gone");
    assert_eq!(
        std::fs::read(dir.path().join("real/keep.rs")).ok(),
        Some(b"keep".to_vec())
    );
    Ok(())
}

/// The same for a copy: `link/` is copied as the link, not as the tree it
/// reaches.
#[cfg(unix)]
#[test]
fn copying_a_directory_link_named_with_a_trailing_slash_copies_the_link() -> TestResult {
    let dir = tree(&[("real/keep.rs", b"keep")]).ok_or("a scratch tree")?;
    std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link"))?;

    let (_, result) = mutate(PathMutation::Copy {
        from: dir.path().join("link/"),
        to: dir.path().join("copy"),
    })
    .ok_or("a mutation answer")?;

    assert_eq!(result, Ok(()));
    assert!(
        dir.path()
            .join("copy")
            .symlink_metadata()
            .is_ok_and(|entry| entry.is_symlink())
    );
    Ok(())
}

#[test]
fn deleting_a_missing_path_reports_why() -> TestResult {
    let dir = tree(&[]).ok_or("a scratch tree")?;

    let (_, result) = mutate(PathMutation::Delete {
        path: dir.path().join("absent.rs"),
    })
    .ok_or("a mutation answer")?;

    assert!(result.is_err());
    Ok(())
}

/// Confirming F2 without changing the name renames a path onto itself. That is
/// a no-op, not a clash with an existing file.
#[test]
fn renaming_a_path_onto_itself_succeeds_and_keeps_it() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(&[("same.rs", b"keep")]).ok_or("a scratch tree")?;
    let path = dir.path().join("same.rs");

    let (_, result) = mutate(PathMutation::Rename {
        from: path.clone(),
        to: path.clone(),
    })
    .ok_or("a mutation answer")?;

    assert_eq!(result, Ok(()));
    assert_eq!(std::fs::read(&path)?, b"keep".to_vec());
    Ok(())
}

/// A case-only rename on a case-insensitive filesystem finds the "destination"
/// already there, because it is the source. Probed rather than assumed: on a
/// case-sensitive filesystem the two spellings are two paths and there is
/// nothing to show.
#[test]
fn a_case_only_rename_is_not_refused_as_a_clash() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(&[("readme.md", b"text")]).ok_or("a scratch tree")?;
    let from = dir.path().join("readme.md");
    let to = dir.path().join("README.md");
    if to.symlink_metadata().is_err() {
        return Ok(()); // case-sensitive: the rename is an ordinary one
    }

    let (_, result) = mutate(PathMutation::Rename {
        from,
        to: to.clone(),
    })
    .ok_or("a mutation answer")?;

    assert_eq!(result, Ok(()));
    let names: Vec<_> = std::fs::read_dir(dir.path())?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .collect();
    assert_eq!(names, vec![std::ffi::OsString::from("README.md")]);
    Ok(())
}

/// Any client can send a copy, so the backend itself must refuse a directory
/// copied into its own subtree: that recursion runs until the path limit.
#[test]
fn copying_a_directory_into_itself_refuses() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(&[("src/a.rs", b"a")]).ok_or("a scratch tree")?;

    let (_, result) = mutate(PathMutation::Copy {
        from: dir.path().join("src"),
        to: dir.path().join("src/nested/copy"),
    })
    .ok_or("a mutation answer")?;

    assert!(result.is_err());
    assert!(!dir.path().join("src/nested").exists());
    Ok(())
}

/// The cross-device fallback: what a rename across a mount point does instead.
#[test]
fn moving_by_copy_then_delete_moves_a_directory() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(&[("src/a.rs", b"a"), ("src/deep/b.rs", b"b")]).ok_or("a scratch tree")?;
    let from = dir.path().join("src");
    let to = dir.path().join("moved");

    super::mutate::move_by_copy(&from, &to)?;

    assert!(!from.exists());
    assert_eq!(std::fs::read(to.join("deep/b.rs"))?, b"b".to_vec());
    Ok(())
}

/// A link to a directory is listed as a link, not as a directory: expanding it
/// would chain-compact through it, and a link to an ancestor would drive an
/// endless run of nested listings.
#[cfg(unix)]
#[test]
fn a_symlink_to_a_directory_is_listed_as_a_link_not_a_directory()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tree(&[("real/keep.rs", b"keep")]).ok_or("a scratch tree")?;
    std::os::unix::fs::symlink(dir.path(), dir.path().join("up"))?;

    let entries = list_dir(dir.path(), false, false).ok_or("a listing answer")??;

    let link = entries
        .iter()
        .find(|entry| entry.label() == "up")
        .ok_or("the link is listed")?;
    assert!(link.is_symlink);
    assert!(!link.is_dir);
    Ok(())
}

/// A copy, like a rename, never clobbers: the explorer picks a free name, and a
/// client that did not is told rather than losing the file there.
#[test]
fn copying_onto_an_existing_path_refuses() -> TestResult {
    let dir = tree(&[("a.rs", b"a"), ("b.rs", b"b")]).ok_or("a scratch tree")?;

    let (_, result) = mutate(PathMutation::Copy {
        from: dir.path().join("a.rs"),
        to: dir.path().join("b.rs"),
    })
    .ok_or("a mutation answer")?;

    assert!(result.is_err());
    assert_eq!(std::fs::read(dir.path().join("b.rs"))?, b"b".to_vec());
    Ok(())
}

#[test]
fn copying_a_file_copies_its_bytes_and_keeps_the_source() -> TestResult {
    let dir = tree(&[("a.rs", b"alpha")]).ok_or("a scratch tree")?;

    let (_, result) = mutate(PathMutation::Copy {
        from: dir.path().join("a.rs"),
        to: dir.path().join("sub/a copy.rs"),
    })
    .ok_or("a mutation answer")?;

    assert_eq!(result, Ok(()));
    assert_eq!(
        std::fs::read(dir.path().join("sub/a copy.rs"))?,
        b"alpha".to_vec()
    );
    assert_eq!(std::fs::read(dir.path().join("a.rs"))?, b"alpha".to_vec());
    Ok(())
}

/// A destination spelled through `..` is inside the source once resolved, and
/// the copy is refused just as the plain spelling is.
#[test]
fn copying_a_directory_into_itself_through_dot_dot_refuses() -> TestResult {
    let dir = tree(&[("src/a.rs", b"a"), ("other/keep", b"")]).ok_or("a scratch tree")?;

    let (_, result) = mutate(PathMutation::Copy {
        from: dir.path().join("src"),
        to: dir.path().join("other/../src/copy"),
    })
    .ok_or("a mutation answer")?;

    assert!(result.is_err());
    assert!(!dir.path().join("src/copy").exists());
    Ok(())
}

/// A destination reached through a link into the source is inside it too.
#[cfg(unix)]
#[test]
fn copying_a_directory_into_itself_through_a_link_refuses() -> TestResult {
    let dir = tree(&[("src/a.rs", b"a")]).ok_or("a scratch tree")?;
    std::os::unix::fs::symlink(dir.path().join("src"), dir.path().join("alias"))?;

    let (_, result) = mutate(PathMutation::Copy {
        from: dir.path().join("src"),
        to: dir.path().join("alias/copy"),
    })
    .ok_or("a mutation answer")?;

    assert!(result.is_err());
    assert!(!dir.path().join("src/copy").exists());
    Ok(())
}

/// A link to a directory inside a copied tree is copied as a link. Followed, a
/// link to an ancestor nests copies until the path limit.
#[cfg(unix)]
#[test]
fn copying_a_tree_keeps_a_directory_link_as_a_link() -> TestResult {
    let dir = tree(&[("src/a.rs", b"a")]).ok_or("a scratch tree")?;
    std::os::unix::fs::symlink(dir.path(), dir.path().join("src/up"))?;

    let (_, result) = mutate(PathMutation::Copy {
        from: dir.path().join("src"),
        to: dir.path().join("copy"),
    })
    .ok_or("a mutation answer")?;

    assert_eq!(result, Ok(()));
    let link = dir.path().join("copy/up");
    assert!(link.symlink_metadata()?.is_symlink());
    assert_eq!(std::fs::read_link(&link)?, dir.path());
    Ok(())
}

/// A hard link shares its source's inode but is another name: renaming onto it
/// is a clash. `fs::rename` between two links of one file succeeds by doing
/// nothing, which would leave the source in place while reporting a move.
#[cfg(unix)]
#[test]
fn renaming_onto_a_hard_link_of_the_source_refuses() -> TestResult {
    let dir = tree(&[("a.rs", b"a")]).ok_or("a scratch tree")?;
    std::fs::hard_link(dir.path().join("a.rs"), dir.path().join("b.rs"))?;

    let (_, result) = mutate(PathMutation::Rename {
        from: dir.path().join("a.rs"),
        to: dir.path().join("b.rs"),
    })
    .ok_or("a mutation answer")?;

    assert!(result.is_err());
    assert!(dir.path().join("a.rs").exists());
    Ok(())
}

/// What a case-insensitive filesystem presents for a case-only rename — one
/// inode under two spellings — is one entry. A hard link stands in for it on a
/// case-sensitive filesystem, which is what lets this run everywhere.
#[cfg(unix)]
#[test]
fn one_inode_under_a_case_only_spelling_is_the_same_entry() -> TestResult {
    let dir = tree(&[("readme.md", b"text")]).ok_or("a scratch tree")?;
    let (lower, upper) = (dir.path().join("readme.md"), dir.path().join("README.md"));
    if upper.symlink_metadata().is_err() {
        std::fs::hard_link(&lower, &upper)?;
    }
    std::fs::hard_link(&lower, dir.path().join("other.md"))?;

    assert!(super::mutate::same_entry(&lower, &upper));
    assert!(!super::mutate::same_entry(
        &lower,
        &dir.path().join("other.md")
    ));
    Ok(())
}

/// A FIFO in the workspace, made with the platform's `mkfifo`; `None` where
/// there is none to make one with.
#[cfg(unix)]
fn fifo(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    let path = dir.join(name);
    let made = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .ok()?;
    made.success().then_some(path)
}

/// Opening a FIFO blocks until a writer appears, which would hold the one
/// serial worker — and every listing and mutation queued behind it — for ever.
/// Classifying, reading and copying one each refuse instead.
#[cfg(unix)]
#[test]
fn a_fifo_is_refused_rather_than_opened() -> TestResult {
    let dir = tree(&[("src/a.rs", b"a")]).ok_or("a scratch tree")?;
    let Some(pipe) = fifo(&dir.path().join("src"), "pipe") else {
        return Ok(()); // no `mkfifo` here: nothing to provoke
    };

    let classified = classify_path(&pipe, false).ok_or("a classification answer")?;
    let read = read_bytes(&pipe, 0, 16).ok_or("a read answer")?;
    let (_, copied) = mutate(PathMutation::Copy {
        from: dir.path().join("src"),
        to: dir.path().join("copy"),
    })
    .ok_or("a mutation answer")?;

    assert!(classified.is_err());
    assert!(read.is_err());
    assert!(copied.is_err());
    Ok(())
}
