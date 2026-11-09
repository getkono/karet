//! The source lines GFM tables occupy, read from an already-parsed tree.

use std::collections::HashMap;
use std::ops::RangeInclusive;

use karet_treesitter::LanguageId;
use karet_treesitter::Query;
use karet_treesitter::SyntaxTree;

/// The query naming a GFM table: tree-sitter-md's block grammar parses one as
/// a `pipe_table`, and never inside a fenced code block.
const TABLE_QUERY: &str = "(pipe_table) @table";

/// Finds the source lines GFM tables occupy, from a tree the caller already
/// parsed, so no reader has to re-parse the document to learn them.
///
/// Pass it a layered tree's root layer: a table inside an injected layer (a
/// fenced `markdown` block) is fence content in the document, not a table of
/// it.
///
/// Keeps each language's compiled query, so build one and reuse it across the
/// parses it serves.
#[derive(Default)]
pub struct TableFinder {
    /// The compiled query per root language; `None` for a grammar without a
    /// table node, remembered so it is not recompiled for every version.
    queries: HashMap<LanguageId, Option<Query>>,
}

impl TableFinder {
    /// A finder with no query compiled yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The inclusive 0-based line ranges of every table in `tree`, whose
    /// source is `text`, in document order — the same ranges
    /// `karet_markdown::table_line_ranges` reports: a table's trailing newline
    /// does not claim the next line. Empty for a grammar with no table node.
    #[must_use]
    pub fn line_ranges(&mut self, tree: &SyntaxTree, text: &str) -> Vec<RangeInclusive<u32>> {
        let lang = tree.language();
        let query = self
            .queries
            .entry(lang)
            .or_insert_with(|| Query::compile(lang, TABLE_QUERY).ok());
        let Some(query) = query.as_ref() else {
            return Vec::new();
        };
        let mut lines = LineCounter::new(text);
        tree.captures(query, text)
            .into_iter()
            .map(|capture| {
                let (start, end) = (capture.span.start.0, capture.span.end.0);
                let first = lines.line_of(start);
                // The line holding the table's last byte.
                let last = lines.line_of(end.saturating_sub(1).max(start));
                first..=last.max(first)
            })
            .collect()
    }
}

/// Maps byte offsets to 0-based line numbers by counting newlines forward, so
/// offsets asked for in ascending order cost one pass over the text in total.
struct LineCounter<'a> {
    text: &'a [u8],
    /// The byte offset counted up to so far.
    at: usize,
    /// The newlines in `text[..at]`.
    line: usize,
}

impl<'a> LineCounter<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text: text.as_bytes(),
            at: 0,
            line: 0,
        }
    }

    /// The line `byte` sits on (clamped to the text's end).
    fn line_of(&mut self, byte: usize) -> u32 {
        let byte = byte.min(self.text.len());
        if byte < self.at {
            // Out of order: count again from the start.
            self.at = 0;
            self.line = 0;
        }
        let newlines = self
            .text
            .get(self.at..byte)
            .map_or(0, |bytes| bytes.iter().filter(|b| **b == b'\n').count());
        self.line += newlines;
        self.at = byte;
        u32::try_from(self.line).unwrap_or(u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use karet_treesitter::LayeredParser;
    use karet_treesitter::language_id_from_injection_name;

    use super::*;

    /// The table line ranges of `text` parsed as Markdown, read from the root
    /// layer as a layered-tree consumer does.
    fn table_lines(text: &str) -> Option<Vec<RangeInclusive<u32>>> {
        let markdown = language_id_from_injection_name("markdown")?;
        let tree = LayeredParser::new().parse(markdown, text).ok()?;
        Some(TableFinder::new().line_ranges(tree.root(), text))
    }

    #[test]
    fn a_table_publishes_the_lines_it_occupies() {
        let found = table_lines("intro\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nafter\n");
        assert_eq!(found, Some(vec![2..=4]));
    }

    #[test]
    fn a_table_ending_the_file_without_a_newline_keeps_its_last_line() {
        let found = table_lines("| a | b |\n|---|---|\n| 1 | 2 |");
        assert_eq!(found, Some(vec![0..=2]));
    }

    #[test]
    fn every_table_is_found_in_document_order() {
        let source = "| a |\n|---|\n\ntext\n\n| b |\n|---|\n| 2 |\n";
        assert_eq!(table_lines(source), Some(vec![0..=1, 5..=7]));
    }

    #[test]
    fn a_table_inside_a_fenced_block_is_not_a_table() {
        // Even a `markdown` fence, whose injected layer parses the table: it is
        // fence content in this document, so only the root layer counts.
        for fence in ["```", "```markdown"] {
            let source = format!("{fence}\n| a | b |\n|---|---|\n| 1 | 2 |\n```\n");
            assert_eq!(table_lines(&source), Some(Vec::new()), "{fence}");
        }
    }

    #[test]
    fn a_quoted_table_is_a_table() {
        let found = table_lines("> | a | b |\n> |---|---|\n> | 1 | 2 |\n\nafter\n");
        assert_eq!(found, Some(vec![0..=2]));
    }

    #[test]
    fn a_table_in_a_list_item_is_a_table() {
        let found = table_lines("- item\n\n  | a | b |\n  |---|---|\n  | 1 | 2 |\n");
        assert_eq!(found, Some(vec![2..=4]));
    }

    #[test]
    fn pipes_without_a_delimiter_row_are_not_a_table() {
        assert_eq!(table_lines("a | b\nc | d\n"), Some(Vec::new()));
    }

    #[test]
    fn a_grammar_without_tables_publishes_none() -> Result<(), Box<dyn std::error::Error>> {
        let Some(rust) = language_id_from_injection_name("rust") else {
            return Ok(()); // rust grammar not compiled in; nothing to test
        };
        let source = "fn main() { let x = a | b; }\n";
        let tree = LayeredParser::new()
            .parse(rust, source)
            .map_err(|_| "the rust grammar failed to parse a valid file")?;
        let mut finder = TableFinder::new();
        assert!(finder.line_ranges(tree.root(), source).is_empty());
        assert!(finder.queries.get(&rust).is_some_and(Option::is_none));
        Ok(())
    }

    #[test]
    fn line_counter_answers_out_of_order_offsets() {
        let mut lines = LineCounter::new("a\nb\nc\n");
        assert_eq!(lines.line_of(4), 2);
        assert_eq!(lines.line_of(2), 1);
        assert_eq!(lines.line_of(0), 0);
        assert_eq!(lines.line_of(99), 3);
    }
}
