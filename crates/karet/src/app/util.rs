use std::io;
use std::path::Path;
use std::path::PathBuf;

use crossterm::event::PopKeyboardEnhancementFlags;
use karet_filetype::WrapMode;
use karet_filetype::file_type_for_path;
use karet_theme::Theme;
use ratatui::layout::Rect;

use crate::app::hit::TabHit;
use crate::app::state::CloseRequest;
use crate::tab::Tab;
use crate::tab::TabKind;

/// Whether the screen point `(x, y)` lies inside `r`.
pub(super) fn rect_contains(r: Rect, (x, y): (u16, u16)) -> bool {
    x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

/// The unsaved-changes confirmation's title and its two action verbs, phrased for
/// the scope `request` closes and its `count` at-risk files.
pub(super) fn close_prompt_choices(
    request: CloseRequest,
    count: usize,
) -> (String, &'static str, &'static str) {
    let files = if count == 1 { "file" } else { "files" };
    if matches!(request, CloseRequest::Quit) {
        (
            format!("Quit with {count} unsaved {files}?"),
            "Save all and quit",
            "Discard and quit",
        )
    } else {
        (
            format!("Close with {count} unsaved {files}?"),
            "Save and close",
            "Discard and close",
        )
    }
}

/// Whether screen row `y` lies within `r`'s vertical span (column ignored).
pub(super) fn row_in_rect(r: Rect, y: u16) -> bool {
    r.height > 0 && y >= r.y && y < r.bottom()
}

/// The tab at column `x` among `hits`, and whether `x` is on its close glyph.
pub(super) fn tab_at(hits: &[TabHit], x: u16) -> Option<(usize, bool)> {
    hits.iter()
        .enumerate()
        .find_map(|(i, h)| (x >= h.start && x < h.end).then_some((i, x == h.close)))
}

/// A non-empty language selector for resolving editor configuration.
pub(crate) fn tab_language(tab: &Tab) -> Option<&str> {
    let language = tab.language();
    (!language.is_empty()).then_some(language)
}

/// Whether `path` names a Markdown file, by the file-type registry's name for it.
pub(crate) fn is_markdown(path: &Path) -> bool {
    file_type_for_path(path).name() == "Markdown"
}

/// Resolve a code tab's long-line behavior from its configured override or file type.
pub(crate) fn effective_word_wrap(tab: &Tab, override_: Option<bool>) -> bool {
    override_.unwrap_or_else(|| {
        matches!(
            &tab.kind,
            TabKind::Code { path, .. }
                if file_type_for_path(path).wrap_mode() == WrapMode::Wrap
        )
    })
}

/// Recursively copy a file or directory tree.
pub(super) fn copy_path_recursive(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_path_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, to).map(|_| ())
    }
}

/// Move a file or directory, falling back to copy-then-delete for cross-device moves.
pub(super) fn move_path(from: &Path, to: &Path) -> io::Result<()> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(rename_err) => {
            copy_path_recursive(from, to)?;
            let remove = if from.is_dir() {
                std::fs::remove_dir_all(from)
            } else {
                std::fs::remove_file(from)
            };
            remove.map_err(|_| rename_err)
        },
    }
}

/// Whether two paths resolve to the same filesystem location.
pub(super) fn same_path(a: &Path, b: &Path) -> bool {
    canonical(a) == canonical(b)
}

/// Whether `path` resolves to `root` or a path below it.
pub(super) fn path_under(root: &Path, path: &Path) -> bool {
    canonical(path).starts_with(canonical(root))
}

pub(super) fn rebase_path(path: &Path, from: &Path, to: &Path) -> Option<PathBuf> {
    if !path_under(from, path) {
        return None;
    }
    let suffix = path.strip_prefix(from).ok()?;
    Some(to.join(suffix))
}

pub(super) fn retarget_tab_path(tab: &mut Tab, path: &Path) {
    let target = match &mut tab.kind {
        TabKind::Code { path: p, .. }
        | TabKind::Hex { path: p, .. }
        | TabKind::Placeholder { path: p, .. } => Some(p),
        #[cfg(feature = "images")]
        TabKind::Image { path: p, .. } => Some(p),
        #[cfg(feature = "pdf")]
        TabKind::Document { path: p, .. } => Some(p),
        _ => None,
    };
    if let Some(p) = target {
        *p = path.to_path_buf();
        tab.is_symlink =
            std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink());
        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            tab.title = name.to_string();
        }
    }
}

/// A destination path under `dir`, suffixing when the source name already exists.
pub(super) fn unique_child_path(dir: &Path, source: &Path) -> PathBuf {
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "item".to_string());
    let first = dir.join(&name);
    if !first.exists() {
        return first;
    }

    for n in 1usize.. {
        let candidate = dir.join(copy_name(source, &name, n));
        if !candidate.exists() {
            return candidate;
        }
    }

    unreachable!("unbounded suffix search should always return");
}

/// Build `name copy.ext`, `name copy 2.ext`, or `dir copy` style conflict names.
fn copy_name(source: &Path, fallback: &str, n: usize) -> String {
    let suffix = if n == 1 {
        " copy".to_string()
    } else {
        format!(" copy {n}")
    };
    if source.is_dir() {
        return format!("{fallback}{suffix}");
    }
    let stem = source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| fallback.to_string());
    match source.extension().map(|ext| ext.to_string_lossy()) {
        Some(ext) if !ext.is_empty() => format!("{stem}{suffix}.{ext}"),
        _ => format!("{stem}{suffix}"),
    }
}

