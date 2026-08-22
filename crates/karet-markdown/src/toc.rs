//! Table-of-contents generation and heading-level editing.
//!
//! The TOC renders as a bullet list of links between `<!-- toc -->` and
//! `<!-- /toc -->` marker comments, so an update can find and replace exactly
//! what it generated. Anchors use GitHub's slugify rules; a heading carrying
//! `<!-- omit from toc -->` (on its own line above, or trailing) is skipped.

use std::collections::HashMap;
use std::ops::RangeInclusive;

/// Options shaping a generated TOC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TocOptions {
    /// The heading levels included (GitHub's convention skips the `#` title).
    pub levels: RangeInclusive<u8>,
}

impl Default for TocOptions {
    /// Levels 2–6: the document title stays out of its own table.
    fn default() -> Self {
        Self { levels: 2..=6 }
    }
}

/// The marker line opening a generated TOC.
pub const TOC_START: &str = "<!-- toc -->";
/// The marker line closing a generated TOC.
pub const TOC_END: &str = "<!-- /toc -->";
/// The directive excluding a heading from the TOC.
pub const OMIT: &str = "<!-- omit from toc -->";

/// One heading as the TOC sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TocHeading {
    level: u8,
    text: String,
    slug: String,
}

/// GitHub's anchor slug for a heading text: lowercased, punctuation dropped
/// (hyphens and underscores survive), spaces hyphenated. Uniqueness suffixes
/// (`-1`, `-2`, …) are applied by the caller across the whole document.
#[must_use]
pub fn github_slugify(text: &str) -> String {
    let mut slug = String::new();
    for c in text.trim().chars() {
        if c.is_alphanumeric() || c == '_' || c == '-' {
            for lower in c.to_lowercase() {
                slug.push(lower);
            }
        } else if c == ' ' {
            slug.push('-');
        }
        // Everything else (punctuation, emoji joiners) is dropped.
    }
    slug
}

/// Collect the in-scope headings of `text`, slugs deduplicated the way GitHub
/// does (every heading counts toward uniqueness, even out-of-range ones).
fn headings(text: &str, options: &TocOptions) -> Vec<TocHeading> {
    let lines: Vec<&str> = text.lines().collect();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    // One incremental fence scan for the whole document: asking
    // `in_fenced_code_block` per line would rescan from the top each time and
    // make this pass quadratic (visibly so on a long document).
    let mut fences = crate::edit::FenceScan::default();
    for (i, line) in lines.iter().enumerate() {
        let fenced = fences.inside();
        fences.feed(line);
        if fenced {
            continue;
        }
        let trimmed = line.trim_start();
        let level = trimmed.chars().take_while(|&c| c == '#').count();
        if level == 0 || level > 6 || !matches!(trimmed.chars().nth(level), Some(' ')) {
            continue;
        }
        let mut heading_text = trimmed[level + 1..].trim().to_owned();
        let omitted = heading_text.contains(OMIT) || (i > 0 && lines[i - 1].trim() == OMIT);
        if let Some(idx) = heading_text.find("<!--") {
            heading_text.truncate(idx);
            heading_text = heading_text.trim_end().to_owned();
        }
        let base = github_slugify(&heading_text);
        let n = seen.entry(base.clone()).or_insert(0);
        let slug = if *n == 0 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        *n += 1;
        let level = u8::try_from(level).unwrap_or(6);
        if omitted || !options.levels.contains(&level) {
            continue;
        }
        out.push(TocHeading {
            level,
            text: heading_text,
            slug,
        });
    }
    out
}

/// Render the TOC for `text` as lines, markers included. `None` when no
/// heading is in scope (an empty TOC would only be noise).
#[must_use]
pub fn render_toc(text: &str, options: &TocOptions) -> Option<Vec<String>> {
    let headings = headings(text, options);
    let min_level = headings.iter().map(|h| h.level).min()?;
    let mut lines = vec![TOC_START.to_owned()];
    for h in &headings {
        let indent = "  ".repeat(usize::from(h.level - min_level));
        lines.push(format!("{indent}- [{}](#{})", h.text, h.slug));
    }
    lines.push(TOC_END.to_owned());
    Some(lines)
}

/// The 0-based line span of an existing `<!-- toc -->` … `<!-- /toc -->`
/// region (markers included), if the document has one.
#[must_use]
pub fn toc_region(text: &str) -> Option<(usize, usize)> {
    let mut start = None;
    for (i, line) in text.lines().enumerate() {
        match (start, line.trim()) {
            (None, t) if t == TOC_START => start = Some(i),
            (Some(s), t) if t == TOC_END => return Some((s, i)),
            _ => {},
        }
    }
    None
}

