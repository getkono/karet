//! The rule implementations. Each rule scans the shared [`Context`] and
//! pushes [`Issue`]s; severity is corrected by the caller from the config.

use super::Config;
use super::Fix;
use super::Issue;
use super::LintSeverity;

/// The identifiers implemented here, id ↔ alias.
const RULES: &[(&str, &str)] = &[
    ("MD001", "heading-increment"),
    ("MD009", "no-trailing-spaces"),
    ("MD010", "no-hard-tabs"),
    ("MD012", "no-multiple-blanks"),
    ("MD013", "line-length"),
    ("MD018", "no-missing-space-atx"),
    ("MD019", "no-multiple-space-atx"),
    ("MD022", "blanks-around-headings"),
    ("MD023", "heading-start-left"),
    ("MD026", "no-trailing-punctuation"),
    ("MD031", "blanks-around-fences"),
    ("MD032", "blanks-around-lists"),
    ("MD034", "no-bare-urls"),
    ("MD037", "no-space-in-emphasis"),
    ("MD038", "no-space-in-code"),
    ("MD039", "no-space-in-links"),
    ("MD040", "fenced-code-language"),
    ("MD041", "first-line-heading"),
    ("MD042", "no-empty-links"),
    ("MD045", "no-alt-text"),
    ("MD047", "single-trailing-newline"),
];

/// Resolve a rule id or alias (any case) to its canonical uppercase id.
pub(super) fn canonical_id(name: &str) -> Option<&'static str> {
    let lowered = name.to_ascii_lowercase();
    RULES
        .iter()
        .find(|(id, alias)| id.eq_ignore_ascii_case(&lowered) || *alias == lowered)
        .map(|(id, _)| *id)
}

/// Everything the rules need, computed once.
pub(super) struct Context<'a> {
    pub text: &'a str,
    pub lines: &'a [&'a str],
    pub config: &'a Config,
    /// Per line: inside a fenced code block (fence delimiters excluded).
    pub in_fence: Vec<bool>,
    /// Per line: is a fence delimiter.
    pub is_fence_line: Vec<bool>,
    /// Lines occupied by YAML front matter (`---` … `---` at the top).
    pub front_matter_end: usize,
}

impl<'a> Context<'a> {
    pub(super) fn new(text: &'a str, lines: &'a [&'a str], config: &'a Config) -> Self {
        let mut in_fence = Vec::with_capacity(lines.len());
        let mut is_fence_line = vec![false; lines.len()];
        let mut fence: Option<(char, usize)> = None;
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            let delim = if trimmed.starts_with("```") {
                Some(('`', trimmed.chars().take_while(|&c| c == '`').count()))
            } else if trimmed.starts_with("~~~") {
                Some(('~', trimmed.chars().take_while(|&c| c == '~').count()))
            } else {
                None
            };
            match (fence, delim) {
                (Some((open, len)), Some((c, l))) if c == open && l >= len => {
                    in_fence.push(true); // the closer renders inside for MD010 etc.? no:
                    is_fence_line[i] = true;
                    fence = None;
                    // A delimiter line is not interior.
                    if let Some(last) = in_fence.last_mut() {
                        *last = false;
                    }
                },
                (Some(_), _) => in_fence.push(true),
                (None, Some(_)) => {
                    is_fence_line[i] = true;
                    in_fence.push(false);
                    fence = delim;
                },
                (None, None) => in_fence.push(false),
            }
        }
        let mut front_matter_end = 0;
        if lines.first().is_some_and(|l| l.trim_end() == "---") {
            for (i, line) in lines.iter().enumerate().skip(1) {
                if line.trim_end() == "---" {
                    front_matter_end = i + 1;
                    break;
                }
            }
        }
        Self {
            text,
            lines,
            config,
            in_fence,
            is_fence_line,
            front_matter_end,
        }
    }

    /// Whether `line` is prose the structural rules should look at.
    fn prose(&self, line: usize) -> bool {
        line >= self.front_matter_end && !self.in_fence.get(line).copied().unwrap_or(false)
    }

