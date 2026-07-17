//! Semantic block extraction for sticky-scroll and structural navigation.

use std::collections::HashMap;

use karet_treesitter::LanguageId;
use karet_treesitter::Query;
use karet_treesitter::SyntaxTree;
use karet_treesitter::semantic_query;

/// One semantic source block with a header and an enclosing lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemanticBlock {
    /// First 0-based source line of the header.
    pub header_start: u32,
    /// Last 0-based source line occupied by the header or signature.
    pub header_end: u32,
    /// Last 0-based source line for which this header remains the active context.
    pub scope_end: u32,
}

impl SemanticBlock {
    /// Whether `line` is inside the block after its header has scrolled away.
    #[must_use]
    pub fn active_at(self, line: u32) -> bool {
        self.header_start < line && line <= self.scope_end
    }

    /// Whether the source signature occupies more than one physical line.
    #[must_use]
    pub fn has_multiline_header(self) -> bool {
        self.header_end > self.header_start
    }
}

/// Semantic blocks in document order, outer scopes before inner scopes on ties.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SemanticBlocks {
    blocks: Vec<SemanticBlock>,
}

impl SemanticBlocks {
    /// Build a normalized semantic-block collection from external block data.
    #[must_use]
    pub fn new(mut blocks: Vec<SemanticBlock>) -> Self {
        blocks.sort_unstable_by_key(|block| (block.header_start, u32::MAX - block.scope_end));
        blocks.dedup();
        Self { blocks }
    }

    /// Borrow the extracted blocks.
    #[must_use]
    pub fn blocks(&self) -> &[SemanticBlock] {
        &self.blocks
    }

    /// Return the active outer-to-inner block chain at `line`.
    #[must_use]
    pub fn active_at(&self, line: u32) -> Vec<SemanticBlock> {
        self.blocks
            .iter()
            .copied()
            .filter(|block| block.active_at(line))
            .collect()
    }
}

/// Cached grammar-query runner that derives [`SemanticBlocks`] from syntax trees.
#[derive(Default)]
pub struct SemanticBlocker {
    queries: HashMap<LanguageId, Option<Query>>,
}

impl SemanticBlocker {
    /// Create an empty semantic-query cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Derive semantic blocks for `tree` and its matching `text`.
    ///
    /// A missing or invalid grammar query degrades to an empty model.
    #[must_use]
    pub fn analyze(&mut self, tree: &SyntaxTree, text: &str) -> SemanticBlocks {
        let lang = tree.language();
        self.queries.entry(lang).or_insert_with(|| {
            semantic_query(lang).and_then(|source| Query::compile(lang, source).ok())
        });
        let Some(query) = self.queries.get(&lang).and_then(Option::as_ref) else {
            return SemanticBlocks::default();
        };

        let starts = line_starts(text);
        let names = query.capture_names();
        let mut blocks = Vec::new();
        let mut headings = Vec::new();

        for matched in tree.matches(query, text) {
            let mut scope = None;
            let mut header = None;
            let mut body = None;
            for capture in matched.captures {
                let Some(name) = names.get(capture.capture as usize).copied() else {
                    continue;
                };
                if name == "semantic.scope" {
                    scope = Some(capture.span);
                } else if name == "semantic.header" {
                    header = Some(capture.span);
                } else if name == "semantic.body" {
                    body = Some(capture.span);
                } else if let Some(level) = name
                    .strip_prefix("semantic.heading.")
                    .and_then(|level| level.parse::<u8>().ok())
                {
                    headings.push((
                        row_at(&starts, capture.span.start.0),
                        occupied_end_row(&starts, text, capture.span.end.0),
                        level,
                    ));
                }
            }

            let Some(scope) = scope else { continue };
            let scope_start = row_at(&starts, scope.start.0);
            let scope_end = occupied_end_row(&starts, text, scope.end.0);
            let (header_start, header_end) = if let Some(header) = header {
                (
                    row_at(&starts, header.start.0),
                    occupied_end_row(&starts, text, header.end.0),
                )
            } else {
                let end = body.map_or(scope_start, |body| row_at(&starts, body.start.0));
                (scope_start, end)
            };
            if scope_end > header_start {
                blocks.push(SemanticBlock {
                    header_start,
                    header_end: header_end.max(header_start),
                    scope_end,
                });
            }
        }

        headings.sort_unstable_by_key(|&(start, _, level)| (start, level));
        let last_line = u32::try_from(starts.len().saturating_sub(1)).unwrap_or(u32::MAX);
        for (index, &(header_start, header_end, level)) in headings.iter().enumerate() {
            let scope_end = headings[index + 1..]
                .iter()
                .find(|&&(_, _, next_level)| next_level <= level)
                .map_or(last_line, |&(next_start, _, _)| {
                    next_start.saturating_sub(1)
                });
            if scope_end > header_start {
                blocks.push(SemanticBlock {
                    header_start,
                    header_end,
                    scope_end,
                });
            }
        }

        SemanticBlocks::new(blocks)
    }
}

