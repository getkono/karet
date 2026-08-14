//! Decoding the paths git writes into diff headers.
//!
//! Git quotes a path when it contains unusual bytes, escaping them as `\NNN`
//! octal or as one of the short `\\`, `\"`, `\t`, `\n`, `\r` forms. Everything
//! here reverses that, and never loses a filename: undecodable bytes fall back to
//! lossy UTF-8 rather than dropping the path.

/// Given `a/<old> b/<new>`, split into `(old, new)` stripping the `a/` / `b/`
/// prefixes. Handles bare, both-quoted, and `\NNN`-octal-escaped header shapes.
pub(super) fn split_ab_paths(s: &str) -> (String, String) {
    if s.starts_with("\"a/")
        && let Some((old, new)) = split_quoted_ab(s)
    {
        return (old, new);
    }
    if let Some(pos) = find_b_split(s) {
        let old = s[2..pos].to_string(); // skip "a/"
        let new = s[pos + 3..].to_string(); // skip " b/"
        return (old, new);
    }
    if let Some(pos) = s.find(' ') {
        let old = s[2..pos].to_string();
        let new = s[pos + 3..].to_string();
        return (old, new);
    }
    (s.to_string(), s.to_string())
}

/// Parse a `"a/..." "b/..."` header. Returns `None` if the structure doesn't match.
fn split_quoted_ab(s: &str) -> Option<(String, String)> {
    let close_a = find_closing_quote(s, 1)?;
    let after = &s[close_a + 1..];
    let after_b = after.strip_prefix(" \"b/")?;
    let new_inner = after_b.strip_suffix('"')?;
    let old_inner = &s[3..close_a]; // skip `"a/`
    Some((decode_path(old_inner), decode_path(new_inner)))
}

/// Find the closing `"` matching the opening quote at `start`, honoring `\"`.
fn find_closing_quote(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// Decode a git-emitted path: strip surrounding quotes, `\NNN` octal escapes, and
/// the `\\`, `\"`, `\t`, `\n`, `\r` short escapes back into bytes, then read as
/// UTF-8 (lossy if the bytes aren't valid UTF-8, so the filename is never lost).
pub(super) fn decode_path(s: &str) -> String {
    let inner = s
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s);

    if !inner.contains('\\') {
        return inner.to_string();
    }

    let bytes = inner.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        // `\` followed by something — try octal (\NNN) first, then short escapes.
        if bytes.get(i + 1).is_some_and(is_octal_digit)
            && bytes.get(i + 2).is_some_and(is_octal_digit)
            && bytes.get(i + 3).is_some_and(is_octal_digit)
        {
            let n = (octal_value(bytes[i + 1]) << 6)
                | (octal_value(bytes[i + 2]) << 3)
                | octal_value(bytes[i + 3]);
            out.push(n);
            i += 4;
            continue;
        }
        match bytes.get(i + 1) {
            Some(&b'\\') => {
                out.push(b'\\');
                i += 2;
            },
            Some(&b'"') => {
                out.push(b'"');
                i += 2;
            },
            Some(&b't') => {
                out.push(b'\t');
                i += 2;
            },
            Some(&b'n') => {
                out.push(b'\n');
                i += 2;
            },
            Some(&b'r') => {
                out.push(b'\r');
                i += 2;
            },
            _ => {
                out.push(b'\\');
                i += 1;
            },
        }
    }

    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(&e.into_bytes()).into_owned())
}

fn is_octal_digit(b: &u8) -> bool {
    (b'0'..=b'7').contains(b)
}

fn octal_value(b: u8) -> u8 {
    b - b'0'
}