    /// The ATX heading level of `line`, when it is one.
    fn heading_level(&self, line: usize) -> Option<usize> {
        if !self.prose(line) || self.is_fence_line.get(line).copied().unwrap_or(false) {
            return None;
        }
        let trimmed = self.lines.get(line)?.trim_start();
        let level = trimmed.chars().take_while(|&c| c == '#').count();
        ((1..=6).contains(&level) && matches!(trimmed.chars().nth(level), None | Some(' ' | '#')))
            .then_some(level)
    }
}

/// A shorthand issue constructor at default severity.
fn issue(
    line: usize,
    col: usize,
    len: usize,
    rule: &'static str,
    message: impl Into<String>,
    fix: Option<Fix>,
) -> Issue {
    let alias = RULES
        .iter()
        .find(|(id, _)| *id == rule)
        .map_or("", |(_, alias)| alias);
    Issue {
        line,
        col,
        len: len.max(1),
        rule,
        alias,
        message: message.into(),
        severity: LintSeverity::Error,
        fix,
    }
}

/// Run every rule.
pub(super) fn run_all(cx: &Context<'_>, issues: &mut Vec<Issue>) {
    per_line(cx, issues);
    headings(cx, issues);
    blanks_and_structure(cx, issues);
    inline_spans(cx, issues);
    document_level(cx, issues);
}

/// MD009, MD010, MD013, MD034 — simple per-line scans.
fn per_line(cx: &Context<'_>, issues: &mut Vec<Issue>) {
    for (i, line) in cx.lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let trailing = chars.iter().rev().take_while(|&&c| c == ' ').count();
        if trailing > 0 && trailing != cx.config.br_spaces {
            issues.push(issue(
                i,
                chars.len() - trailing,
                trailing,
                "MD009",
                "trailing spaces",
                Some(Fix::Replace {
                    line: i,
                    text: line.trim_end_matches(' ').to_owned(),
                }),
            ));
        }
        if let Some(col) = chars.iter().position(|&c| c == '\t') {
            issues.push(issue(
                i,
                col,
                1,
                "MD010",
                "hard tab",
                Some(Fix::Replace {
                    line: i,
                    text: line.replace('\t', "    "),
                }),
            ));
        }
        if chars.len() > cx.config.line_length && !line.contains("://") {
            issues.push(issue(
                i,
                cx.config.line_length,
                chars.len() - cx.config.line_length,
                "MD013",
                format!(
                    "line length {} exceeds {}",
                    chars.len(),
                    cx.config.line_length
                ),
                None,
            ));
        }
        // A link reference definition's destination is a link, not a bare URL;
        // upstream MD034 leaves them alone, and wrapping one in angle brackets
        // would rewrite a perfectly good definition.
        if cx.prose(i) && !is_link_definition(line) {
            for (col, url_len) in bare_urls(line) {
                let url: String = chars[col..col + url_len].iter().collect();
                issues.push(issue(
                    i,
                    col,
                    url_len,
                    "MD034",
                    "bare URL",
                    Some(Fix::Replace {
                        line: i,
                        text: {
                            let mut t: String = chars[..col].iter().collect();
                            t.push('<');
                            t.push_str(&url);
                            t.push('>');
                            t.extend(&chars[col + url_len..]);
                            t
                        },
                    }),
                ));
            }
        }
    }
}

/// Whether `line` is a link reference definition — `[label]: destination`,
/// optionally indented up to three spaces, as CommonMark defines it.
fn is_link_definition(line: &str) -> bool {
    let indent = line.len() - line.trim_start().len();
    if indent > 3 {
        return false;
    }
    let rest = line.trim_start();
    let Some(inner) = rest.strip_prefix('[') else {
        return false;
    };
    // The label runs to the first unescaped `]`, which must be followed by `:`.
    let mut escaped = false;
    for (offset, c) in inner.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '[' => return false, // labels do not nest
            ']' => return inner[offset + 1..].starts_with(':'),
            _ => {},
        }
    }
    false
}

/// Bare `http(s)://` URLs outside code spans, autolinks, and link syntax.
fn bare_urls(line: &str) -> Vec<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut in_code = false;
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '`' => in_code = !in_code,
            'h' if !in_code => {
                let rest: String = chars[i..].iter().collect();
                if rest.starts_with("http://") || rest.starts_with("https://") {
                    let before = if i == 0 { None } else { Some(chars[i - 1]) };
                    // `<url>`, `](url)`, `](url` and `"url"` are already links.
                    if matches!(before, Some('<' | '(' | '"' | '\'')) {
                        i += 1;
                        continue;
                    }
                    let len = chars[i..]
                        .iter()
                        .take_while(|&&c| !c.is_whitespace() && c != '>' && c != ')' && c != '"')
                        .count();
                    out.push((i, len));
                    i += len;
                    continue;
                }
            },
            _ => {},
        }
        i += 1;
    }
    out
}