fn line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(
        text.bytes()
            .enumerate()
            .filter(|(_, byte)| *byte == b'\n')
            .map(|(index, _)| index + 1),
    );
    starts
}

fn row_at(starts: &[usize], byte: usize) -> u32 {
    u32::try_from(
        starts
            .partition_point(|&start| start <= byte)
            .saturating_sub(1),
    )
    .unwrap_or(u32::MAX)
}

fn occupied_end_row(starts: &[usize], text: &str, end: usize) -> u32 {
    let adjusted = if end > 0 && text.as_bytes().get(end - 1) == Some(&b'\n') {
        end - 1
    } else {
        end
    };
    row_at(starts, adjusted)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use karet_treesitter::ParserPool;
    use karet_treesitter::SyntaxTree;
    use karet_treesitter::language_id_from_path;

    use super::*;

    fn analyze(path: &str, text: &str) -> Result<SemanticBlocks, Box<dyn std::error::Error>> {
        let lang = language_id_from_path(Path::new(path)).ok_or("missing grammar")?;
        let mut pool = ParserPool::new();
        let tree = SyntaxTree::parse(&mut pool, lang, text)?;
        Ok(SemanticBlocker::new().analyze(&tree, text))
    }

    #[test]
    fn public_model_reports_active_and_multiline_headers() {
        let block = SemanticBlock {
            header_start: 1,
            header_end: 2,
            scope_end: 8,
        };
        assert!(!block.active_at(1));
        assert!(block.active_at(2));
        assert!(block.has_multiline_header());
        let blocks = SemanticBlocks::new(vec![block]);
        assert_eq!(blocks.blocks(), [block]);
        assert_eq!(blocks.active_at(4), [block]);
    }

    #[test]
    fn markdown_headings_form_a_chain_and_a_second_h1_resets_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let text = "# First\n\n## Child\n\nbody\n\n# Second\n\ntail\n";
        let blocks = analyze("notes.md", text)?;
        assert_eq!(
            blocks.active_at(4),
            [
                SemanticBlock {
                    header_start: 0,
                    header_end: 0,
                    scope_end: 5
                },
                SemanticBlock {
                    header_start: 2,
                    header_end: 2,
                    scope_end: 5
                },
            ]
        );
        assert_eq!(
            blocks.active_at(8),
            [SemanticBlock {
                header_start: 6,
                header_end: 6,
                scope_end: 9
            }]
        );
        Ok(())
    }

    #[test]
    fn java_class_and_multiline_method_are_nested() -> Result<(), Box<dyn std::error::Error>> {
        let text = "class A {\n  void run(\n      int value\n  ) {\n    value++;\n  }\n}\n";
        let blocks = analyze("A.java", text)?;
        let active = blocks.active_at(4);
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].header_start, 0);
        assert_eq!(active[1].header_start, 1);
        assert!(active[1].has_multiline_header());
        Ok(())
    }
}