/// The canonical form of `path` for tab de-duplication; see
/// [`canonical_nearest`](crate::paths::canonical_nearest).
pub(super) use crate::paths::canonical_nearest as canonical;

/// Parse a revision-range spec typed into the go-to-commit input into
/// `(base, head, merge_base)`, or `None` when it is a single revision.
///
/// A three-dot `a...b` selects the merge-base range; a two-dot `a..b` the raw tips. An
/// omitted side defaults to `HEAD` (matching git: `..b`, `a..`). Whitespace is trimmed.
pub(super) fn parse_rev_range(input: &str) -> Option<(String, String, bool)> {
    // Three-dot first: "..." also contains "..".
    let (sep, merge_base) = if input.contains("...") {
        ("...", true)
    } else if input.contains("..") {
        ("..", false)
    } else {
        return None;
    };
    let (base, head) = input.split_once(sep)?;
    let side = |s: &str| {
        let s = s.trim();
        if s.is_empty() { "HEAD" } else { s }.to_string()
    };
    Some((side(base), side(head), merge_base))
}

/// Pops the kitty keyboard-enhancement flags on drop, so they are cleared even if
/// the event loop panics (ratatui's panic hook restores the rest of the terminal).
pub(super) struct KeyboardEnhancementGuard;

impl Drop for KeyboardEnhancementGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
}

/// Resolve a `workbench.colorTheme` setting to a [`Theme`]: the built-in `"dark"`
/// (also the empty string), or a path to a VS Code `.json` theme file.
/// Returns a human-readable message on a read/parse failure so the caller can warn
/// and fall back to the default.
pub(super) fn load_theme(name: &str) -> Result<Theme, String> {
    if name.is_empty() || name == "dark" {
        return Ok(Theme::dark());
    }
    let path = Path::new(name);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "json" {
        return Err(format!(
            "theme `{name}`: only the built-in `dark` theme and VS Code JSON theme files are supported"
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("theme `{name}`: {e}"))?;
    let text = String::from_utf8(bytes).map_err(|e| format!("theme `{name}`: {e}"))?;
    Theme::load_vscode(&text).map_err(|e| format!("theme `{name}`: {e}"))
}

/// Build the lane layout for `commits` (newest first). Lane assignment is sequential
/// from the tip, so callers cache this and recompute only when the commit list changes.
pub(crate) fn commit_rails(commits: &[karet_vcs::Commit]) -> Vec<karet_graph::RailRow> {
    let inputs: Vec<karet_graph::LaneInput> = commits
        .iter()
        .enumerate()
        .map(|(i, commit)| karet_graph::LaneInput {
            id: commit.hash.clone(),
            parents: commit.parents.clone(),
            head: i == 0,
        })
        .collect();
    karet_graph::assign_lanes(&inputs)
}

/// Scroll `offset` the minimum distance that brings row `cursor` inside a viewport of
/// `height` rows. The three commit lists and the change list all need exactly this.
#[must_use]
pub(crate) fn keep_visible(cursor: usize, offset: usize, height: usize) -> usize {
    if cursor < offset {
        cursor
    } else if height > 0 && cursor >= offset + height {
        cursor + 1 - height
    } else {
        offset
    }
}

/// A terse `git log`-style relative time (e.g. `3d ago`) for a Unix timestamp.
pub(crate) fn relative_time(secs: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0);
    relative_time_at(secs, now)
}

/// [`relative_time`] as seen at `now` (seconds since the Unix epoch).
fn relative_time_at(secs: i64, now: i64) -> String {
    let delta = now - secs;
    if delta < 0 {
        return "just now".to_string();
    }
    let (n, unit) = if delta < 60 {
        (delta, "s")
    } else if delta < 3600 {
        (delta / 60, "m")
    } else if delta < 86_400 {
        (delta / 3600, "h")
    } else if delta < 86_400 * 7 {
        (delta / 86_400, "d")
    } else if delta < 86_400 * 30 {
        (delta / (86_400 * 7), "w")
    } else if delta < 86_400 * 365 {
        (delta / (86_400 * 30), "mo")
    } else {
        (delta / (86_400 * 365), "y")
    };
    format!("{n}{unit} ago")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clamp only moves the viewport when the cursor is actually outside it, and
    /// then by the minimum distance.
    #[test]
    fn keep_visible_scrolls_the_minimum_distance() {
        // Already inside: unchanged.
        assert_eq!(keep_visible(5, 3, 10), 3);
        // Above the viewport: the cursor becomes the top row.
        assert_eq!(keep_visible(1, 4, 10), 1);
        // Below the viewport: the cursor becomes the bottom row.
        assert_eq!(keep_visible(20, 4, 10), 11);
        // A zero-height viewport can't scroll anywhere.
        assert_eq!(keep_visible(20, 4, 0), 4);
    }

    #[test]
    fn relative_time_uses_compact_git_style_units() {
        let now = 40_000_000;
        assert_eq!(relative_time_at(now + 1, now), "just now");
        assert_eq!(relative_time_at(now - 42, now), "42s ago");
        assert_eq!(relative_time_at(now - 120, now), "2m ago");
        assert_eq!(relative_time_at(now - 10_800, now), "3h ago");
        assert_eq!(relative_time_at(now - 172_800, now), "2d ago");
        assert_eq!(relative_time_at(now - 1_209_600, now), "2w ago");
        assert_eq!(relative_time_at(now - 5_184_000, now), "2mo ago");
        assert_eq!(relative_time_at(now - 63_072_000, now), "2y ago");
    }
}