/// MD001, MD018, MD019, MD022, MD023, MD026, MD041 — heading rules.
fn headings(cx: &Context<'_>, issues: &mut Vec<Issue>) {
    let mut previous_level: Option<usize> = None;
    let mut first_content_seen = false;
    for i in 0..cx.lines.len() {
        let line = cx.lines[i];
        if !cx.prose(i) {
            continue;
        }
        let trimmed = line.trim_start();
        // MD018/MD019 look at raw `#` runs, heading or not.
        let hashes = trimmed.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hashes) {
            let after: Vec<char> = trimmed.chars().skip(hashes).collect();
            let indent = line.chars().count() - trimmed.chars().count();
            if after.first().is_some_and(|c| !c.is_whitespace()) {
                issues.push(issue(
                    i,
                    indent,
                    hashes + 1,
                    "MD018",
                    "no space after hash on heading",
                    Some(Fix::Replace {
                        line: i,
                        text: format!(
                            "{}{} {}",
                            &line[..line.len() - trimmed.len()],
                            "#".repeat(hashes),
                            after.iter().collect::<String>()
                        ),
                    }),
                ));
            } else if after.first() == Some(&' ') && after.get(1) == Some(&' ') {
                issues.push(issue(
                    i,
                    indent + hashes,
                    after.iter().take_while(|&&c| c == ' ').count(),
                    "MD019",
                    "multiple spaces after hash on heading",
                    Some(Fix::Replace {
                        line: i,
                        text: format!(
                            "{}{} {}",
                            &line[..line.len() - trimmed.len()],
                            "#".repeat(hashes),
                            after.iter().collect::<String>().trim_start()
                        ),
                    }),
                ));
            }
        }
        let Some(level) = cx.heading_level(i) else {
            if !trimmed.is_empty() {
                if !first_content_seen && cx.state_on("MD041") {
                    issues.push(issue(
                        i,
                        0,
                        trimmed.chars().count(),
                        "MD041",
                        "first line is not a top-level heading",
                        None,
                    ));
                }
                first_content_seen = true;
            }
            continue;
        };
        if !first_content_seen {
            first_content_seen = true;
            if level != 1 && cx.state_on("MD041") {
                issues.push(issue(
                    i,
                    0,
                    line.chars().count(),
                    "MD041",
                    "first heading is not top-level",
                    None,
                ));
            }
        }
        // MD001: levels only ever step down by one.
        if let Some(prev) = previous_level
            && level > prev + 1
        {
            issues.push(issue(
                i,
                0,
                level,
                "MD001",
                format!("heading level jumps from {prev} to {level}"),
                None,
            ));
        }
        previous_level = Some(level);
        // MD023: headings start at the margin.
        if line.starts_with(' ') {
            issues.push(issue(
                i,
                0,
                line.chars().count() - trimmed.chars().count(),
                "MD023",
                "heading is indented",
                Some(Fix::Replace {
                    line: i,
                    text: trimmed.to_owned(),
                }),
            ));
        }
        // MD026: no trailing punctuation in heading text.
        let text_part = trimmed.trim_start_matches('#').trim();
        if let Some(last) = text_part.chars().last()
            && ".,;:!。，；：！".contains(last)
        {
            issues.push(issue(
                i,
                line.chars().count().saturating_sub(1),
                1,
                "MD026",
                "trailing punctuation in heading",
                Some(Fix::Replace {
                    line: i,
                    text: line.trim_end().trim_end_matches(last).trim_end().to_owned(),
                }),
            ));
        }
        // MD022: a blank line above and below.
        if i > cx.front_matter_end && !cx.lines[i - 1].trim().is_empty() {
            issues.push(issue(
                i,
                0,
                1,
                "MD022",
                "heading needs a blank line above",
                Some(Fix::InsertBlankBefore { line: i }),
            ));
        }
        if i + 1 < cx.lines.len() && !cx.lines[i + 1].trim().is_empty() {
            issues.push(issue(
                i,
                0,
                1,
                "MD022",
                "heading needs a blank line below",
                Some(Fix::InsertBlankBefore { line: i + 1 }),
            ));
        }
    }
}