/// Shift the ATX heading on `line` by `delta` levels: `#` added or removed,
/// clamped to 1–6; a plain line becomes a level-1 heading on the way up, and
/// a level-1 heading becomes plain on the way down. `None` when there is
/// nothing to do.
#[must_use]
pub fn heading_shift(line: &str, delta: i8) -> Option<String> {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let level = rest.chars().take_while(|&c| c == '#').count();
    let is_heading = level > 0 && matches!(rest.chars().nth(level), None | Some(' '));
    let body = if is_heading {
        rest[level..].trim_start()
    } else {
        rest
    };
    let current = if is_heading { level as i8 } else { 0 };
    let next = (current + delta).clamp(0, 6);
    if next == current {
        return None;
    }
    if next == 0 {
        return Some(format!("{indent}{body}"));
    }
    let hashes = "#".repeat(usize::try_from(next).unwrap_or(1));
    Some(format!("{indent}{hashes} {body}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_matches_github_conventions() {
        assert_eq!(github_slugify("Design principles"), "design-principles");
        assert_eq!(github_slugify("What's new? (v2)"), "whats-new-v2");
        assert_eq!(
            github_slugify("snake_case and kebab-case"),
            "snake_case-and-kebab-case"
        );
        assert_eq!(github_slugify("Ünïcode Héadings"), "ünïcode-héadings");
        assert_eq!(github_slugify("  padded  "), "padded");
    }

    #[test]
    fn toc_renders_nested_links_with_deduplicated_slugs() {
        let text = "# Title\n\n## Setup\n\ntext\n\n### Setup\n\n## Usage\n";
        let toc = render_toc(text, &TocOptions::default()).unwrap_or_default();
        assert_eq!(
            toc,
            vec![
                TOC_START.to_owned(),
                "- [Setup](#setup)".to_owned(),
                "  - [Setup](#setup-1)".to_owned(),
                "- [Usage](#usage)".to_owned(),
                TOC_END.to_owned(),
            ]
        );
    }

    #[test]
    fn omitted_and_out_of_range_headings_stay_out_but_count_for_slugs() {
        let text = "# Title <!-- omit from toc -->\n\n## Keep\n\n<!-- omit from toc -->\n## Skipped\n\n## Skipped\n";
        let toc = render_toc(text, &TocOptions::default()).unwrap_or_default();
        // The second "Skipped" keeps its -1 suffix even though the first is
        // omitted — GitHub numbers anchors over every rendered heading.
        assert_eq!(
            toc,
            vec![
                TOC_START.to_owned(),
                "- [Keep](#keep)".to_owned(),
                "- [Skipped](#skipped-1)".to_owned(),
                TOC_END.to_owned(),
            ]
        );
    }

    #[test]
    fn headings_inside_fences_are_code_not_structure() {
        let text = "## Real\n\n```sh\n## not a heading\n```\n";
        let toc = render_toc(text, &TocOptions::default()).unwrap_or_default();
        assert_eq!(toc.len(), 3);
        assert!(toc[1].contains("Real"));
    }

    #[test]
    fn a_document_with_no_scoped_headings_renders_nothing() {
        assert_eq!(render_toc("plain text\n", &TocOptions::default()), None);
        // A lone title is out of the default 2..=6 range.
        assert_eq!(render_toc("# Title\n", &TocOptions::default()), None);
    }

    #[test]
    fn toc_region_finds_the_marker_span() {
        let text = "intro\n<!-- toc -->\n- [a](#a)\n<!-- /toc -->\nrest\n";
        assert_eq!(toc_region(text), Some((1, 3)));
        assert_eq!(toc_region("no markers\n"), None);
        // An unclosed opener is not a region.
        assert_eq!(toc_region("<!-- toc -->\n"), None);
    }

    #[test]
    fn heading_shift_adds_removes_and_clamps() {
        assert_eq!(heading_shift("## Two", 1), Some("### Two".to_owned()));
        assert_eq!(heading_shift("## Two", -1), Some("# Two".to_owned()));
        assert_eq!(heading_shift("# One", -1), Some("One".to_owned()));
        assert_eq!(heading_shift("plain", 1), Some("# plain".to_owned()));
        assert_eq!(heading_shift("###### Six", 1), None);
        assert_eq!(heading_shift("plain", -1), None);
        assert_eq!(
            heading_shift("  # indented", 1),
            Some("  ## indented".to_owned())
        );
    }
}
