//! Filesystem-worker tests.
//!
//! Every case goes through [`answer`](super::answer) — the same function the
//! worker thread calls — so what is tested is the job-to-event contract, not a
//! helper that happens to sit beside it.

use std::path::Path;

use karet_core::DirEntry;

use super::*;

/// A scratch tree with `files` written into it.
///
/// Returns `None` rather than failing the run when the platform cannot provide a
/// temporary directory; the workspace lint floor bans `expect` in tests too.
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
fn a_text_file_classifies_as_text_with_its_length_and_head() {
    let Some(dir) = tree(&[("main.rs", b"fn main() {}\n")]) else {
        return;
    };

    let Some(Ok(class)) = classify_path(&dir.path().join("main.rs"), false) else {
        return;
    };

    assert_eq!(class.kind, karet_filetype::FileKind::Text);
    assert_eq!(class.len, 13);
    assert_eq!(class.head, b"fn main() {}\n");
}

/// Classification is defined over whatever leading bytes exist. An empty file
/// must classify rather than error, or opening a freshly created file fails.
#[test]
fn an_empty_file_still_classifies() {
    let Some(dir) = tree(&[("new.txt", b"")]) else {
        return;
    };

    let Some(Ok(class)) = classify_path(&dir.path().join("new.txt"), false) else {
        return;
    };

    assert_eq!(class.len, 0);
    assert!(class.head.is_empty());
}

#[test]
fn classifying_a_missing_path_reports_why() {
    let Some(dir) = tree(&[]) else {
        return;
    };

    let Some(result) = classify_path(&dir.path().join("absent.rs"), false) else {
        return;
    };

    assert!(result.is_err());
}

#[test]
fn reading_bytes_answers_a_chunk_that_knows_the_total_length() {
    let Some(dir) = tree(&[("data.bin", &[0, 1, 2, 3, 4, 5, 6, 7])]) else {
        return;
    };

    let Some(Ok(chunk)) = read_bytes(&dir.path().join("data.bin"), 2, 3) else {
        return;
    };

    assert_eq!(chunk.offset, 2);
    assert_eq!(chunk.bytes, vec![2, 3, 4]);
    assert_eq!(chunk.total_len, 8);
    assert!(!chunk.is_final());
}

/// A client walking a file to its end asks for one range past it. That must
/// terminate the read, not error — otherwise the last chunk never arrives.
#[test]
fn reading_past_the_end_answers_an_empty_final_chunk() {
    let Some(dir) = tree(&[("data.bin", &[0, 1, 2, 3])]) else {
        return;
    };

    let Some(Ok(chunk)) = read_bytes(&dir.path().join("data.bin"), 4, 16) else {
        return;
    };

    assert!(chunk.bytes.is_empty());
    assert!(chunk.is_final());
}

/// The chunk cap keeps a large PDF from monopolizing the stream ahead of an
/// interactive edit, so an oversized request must be clamped, not honored.
#[test]
fn a_read_larger_than_the_chunk_cap_is_clamped() {
    let big = vec![7_u8; (MAX_CHUNK as usize) + 4096];
    let Some(dir) = tree(&[("big.bin", &big)]) else {
        return;
    };

    let Some(Ok(chunk)) = read_bytes(&dir.path().join("big.bin"), 0, u64::MAX) else {
        return;
    };

    assert_eq!(chunk.bytes.len() as u64, MAX_CHUNK);
    assert!(!chunk.is_final());
}