impl Context<'_> {
    /// Whether the config leaves `rule` on (rules that must not double-report
    /// check before pushing; everything else is filtered by the caller).
    fn state_on(&self, rule: &'static str) -> bool {
        self.config.state(rule).is_some()
    }
}

/// MD012, MD022 companions, MD031, MD032, MD040 — blank-line structure.
fn blanks_and_structure(cx: &Context<'_>, issues: &mut Vec<Issue>) {
    let mut blanks = 0_usize;
    for i in 0..cx.lines.len() {
        let line = cx.lines[i];
        if line.trim().is_empty() && cx.prose(i) {
            blanks += 1;
            if blanks > 1 {
                issues.push(issue(
                    i,
                    0,
                    1,
                    "MD012",
                    "multiple consecutive blank lines",
                    Some(Fix::Delete { line: i }),
                ));
            }
        } else {
            blanks = 0;
        }
        if cx.is_fence_line[i] {
            let opener = !cx.in_fence.get(i.wrapping_sub(1)).copied().unwrap_or(false) || i == 0;
            if opener {
                // MD040: an opening fence names a language.
                let trimmed = cx.lines[i].trim_start();
                let info = trimmed.trim_start_matches(['`', '~']).trim();
                let is_opener = i
                    .checked_add(1)
                    .is_some_and(|next| cx.in_fence.get(next).copied().unwrap_or(false))
                    || cx.lines.get(i + 1).is_none();
                if info.is_empty() && is_opener {
                    issues.push(issue(
                        i,
                        0,
                        trimmed.chars().count(),
                        "MD040",
                        "fenced code block has no language",
                        None,
                    ));
                }
            }
            // MD031: blank lines around the whole fence.
            let before_interior =
                i > 0 && !cx.in_fence[i - 1] && !cx.lines[i - 1].trim().is_empty();
            let after_exterior = i + 1 < cx.lines.len()
                && !cx.in_fence[i + 1]
                && !cx.is_fence_line[i + 1]
                && !cx.lines[i + 1].trim().is_empty();
            let is_open = cx.in_fence.get(i + 1).copied().unwrap_or(false);
            if is_open && before_interior && !cx.is_fence_line[i - 1] {
                issues.push(issue(
                    i,
                    0,
                    1,
                    "MD031",
                    "fenced code block needs a blank line above",
                    Some(Fix::InsertBlankBefore { line: i }),
                ));
            }
            if !is_open && after_exterior {
                issues.push(issue(
                    i,
                    0,
                    1,
                    "MD031",
                    "fenced code block needs a blank line below",
                    Some(Fix::InsertBlankBefore { line: i + 1 }),
                ));
            }
        }
        // MD032: lists surrounded by blank lines.
        if cx.prose(i) && crate::edit::list_context(line).is_some() {
            let prev_is_list = i > 0
                && (crate::edit::list_context(cx.lines[i - 1]).is_some()
                    || cx.lines[i - 1].starts_with(' '));
            if i > 0 && !cx.lines[i - 1].trim().is_empty() && !prev_is_list {
                issues.push(issue(
                    i,
                    0,
                    1,
                    "MD032",
                    "list needs a blank line above",
                    Some(Fix::InsertBlankBefore { line: i }),
                ));
            }
            let next_is_list = cx.lines.get(i + 1).is_some_and(|next| {
                crate::edit::list_context(next).is_some()
                    || next.starts_with(' ')
                    || next.trim().is_empty()
            });
            if !next_is_list && cx.lines.get(i + 1).is_some() {
                issues.push(issue(
                    i,
                    0,
                    1,
                    "MD032",
                    "list needs a blank line below",
                    Some(Fix::InsertBlankBefore { line: i + 1 }),
                ));
            }
        }
    }
}

