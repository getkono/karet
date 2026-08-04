//! Source-level GitHub-flavored Markdown table discovery and formatting.

use std::ops::Range;
use std::ops::RangeInclusive;

use pulldown_cmark::Event;
use pulldown_cmark::Options;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use unicode_width::UnicodeWidthStr;

use crate::Alignment;

fn table_byte_ranges(source: &str) -> Vec<Range<usize>> {
    Parser::new_ext(source, Options::ENABLE_TABLES)
        .into_offset_iter()
        .filter_map(|(event, range)| matches!(event, Event::Start(Tag::Table(_))).then_some(range))
        .collect()
}

/// Return the inclusive, zero-based source-line ranges occupied by GFM tables.
///
/// Recognition comes from the same CommonMark/GFM parser as [`crate::parse`], so
/// pipe-shaped text in fenced code blocks is not mistaken for a table.
#[must_use]
pub fn table_line_ranges(source: &str) -> Vec<RangeInclusive<u32>> {
    table_byte_ranges(source)
        .into_iter()
        .map(|range| {
            let start = source[..range.start]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count();
            let through_end = source[..range.end]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count();
            let end = through_end.saturating_sub(usize::from(
                range.end > range.start && source.as_bytes()[range.end - 1] == b'\n',
            ));
            u32::try_from(start).unwrap_or(u32::MAX)..=u32::try_from(end).unwrap_or(u32::MAX)
        })
        .collect()
}

/// Align every GFM table in `source`, preserving non-table text and line endings.
///
/// Rows receive conventional outer pipes and one padding space. Columns use
/// terminal display width, so wide Unicode text remains visually aligned. Pipes
/// escaped with a backslash or enclosed in a code span remain cell content.
#[must_use]
pub fn format_tables(source: &str) -> String {
    let mut formatted = source.to_owned();
    for range in table_byte_ranges(source).into_iter().rev() {
        if let Some(table) = format_table(&source[range.clone()]) {
            formatted.replace_range(range, &table);
        }
    }
    formatted
}

#[derive(Clone, Copy)]
struct SourceLine<'a> {
    content: &'a str,
    ending: &'a str,
}

fn source_lines(source: &str) -> Vec<SourceLine<'_>> {
    source
        .split_inclusive('\n')
        .map(|line| {
            if let Some(content) = line.strip_suffix("\r\n") {
                SourceLine {
                    content,
                    ending: "\r\n",
                }
            } else if let Some(content) = line.strip_suffix('\n') {
                SourceLine {
                    content,
                    ending: "\n",
                }
            } else {
                SourceLine {
                    content: line,
                    ending: "",
                }
            }
        })
        .collect()
}

fn markdown_prefix(line: &str) -> (&str, &str) {
    let mut end = 0;
    let mut chars = line.char_indices().peekable();
    while let Some((index, ch)) = chars.peek().copied() {
        if ch == ' ' || ch == '\t' {
            end = index + ch.len_utf8();
            chars.next();
            continue;
        }
        if ch == '>' {
            end = index + 1;
            chars.next();
            if let Some((space, ' ' | '\t')) = chars.peek().copied() {
                end = space + 1;
                chars.next();
            }
            continue;
        }
        break;
    }
    line.split_at(end)
}

fn split_cells(line: &str) -> Vec<String> {
    let (_, body) = markdown_prefix(line);
    let body = body.trim();
    let body = body.strip_prefix('|').unwrap_or(body);
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = body.chars().peekable();
    let mut code_fence = 0_usize;
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            cell.push(ch);
            if let Some(escaped) = chars.next() {
                cell.push(escaped);
            }
            continue;
        }
        if ch == '`' {
            let mut run = 1;
            while chars.next_if_eq(&'`').is_some() {
                run += 1;
            }
            cell.extend(std::iter::repeat_n('`', run));
            if code_fence == 0 {
                code_fence = run;
            } else if code_fence == run {
                code_fence = 0;
            }
            continue;
        }
        if ch == '|' && code_fence == 0 {
            cells.push(cell.trim().to_owned());
            cell.clear();
        } else {
            cell.push(ch);
        }
    }
    if !cell.trim().is_empty() || !body.ends_with('|') {
        cells.push(cell.trim().to_owned());
    }
    cells
}

