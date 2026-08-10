//! Buffer blame on the VCS worker: per-version cached line attribution via
//! `blameline`, mapped onto the current buffer's line positions.

use std::collections::HashMap;
use std::path::PathBuf;

use karet_core::BlameAttribution;

use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) struct BlameCacheKey {
    doc: DocumentId,
    version: u64,
    path: PathBuf,
    head: String,
}

pub(super) type BlameCache = HashMap<BlameCacheKey, Vec<BlameAttribution>>;

pub(super) fn blame(
    cache: &mut BlameCache,
    root: &Option<PathBuf>,
    doc: DocumentId,
    version: u64,
    path: &Path,
    text: &str,
    line: u32,
) -> Result<Option<BlameAttribution>, String> {
    let Some(root) = root.as_ref() else {
        return Ok(None);
    };
    let repo = match Repository::discover(root) {
        Ok(repo) => repo,
        Err(VcsError::NotARepository) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let Some(head_hash) = repo.head_hash().map_err(|error| error.to_string())? else {
        return Ok(None);
    };
    let key = BlameCacheKey {
        doc,
        version,
        path: path.to_path_buf(),
        head: head_hash,
    };
    if let Some(attribution) = cache.get(&key) {
        return Ok(attribution.get(line as usize).cloned());
    }
    let Some(head) = repo
        .file_at_rev(path, "HEAD")
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let Ok(head) = String::from_utf8(head) else {
        return Ok(None);
    };
    let groups = match blameline::blame_file(root, path) {
        Ok(groups) => groups,
        Err(blameline::BlameError::NotARepository | blameline::BlameError::NotCommitted(_)) => {
            return Ok(None);
        },
        Err(error) => return Err(error.to_string()),
    };
    let current_lines: Vec<&str> = text.lines().collect();
    let head_lines: Vec<&str> = head.lines().collect();
    let attribution = map_attribution(&current_lines, &head_lines, &groups);
    let result = attribution.get(line as usize).cloned();
    // Cursor movement reuses this full-file mapping. Keep only the newest version
    // for a document so typing cannot grow the worker cache without bound.
    cache.retain(|cached, _| cached.doc != doc);
    cache.insert(key, attribution);
    Ok(result)
}

pub(super) fn map_attribution(
    current: &[&str],
    head: &[&str],
    groups: &[blameline::BlameGroup],
) -> Vec<BlameAttribution> {
    let mut positions: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, content) in head.iter().enumerate() {
        positions.entry(content).or_default().push(index);
    }
    let mut by_head = vec![BlameAttribution::Uncommitted; head.len()];
    for group in groups {
        let Some(author_time) = group.author_time() else {
            continue;
        };
        let commit = BlameCommit {
            hash: group.commit_hash.clone(),
            author: group.author.clone(),
            author_time,
        };
        let start = group.lines.start.saturating_sub(1) as usize;
        let end = (group.lines.end as usize).min(by_head.len());
        for item in by_head.iter_mut().take(end).skip(start) {
            *item = BlameAttribution::Commit(commit.clone());
        }
    }
    current
        .iter()
        .enumerate()
        .map(|(index, content)| {
            if head.get(index) == Some(content) {
                return by_head
                    .get(index)
                    .cloned()
                    .unwrap_or(BlameAttribution::Uncommitted);
            }
            match positions.get(content).map(Vec::as_slice) {
                Some([unique]) => by_head
                    .get(*unique)
                    .cloned()
                    .unwrap_or(BlameAttribution::Uncommitted),
                _ => BlameAttribution::Uncommitted,
            }
        })
        .collect()
}