/// MD037, MD038, MD039, MD042, MD045 — inline-span rules.
fn inline_spans(cx: &Context<'_>, issues: &mut Vec<Issue>) {
    for (i, line) in cx.lines.iter().enumerate() {
        if !cx.prose(i) {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        // MD038: spaces just inside backticks.
        let mut open: Option<usize> = None;
        for (col, &c) in chars.iter().enumerate() {
            if c != '`' {
                continue;
            }
            match open.take() {
                None => open = Some(col),
                Some(start) => {
                    let interior: String = chars[start + 1..col].iter().collect();
                    if interior != interior.trim() && !interior.trim().is_empty() {
                        issues.push(issue(
                            i,
                            start,
                            col - start + 1,
                            "MD038",
                            "spaces inside code span",
                            Some(Fix::Replace {
                                line: i,
                                text: {
                                    let mut t: String = chars[..=start].iter().collect();
                                    t.push_str(interior.trim());
                                    t.extend(&chars[col..]);
                                    t
                                },
                            }),
                        ));
                    }
                },
            }
        }
        // MD037: spaces just inside `**`/`__` emphasis markers.
        for marker in ["**", "__"] {
            let m: Vec<char> = marker.chars().collect();
            let positions: Vec<usize> = (0..chars.len().saturating_sub(1))
                .filter(|&p| chars[p] == m[0] && chars[p + 1] == m[1])
                .collect();
            for pair in positions.chunks(2) {
                if let [a, b] = pair {
                    let interior: String = chars[a + 2..*b].iter().collect();
                    if interior != interior.trim() && !interior.trim().is_empty() {
                        issues.push(issue(
                            i,
                            *a,
                            b - a + 2,
                            "MD037",
                            "spaces inside emphasis markers",
                            Some(Fix::Replace {
                                line: i,
                                text: {
                                    let mut t: String = chars[..a + 2].iter().collect();
                                    t.push_str(interior.trim());
                                    t.extend(&chars[*b..]);
                                    t
                                },
                            }),
                        ));
                    }
                }
            }
        }
        // Link shapes: `[text](target)`.
        let mut idx = 0;
        while idx < chars.len() {
            if chars[idx] == '['
                && (idx == 0 || chars[idx - 1] != '!')
                && let Some(close) = find_from(&chars, idx, ']')
                && chars.get(close + 1) == Some(&'(')
                && let Some(end) = find_from(&chars, close + 1, ')')
            {
                let text: String = chars[idx + 1..close].iter().collect();
                let target: String = chars[close + 2..end].iter().collect();
                if text != text.trim() && !text.trim().is_empty() {
                    issues.push(issue(
                        i,
                        idx,
                        end - idx + 1,
                        "MD039",
                        "spaces inside link text",
                        Some(Fix::Replace {
                            line: i,
                            text: {
                                let mut t: String = chars[..=idx].iter().collect();
                                t.push_str(text.trim());
                                t.extend(&chars[close..]);
                                t
                            },
                        }),
                    ));
                }
                if target.trim().is_empty() || target.trim() == "#" {
                    issues.push(issue(i, idx, end - idx + 1, "MD042", "empty link", None));
                }
                idx = end + 1;
                continue;
            }
            if chars[idx] == '!'
                && chars.get(idx + 1) == Some(&'[')
                && let Some(close) = find_from(&chars, idx + 1, ']')
            {
                let alt: String = chars[idx + 2..close].iter().collect();
                if alt.trim().is_empty() {
                    issues.push(issue(
                        i,
                        idx,
                        close - idx + 1,
                        "MD045",
                        "image without alternate text",
                        None,
                    ));
                }
                idx = close + 1;
                continue;
            }
            idx += 1;
        }
    }
}

fn find_from(chars: &[char], after: usize, needle: char) -> Option<usize> {
    chars
        .iter()
        .enumerate()
        .skip(after + 1)
        .find(|&(_, &c)| c == needle)
        .map(|(p, _)| p)
}

/// MD047 — the file ends with exactly one newline.
fn document_level(cx: &Context<'_>, issues: &mut Vec<Issue>) {
    if cx.text.is_empty() {
        return;
    }
    let single_trailing = cx.text.ends_with('\n') && !cx.text.ends_with("\n\n");
    if !single_trailing {
        let last = cx.lines.len().saturating_sub(1);
        issues.push(issue(
            last,
            cx.lines.last().map_or(0, |l| l.chars().count()),
            1,
            "MD047",
            "file should end with a single newline",
            Some(Fix::EnsureTrailingNewline),
        ));
    }
}