/// Locate the rightmost ` b/` boundary that leaves a non-empty `a/` segment.
fn find_b_split(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut pos = s.len();
    while pos > 0 {
        pos -= 1;
        if bytes.get(pos) == Some(&b' ')
            && bytes.get(pos + 1) == Some(&b'b')
            && bytes.get(pos + 2) == Some(&b'/')
            && pos >= 3
        {
            return Some(pos);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DiffError;
    use crate::model::FileStatus;

    #[test]
    fn decode_path_passthrough_and_escapes() {
        assert_eq!(decode_path("src/main.rs"), "src/main.rs");
        // \343\201\223 = U+3053
        assert_eq!(decode_path("\\343\\201\\223.txt"), "こ.txt");
        assert_eq!(decode_path("a\\\\b"), "a\\b");
        assert_eq!(decode_path("a\\\"b"), "a\"b");
        assert_eq!(decode_path("a\\tb"), "a\tb");
    }

    #[test]
    fn split_bare_paths() {
        assert_eq!(
            split_ab_paths("a/src/main.rs b/src/main.rs"),
            ("src/main.rs".to_string(), "src/main.rs".to_string())
        );
        // A rename splits at the rightmost ` b/`.
        assert_eq!(
            split_ab_paths("a/foo.rs b/bar.rs"),
            ("foo.rs".to_string(), "bar.rs".to_string())
        );
    }

    #[test]
    fn split_quoted_paths() {
        assert_eq!(
            split_ab_paths("\"a/foo bar.txt\" \"b/foo bar.txt\""),
            ("foo bar.txt".to_string(), "foo bar.txt".to_string())
        );
    }

    #[test]
    fn split_path_containing_b_slash_takes_the_rightmost_boundary() {
        // A path with a `b/` directory in it splits at the *last* ` b/`.
        assert_eq!(
            split_ab_paths("a/src b/lib.rs b/src b/lib.rs"),
            ("src b/lib.rs b/src".to_string(), "lib.rs".to_string())
        );
        // Genuinely ambiguous without quoting, but unreachable from git: it quotes
        // any path containing a space, which `split_quoted_ab` then splits exactly.
        assert_eq!(
            split_ab_paths("\"a/src b/lib.rs\" \"b/src b/lib.rs\""),
            ("src b/lib.rs".to_string(), "src b/lib.rs".to_string())
        );
    }

    #[test]
    fn parse_quoted_path_with_space() -> Result<(), DiffError> {
        let raw = "diff --git \"a/foo bar.txt\" \"b/foo bar.txt\"\nindex aaa..bbb 100644\n--- \"a/foo bar.txt\"\n+++ \"b/foo bar.txt\"\n@@ -1,1 +1,1 @@\n-old\n+new\n";
        let diff = crate::parse::parse(raw)?;
        assert_eq!(diff.files[0].path, "foo bar.txt");
        Ok(())
    }

    #[test]
    fn parse_quoted_path_with_octal_escape() -> Result<(), DiffError> {
        // \357\274\232 = U+FF1A FULLWIDTH COLON
        let raw = "diff --git \"a/A1\\357\\274\\232 X.html\" \"b/A1\\357\\274\\232 X.html\"\nnew file mode 100644\nindex 0000000..1111111\n--- /dev/null\n+++ \"b/A1\\357\\274\\232 X.html\"\n@@ -0,0 +1,1 @@\n+hi\n";
        let diff = crate::parse::parse(raw)?;
        assert_eq!(diff.files[0].path, "A1\u{ff1a} X.html");
        assert_eq!(diff.files[0].status, FileStatus::Added);
        Ok(())
    }

    #[test]
    fn parse_quoted_renamed_path() -> Result<(), DiffError> {
        let raw = "diff --git \"a/A\\357\\274\\232.txt\" \"b/B\\357\\274\\232.txt\"\nsimilarity index 100%\nrename from \"A\\357\\274\\232.txt\"\nrename to \"B\\357\\274\\232.txt\"\n";
        let diff = crate::parse::parse(raw)?;
        let f = &diff.files[0];
        assert_eq!(f.path, "B\u{ff1a}.txt");
        assert_eq!(f.old_path.as_deref(), Some("A\u{ff1a}.txt"));
        assert!(matches!(f.status, FileStatus::Renamed { similarity: 100 }));
        Ok(())
    }

    #[test]
    fn parse_quoted_copied_path() -> Result<(), DiffError> {
        let raw = "diff --git \"a/A B.txt\" \"b/C D.txt\"\nsimilarity index 100%\ncopy from \"A B.txt\"\ncopy to \"C D.txt\"\n";
        let diff = crate::parse::parse(raw)?;
        let f = &diff.files[0];
        assert_eq!(f.path, "C D.txt");
        assert_eq!(f.old_path.as_deref(), Some("A B.txt"));
        Ok(())
    }
}
