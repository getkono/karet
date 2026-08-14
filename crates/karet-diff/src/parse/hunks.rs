//! Parsing `@@ … @@` hunk headers and their prefixed content lines.

use crate::DiffError;
use crate::model::DiffLine;
use crate::model::Hunk;
use crate::model::LineKind;
use crate::parse::scope::extract_scope;
use crate::parse::strip_cr;

/// Parse the hunks in `lines` (raw lines, `\r` not yet stripped).
pub(super) fn parse_hunks(lines: &[&str]) -> Result<Vec<Hunk>, DiffError> {
    let mut hunks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let (line, _) = strip_cr(lines[i]);
        if !line.starts_with("@@ ") {
            i += 1;
            continue;
        }

        let (old_start, old_count, new_start, new_count) = parse_hunk_header(line)?;
        let header = line.to_string();
        let scope = extract_scope(&header);
        i += 1;

        let mut diff_lines = Vec::new();
        let mut old_lineno = old_start;
        let mut new_lineno = new_start;

        while i < lines.len() && !strip_cr(lines[i]).0.starts_with("@@ ") {
            let (l, crlf) = strip_cr(lines[i]);
            if l.starts_with("\\ ") {
                // "\ No newline at end of file" — a property of the line before it,
                // on whichever side that line belongs to.
                if let Some(last) = diff_lines.last_mut() {
                    let last: &mut DiffLine = last;
                    last.no_newline = true;
                }
                i += 1;
                continue;
            }
            let (kind, content) = if let Some(rest) = l.strip_prefix('+') {
                (LineKind::Add, rest)
            } else if let Some(rest) = l.strip_prefix('-') {
                (LineKind::Remove, rest)
            } else {
                (LineKind::Context, l.strip_prefix(' ').unwrap_or(l))
            };

            let (old_ln, new_ln) = match kind {
                LineKind::Context => {
                    let pair = (Some(old_lineno), Some(new_lineno));
                    old_lineno += 1;
                    new_lineno += 1;
                    pair
                },
                LineKind::Remove => {
                    let pair = (Some(old_lineno), None);
                    old_lineno += 1;
                    pair
                },
                LineKind::Add => {
                    let pair = (None, Some(new_lineno));
                    new_lineno += 1;
                    pair
                },
            };

            let mut line = DiffLine::new(kind, old_ln, new_ln, content);
            line.crlf = crlf;
            diff_lines.push(line);
            i += 1;
        }

        hunks.push(Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
            header,
            scope,
            new_scope: None,
            lines: diff_lines,
        });
    }

    Ok(hunks)
}

/// Parse `@@ -old_start[,old_count] +new_start[,new_count] @@`.
fn parse_hunk_header(line: &str) -> Result<(u32, u32, u32, u32), DiffError> {
    let err = || DiffError::Parse(format!("invalid hunk header: {line}"));

    let inner = line.strip_prefix("@@ ").ok_or_else(err)?;
    let end = inner.find(" @@").ok_or_else(err)?;
    let ranges = &inner[..end];

    let mut parts = ranges.splitn(2, ' ');
    let old_part = parts.next().ok_or_else(err)?;
    let new_part = parts.next().ok_or_else(err)?;

    let (old_start, old_count) = parse_range(old_part.trim_start_matches('-'))?;
    let (new_start, new_count) = parse_range(new_part.trim_start_matches('+'))?;

    Ok((old_start, old_count, new_start, new_count))
}

fn parse_range(s: &str) -> Result<(u32, u32), DiffError> {
    let err = || DiffError::Parse(format!("invalid range: {s}"));
    if let Some((a, b)) = s.split_once(',') {
        let start = a.parse::<u32>().map_err(|_| err())?;
        let count = b.parse::<u32>().map_err(|_| err())?;
        Ok((start, count))
    } else {
        let start = s.parse::<u32>().map_err(|_| err())?;
        Ok((start, 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunk_header_ranges() -> Result<(), DiffError> {
        assert_eq!(parse_hunk_header("@@ -1,5 +2,6 @@")?, (1, 5, 2, 6));
        // A range without a count means exactly one line.
        assert_eq!(parse_hunk_header("@@ -3 +4 @@")?, (3, 1, 4, 1));
        // An empty side reports a 0-based start.
        assert_eq!(parse_hunk_header("@@ -0,0 +1,3 @@")?, (0, 0, 1, 3));
        // Trailing scope text does not disturb the ranges.
        assert_eq!(
            parse_hunk_header("@@ -1,2 +1,2 @@ fn main() {")?,
            (1, 2, 1, 2)
        );
        Ok(())
    }

    #[test]
    fn malformed_headers_error() {
        assert!(parse_hunk_header("@@ bogus @@").is_err());
        assert!(parse_hunk_header("@@ -1,5 @@").is_err());
        assert!(parse_hunk_header("not a header").is_err());
    }

    #[test]
    fn crlf_content_is_flagged_not_kept() -> Result<(), DiffError> {
        let raw = ["@@ -1,2 +1,2 @@", " alpha\r", "-beta\r", "+GAMMA\r"];
        let hunks = parse_hunks(&raw)?;
        let lines = &hunks[0].lines;
        assert_eq!(lines.len(), 3);
        // The `\r` belongs to the file, so it is recorded but kept out of `content`.
        assert!(lines.iter().all(|l| l.crlf));
        let contents: Vec<&str> = lines.iter().map(|l| l.content.as_str()).collect();
        assert_eq!(contents, ["alpha", "beta", "GAMMA"]);
        assert_eq!(lines[1].terminator(), "\r\n");
        Ok(())
    }

    #[test]
    fn no_newline_marker_attaches_to_the_preceding_line() -> Result<(), DiffError> {
        // Both sides can lack a trailing newline, each marked after its own line.
        let raw = [
            "@@ -1 +1 @@",
            "-old",
            "\\ No newline at end of file",
            "+new",
            "\\ No newline at end of file",
        ];
        let hunks = parse_hunks(&raw)?;
        let lines = &hunks[0].lines;
        assert_eq!(lines.len(), 2);
        assert!(lines[0].no_newline && lines[1].no_newline);
        assert_eq!(lines[0].terminator(), "");
        Ok(())
    }

    #[test]
    fn a_marker_with_no_preceding_line_is_ignored() -> Result<(), DiffError> {
        // Malformed, but must not panic.
        let raw = ["@@ -1 +1 @@", "\\ No newline at end of file", "+new"];
        let hunks = parse_hunks(&raw)?;
        assert_eq!(hunks[0].lines.len(), 1);
        assert!(!hunks[0].lines[0].no_newline);
        Ok(())
    }
}