fn alignment(cell: &str) -> Option<Alignment> {
    let marker = cell.trim();
    let left = marker.starts_with(':');
    let right = marker.ends_with(':');
    let dashes = marker.trim_matches(':');
    (!dashes.is_empty() && dashes.chars().all(|ch| ch == '-')).then_some(match (left, right) {
        (true, true) => Alignment::Center,
        (true, false) => Alignment::Left,
        (false, true) => Alignment::Right,
        (false, false) => Alignment::None,
    })
}

fn padded(content: &str, width: usize, alignment: Alignment) -> String {
    let missing = width.saturating_sub(UnicodeWidthStr::width(content));
    match alignment {
        Alignment::Right => format!("{}{content}", " ".repeat(missing)),
        Alignment::Center => {
            let left = missing / 2;
            format!(
                "{}{content}{}",
                " ".repeat(left),
                " ".repeat(missing - left)
            )
        },
        Alignment::None | Alignment::Left => format!("{content}{}", " ".repeat(missing)),
    }
}

fn separator(width: usize, alignment: Alignment) -> String {
    let width = width.max(3);
    match alignment {
        Alignment::Left => format!(":{}", "-".repeat(width - 1)),
        Alignment::Right => format!("{}:", "-".repeat(width - 1)),
        Alignment::Center => format!(":{}:", "-".repeat(width - 2)),
        Alignment::None => "-".repeat(width),
    }
}

fn format_table(source: &str) -> Option<String> {
    let lines = source_lines(source);
    if lines.len() < 2 {
        return None;
    }
    let mut rows: Vec<Vec<String>> = lines.iter().map(|line| split_cells(line.content)).collect();
    let mut alignments: Vec<Alignment> = rows
        .get(1)?
        .iter()
        .map(|cell| alignment(cell))
        .collect::<Option<_>>()?;
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    if columns == 0 {
        return None;
    }
    alignments.resize(columns, Alignment::None);
    for row in &mut rows {
        row.resize(columns, String::new());
    }
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .enumerate()
                .filter(|(row, _)| *row != 1)
                .map(|(_, row)| UnicodeWidthStr::width(row[column].as_str()))
                .max()
                .unwrap_or(0)
                .max(3)
        })
        .collect();

    let mut output = String::new();
    for (index, line) in lines.iter().enumerate() {
        let (prefix, _) = markdown_prefix(line.content);
        output.push_str(prefix);
        output.push('|');
        for column in 0..columns {
            output.push(' ');
            if index == 1 {
                output.push_str(&separator(widths[column], alignments[column]));
            } else {
                output.push_str(&padded(
                    &rows[index][column],
                    widths[column],
                    alignments[column],
                ));
            }
            output.push_str(" |");
        }
        output.push_str(line.ending);
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_tables_but_not_pipe_examples_in_fences() {
        let source = "before\n```md\n| no |\n| -- |\n```\n| yes | here |\n| ---: | :---: |\n| 1 | 你好 |\n\nafter\n";
        assert_eq!(table_line_ranges(source), vec![5..=7]);
    }

    #[test]
    fn formats_alignment_wide_text_escaped_pipes_and_code_spans() {
        let source = "| Name|Value|Note |\n|:--|--:|:-:|\n|你好|7|a\\|b|\n|x|123|`a|b`|\n";
        assert_eq!(
            format_tables(source),
            "| Name | Value | Note  |\n\
             | :--- | ----: | :---: |\n\
             | 你好 |     7 | a\\|b  |\n\
             | x    |   123 | `a|b` |\n"
        );
    }

    #[test]
    fn formatting_is_idempotent_and_preserves_crlf_and_surroundings() {
        let source = "lead\r\n\r\n|a|b\r\n|-|-|\r\n|1|22\r\n\r\ntail";
        let once = format_tables(source);
        assert!(once.starts_with("lead\r\n\r\n"));
        assert!(once.ends_with("\r\n\r\ntail"));
        assert_eq!(format_tables(&once), once);
    }

    #[test]
    fn quoted_table_keeps_each_quote_prefix() {
        let source = "> | a | b |\n> | - | :-: |\n> | one | two |\n";
        assert_eq!(
            format_tables(source),
            "> | a   |  b  |\n> | --- | :-: |\n> | one | two |\n"
        );
    }
}
