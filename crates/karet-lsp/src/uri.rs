//! `file://` URI ↔ [`Path`] conversion.
//!
//! LSP identifies documents by URI; karet identifies them by path. Paths are
//! percent-encoded per RFC 3986 (everything but unreserved characters and `/`),
//! and incoming URIs are decoded back. Only absolute paths convert — a relative
//! path has no well-formed `file://` form — and only `file`-scheme URIs map back.

use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

use lsp_types::Uri;

use crate::LspError;

/// Convert an absolute `path` to a `file://` URI.
///
/// # Errors
/// Returns [`LspError::Protocol`] when the path is relative, is not valid UTF-8,
/// or fails to parse as a URI after encoding.
pub(crate) fn path_to_uri(path: &Path) -> Result<Uri, LspError> {
    if !path.is_absolute() {
        return Err(LspError::Protocol(format!(
            "cannot convert relative path {} to a file URI",
            path.display()
        )));
    }
    let Some(s) = path.to_str() else {
        return Err(LspError::Protocol(format!(
            "cannot convert non-UTF-8 path {} to a file URI",
            path.display()
        )));
    };
    let mut out = String::with_capacity(s.len() + 8);
    out.push_str("file://");
    #[cfg(windows)]
    let s = &{
        // `C:\x` → `/C:/x` so the URI path component starts with a slash.
        let mut n = s.replace('\\', "/");
        if !n.starts_with('/') {
            n.insert(0, '/');
        }
        n
    };
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(char::from(byte));
            },
            _ => {
                out.push_str(&format!("%{byte:02X}"));
            },
        }
    }
    Uri::from_str(&out).map_err(|e| LspError::Protocol(format!("invalid file URI {out:?}: {e}")))
}

/// Convert a `file://` URI back to a path, or `None` for other schemes or
/// undecodable paths.
pub(crate) fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let scheme = uri.scheme()?;
    if !scheme.as_str().eq_ignore_ascii_case("file") {
        return None;
    }
    let decoded = uri
        .path()
        .as_estr()
        .decode()
        .into_string()
        .ok()?
        .into_owned();
    #[cfg(windows)]
    let decoded = {
        // `/C:/x` → `C:/x`.
        let mut d = decoded;
        if d.len() >= 3 && d.starts_with('/') && d.as_bytes()[2] == b':' {
            d.remove(0);
        }
        d
    };
    Some(PathBuf::from(decoded))
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

    #[test]
    fn roundtrips_a_plain_path() -> TestResult {
        let uri = path_to_uri(Path::new("/home/user/src/main.rs"))?;
        assert_eq!(uri.as_str(), "file:///home/user/src/main.rs");
        assert_eq!(
            uri_to_path(&uri),
            Some(PathBuf::from("/home/user/src/main.rs"))
        );
        Ok(())
    }

    #[test]
    fn percent_encodes_spaces_and_unicode() -> TestResult {
        let uri = path_to_uri(Path::new("/tmp/my project/naïve file.rs"))?;
        assert_eq!(
            uri.as_str(),
            "file:///tmp/my%20project/na%C3%AFve%20file.rs"
        );
        assert_eq!(
            uri_to_path(&uri),
            Some(PathBuf::from("/tmp/my project/naïve file.rs"))
        );
        Ok(())
    }

    #[test]
    fn rejects_relative_paths() {
        assert!(matches!(
            path_to_uri(Path::new("relative/main.rs")),
            Err(LspError::Protocol(_))
        ));
    }

    #[test]
    fn non_file_schemes_do_not_map_back() -> TestResult {
        let uri = Uri::from_str("untitled:Untitled-1")?;
        assert_eq!(uri_to_path(&uri), None);
        let https = Uri::from_str("https://example.com/x.rs")?;
        assert_eq!(uri_to_path(&https), None);
        Ok(())
    }

    #[test]
    fn decodes_server_style_uris() -> TestResult {
        let uri = Uri::from_str("file:///var/x%2By/a%20b.rs")?;
        assert_eq!(uri_to_path(&uri), Some(PathBuf::from("/var/x+y/a b.rs")));
        Ok(())
    }
}
