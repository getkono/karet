//! Trust-aware Markdown link resolution and desktop activation.

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use thiserror::Error;
use url::Url;

/// A validated link target with its workspace boundary made explicit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LinkTarget {
    /// A web or mail link that may be delegated to the platform opener.
    ExternalUrl(String),
    /// A file contained by the workspace, including a terminal-safe file URI.
    WorkspaceFile { path: PathBuf, uri: String },
    /// A relative file link that resolves beyond the workspace boundary.
    OutsideWorkspaceFile(PathBuf),
}

impl LinkTarget {
    /// The URI safe to expose through OSC 8, when activation does not require a
    /// workspace-boundary confirmation.
    pub(crate) fn osc8_uri(&self) -> Option<&str> {
        match self {
            Self::ExternalUrl(url) => Some(url),
            Self::WorkspaceFile { uri, .. } => Some(uri),
            Self::OutsideWorkspaceFile(_) => None,
        }
    }
}

/// Why a Markdown target cannot be activated.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub(crate) enum LinkError {
    /// The target contains bytes that could inject a terminal control sequence.
    #[error("link target contains control characters")]
    ControlCharacter,
    /// Only explicitly safe schemes are accepted.
    #[error("unsupported link scheme `{0}`")]
    UnsupportedScheme(String),
    /// The URL is malformed or lacks required addressing information.
    #[error("invalid link target")]
    Invalid,
    /// Markdown file targets must be relative to their source document.
    #[error("file links must be relative to the Markdown document")]
    AbsoluteFile,
}

/// Validate `raw` and resolve relative file targets against `source`.
pub(crate) fn resolve(raw: &str, source: &Path, root: &Path) -> Result<LinkTarget, LinkError> {
    if raw.chars().any(char::is_control) {
        return Err(LinkError::ControlCharacter);
    }
    if let Some(scheme) = explicit_scheme(raw) {
        let url = Url::parse(raw).map_err(|_| LinkError::Invalid)?;
        return match scheme.as_str() {
            "http" | "https" if url.host_str().is_some() => Ok(LinkTarget::ExternalUrl(url.into())),
            "mailto" if !url.path().is_empty() => Ok(LinkTarget::ExternalUrl(url.into())),
            "http" | "https" | "mailto" => Err(LinkError::Invalid),
            _ => Err(LinkError::UnsupportedScheme(scheme)),
        };
    }

    let path_part = raw.split(['?', '#']).next().unwrap_or_default();
    if Path::new(path_part).is_absolute() {
        return Err(LinkError::AbsoluteFile);
    }
    let root = absolute(root).map_err(|_| LinkError::Invalid)?;
    let source = if source.is_absolute() {
        source.to_path_buf()
    } else {
        root.join(source)
    };
    let base = Url::from_file_path(&source).map_err(|()| LinkError::Invalid)?;
    let joined = base.join(raw).map_err(|_| LinkError::Invalid)?;
    if joined.scheme() != "file"
        || joined.host_str().is_some()
        || joined.username() != ""
        || joined.password().is_some()
        || joined.query().is_some()
    {
        return Err(LinkError::Invalid);
    }
    let path = joined.to_file_path().map_err(|()| LinkError::Invalid)?;
    let resolved_root = canonical_nearest(&root);
    let resolved_path = canonical_nearest(&path);
    if !resolved_path.starts_with(&resolved_root) {
        return Ok(LinkTarget::OutsideWorkspaceFile(resolved_path));
    }

    let mut uri = Url::from_file_path(&resolved_path).map_err(|()| LinkError::Invalid)?;
    uri.set_fragment(joined.fragment());
    Ok(LinkTarget::WorkspaceFile {
        path: resolved_path,
        uri: uri.into(),
    })
}

/// Ask the platform desktop to activate a validated URL.
pub(crate) fn open_external(target: &str) -> std::io::Result<()> {
    let (program, args) = opener(target);
    Command::new(program).args(args).spawn().map(|_| ())
}

#[cfg(target_os = "macos")]
fn opener(target: &str) -> (&'static str, Vec<&str>) {
    ("open", vec![target])
}

#[cfg(target_os = "windows")]
fn opener(target: &str) -> (&'static str, Vec<&str>) {
    ("explorer.exe", vec![target])
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn opener(target: &str) -> (&'static str, Vec<&str>) {
    ("xdg-open", vec![target])
}

fn explicit_scheme(raw: &str) -> Option<String> {
    let colon = raw.find(':')?;
    let candidate = raw.get(..colon)?;
    let mut chars = candidate.chars();
    let first = chars.next()?;
    (first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
    .then(|| candidate.to_ascii_lowercase())
}

fn absolute(path: &Path) -> std::io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir().map(|cwd| cwd.join(path))
    }
}

/// Resolve the nearest existing ancestor so symlinks cannot disguise an escape.
fn canonical_nearest(path: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(path) {
        return resolved;
    }
    for ancestor in path.ancestors().skip(1) {
        let Ok(resolved) = std::fs::canonicalize(ancestor) else {
            continue;
        };
        let Ok(suffix) = path.strip_prefix(ancestor) else {
            continue;
        };
        return resolved.join(suffix);
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_safe_external_schemes() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("README.md");
        assert!(matches!(
            resolve("https://example.com/a", &source, root.path())?,
            LinkTarget::ExternalUrl(_)
        ));
        assert!(matches!(
            resolve("mailto:team@example.com", &source, root.path())?,
            LinkTarget::ExternalUrl(_)
        ));
        assert_eq!(
            resolve("javascript:alert(1)", &source, root.path()),
            Err(LinkError::UnsupportedScheme("javascript".to_string()))
        );
        assert_eq!(
            resolve("https://example.com/\u{1b}]8;;bad", &source, root.path()),
            Err(LinkError::ControlCharacter)
        );
        Ok(())
    }

    #[test]
    fn resolves_files_and_marks_workspace_escapes() -> Result<(), Box<dyn std::error::Error>> {
        let parent = tempfile::tempdir()?;
        let root = parent.path().join("workspace");
        std::fs::create_dir_all(root.join("docs"))?;
        let source = root.join("README.md");
        std::fs::write(root.join("docs/guide.md"), "guide")?;

        let inside = resolve("docs/guide.md#start", &source, &root)?;
        let LinkTarget::WorkspaceFile { path, uri } = inside else {
            return Err("expected a workspace file".into());
        };
        assert_eq!(path, std::fs::canonicalize(root.join("docs/guide.md"))?);
        assert!(uri.ends_with("docs/guide.md#start"));

        assert!(matches!(
            resolve("../outside.md", &source, &root)?,
            LinkTarget::OutsideWorkspaceFile(_)
        ));
        assert_eq!(
            resolve("/etc/passwd", &source, &root),
            Err(LinkError::AbsoluteFile)
        );
        Ok(())
    }

    #[test]
    fn platform_opener_passes_the_target_as_one_argument() {
        let target = "https://example.com/a?x=$(touch nope)&y=two words";
        let (_, args) = opener(target);
        assert_eq!(args, vec![target]);
    }
}