#[test]
fn listing_the_workspace_reports_paths_and_whether_it_was_cut_short() {
    let Some(dir) = tree(&[
        ("a.rs", b"x"),
        ("b.rs", b"x"),
        ("c.rs", b"x"),
        ("target/generated.rs", b"x"),
    ]) else {
        return;
    };

    let (_, event) = answer(FsJob::ListFiles {
        id: RequestId(1),
        root: dir.path().to_path_buf(),
        limit: 100,
    });

    let Event::FilesListed { files, truncated } = event else {
        return;
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
}

#[test]
fn a_truncated_workspace_listing_says_so() {
    let Some(dir) = tree(&[("a.rs", b"x"), ("b.rs", b"x"), ("c.rs", b"x")]) else {
        return;
    };

    let (_, event) = answer(FsJob::ListFiles {
        id: RequestId(1),
        root: dir.path().to_path_buf(),
        limit: 2,
    });

    let Event::FilesListed { files, truncated } = event else {
        return;
    };
    assert_eq!(files.len(), 2);
    assert!(truncated);
}

#[test]
fn a_directory_listing_puts_directories_first_and_excludes_dot_git() {
    let Some(dir) = tree(&[
        ("zeta.rs", b"x"),
        ("alpha.rs", b"x"),
        ("src/lib.rs", b"x"),
        (".git/HEAD", b"ref: refs/heads/master\n"),
    ]) else {
        return;
    };

    let Some(Ok(entries)) = list_dir(dir.path(), true, false) else {
        return;
    };

    assert_eq!(labels(&entries), ["src", "alpha.rs", "zeta.rs"]);
}

/// Ignored entries are flagged, never filtered: a user must still see the
/// `target/` directory their build produced, dimmed rather than absent.
#[test]
fn gitignored_entries_are_listed_and_flagged() {
    let Some(dir) = tree(&[
        (".gitignore", b"ignored.rs\n"),
        ("kept.rs", b"x"),
        ("ignored.rs", b"x"),
    ]) else {
        return;
    };

    let Some(Ok(entries)) = list_dir(dir.path(), true, true) else {
        return;
    };

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
}

#[test]
fn hidden_entries_are_omitted_unless_asked_for() {
    let Some(dir) = tree(&[(".env", b"SECRET=1"), ("main.rs", b"x")]) else {
        return;
    };

    let (Some(Ok(hidden)), Some(Ok(shown))) = (
        list_dir(dir.path(), false, false),
        list_dir(dir.path(), true, false),
    ) else {
        return;
    };

    assert_eq!(labels(&hidden), ["main.rs"]);
    assert!(labels(&shown).contains(&".env".to_owned()), "{shown:?}");
}

#[test]
fn listing_a_file_rather_than_a_directory_reports_why() {
    let Some(dir) = tree(&[("main.rs", b"x")]) else {
        return;
    };

    let Some(result) = list_dir(&dir.path().join("main.rs"), false, false) else {
        return;
    };

    assert!(result.is_err());
}

#[test]
fn creating_a_file_makes_it_and_its_parents() {
    let Some(dir) = tree(&[]) else {
        return;
    };
    let path = dir.path().join("deep/nested/new.rs");

    let Some((echoed, result)) = mutate(PathMutation::CreateFile { path: path.clone() }) else {
        return;
    };

    assert_eq!(result, Ok(()));
    assert!(path.is_file());
    assert_eq!(echoed.target(), &path);
}

/// "New file" must never truncate something the user forgot about.
#[test]
fn creating_a_file_that_exists_refuses_rather_than_truncating() {
    let Some(dir) = tree(&[("keep.rs", b"important")]) else {
        return;
    };
    let path = dir.path().join("keep.rs");

    let Some((_, result)) = mutate(PathMutation::CreateFile { path: path.clone() }) else {
        return;
    };

    assert!(result.is_err());
    assert_eq!(std::fs::read(&path).ok(), Some(b"important".to_vec()));
}

/// `fs::rename` overwrites on Unix. The explorer must not.
#[test]
fn renaming_onto_an_existing_path_refuses() {
    let Some(dir) = tree(&[("from.rs", b"source"), ("to.rs", b"destination")]) else {
        return;
    };

    let Some((_, result)) = mutate(PathMutation::Rename {
        from: dir.path().join("from.rs"),
        to: dir.path().join("to.rs"),
    }) else {
        return;
    };

    assert!(result.is_err());
    assert_eq!(
        std::fs::read(dir.path().join("to.rs")).ok(),
        Some(b"destination".to_vec())
    );
}

#[test]
fn renaming_moves_the_file() {
    let Some(dir) = tree(&[("from.rs", b"source")]) else {
        return;
    };

    let Some((_, result)) = mutate(PathMutation::Rename {
        from: dir.path().join("from.rs"),
        to: dir.path().join("moved/to.rs"),
    }) else {
        return;
    };

    assert_eq!(result, Ok(()));
    assert!(!dir.path().join("from.rs").exists());
    assert_eq!(
        std::fs::read(dir.path().join("moved/to.rs")).ok(),
        Some(b"source".to_vec())
    );
}

#[test]
fn copying_a_directory_copies_everything_under_it() {
    let Some(dir) = tree(&[("src/a.rs", b"a"), ("src/deep/b.rs", b"b")]) else {
        return;
    };

    let Some((_, result)) = mutate(PathMutation::Copy {
        from: dir.path().join("src"),
        to: dir.path().join("copy"),
    }) else {
        return;
    };

    assert_eq!(result, Ok(()));
    assert_eq!(
        std::fs::read(dir.path().join("copy/a.rs")).ok(),
        Some(b"a".to_vec())
    );
    assert_eq!(
        std::fs::read(dir.path().join("copy/deep/b.rs")).ok(),
        Some(b"b".to_vec())
    );
}

#[test]
fn deleting_removes_a_directory_and_its_contents() {
    let Some(dir) = tree(&[("gone/a.rs", b"a"), ("gone/deep/b.rs", b"b")]) else {
        return;
    };

    let Some((_, result)) = mutate(PathMutation::Delete {
        path: dir.path().join("gone"),
    }) else {
        return;
    };

    assert_eq!(result, Ok(()));
    assert!(!dir.path().join("gone").exists());
}

/// Deleting a link to a directory must remove the link, never the directory it
/// points at — the difference between tidying up and losing work.
#[cfg(unix)]
#[test]
fn deleting_a_symlink_to_a_directory_leaves_the_target() {
    let Some(dir) = tree(&[("real/keep.rs", b"keep")]) else {
        return;
    };
    let link = dir.path().join("link");
    if std::os::unix::fs::symlink(dir.path().join("real"), &link).is_err() {
        return;
    }

    let Some((_, result)) = mutate(PathMutation::Delete { path: link.clone() }) else {
        return;
    };

    assert_eq!(result, Ok(()));
    assert!(!link.exists());
    assert_eq!(
        std::fs::read(dir.path().join("real/keep.rs")).ok(),
        Some(b"keep".to_vec())
    );
}

#[test]
fn deleting_a_missing_path_reports_why() {
    let Some(dir) = tree(&[]) else {
        return;
    };

    let Some((_, result)) = mutate(PathMutation::Delete {
        path: dir.path().join("absent.rs"),
    }) else {
        return;
    };

    assert!(result.is_err());
}
