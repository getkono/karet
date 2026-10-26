//! Inline completions: text a producer proposes to insert at the caret, shown
//! as "ghost text" the user can accept whole, accept a word at a time, or
//! ignore by typing on.
//!
//! An item is positioned against the text it was asked for. The user keeps
//! typing while the producer thinks, so an item is never taken as current by
//! its version alone: [`InlineCompletionItem::ghost`] re-reads it against the
//! line as it is now and keeps it only while what was typed since is a prefix
//! of what it proposes, which is the rule LSP 3.18 states for
//! `textDocument/inlineCompletion`.

use super::VirtualLine;
use super::VirtualText;
use crate::coord::LineCol;
use crate::coord::Range;
use crate::edit::TextEdit;
use crate::token::ThemeRole;

/// Why inline completions are being asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum InlineCompletionTrigger {
    /// The user asked: a producer may offer several items to cycle through.
    Invoked,
    /// The editor asked on its own while the user typed: one item is enough.
    #[default]
    Automatic,
    /// Forward-compatibility fallback: an unrecognized value from a newer peer.
    #[cfg_attr(feature = "serde", serde(other))]
    Unknown,
}

/// One proposed insertion.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InlineCompletionItem {
    /// The text that replaces [`range`](Self::range), with `\n` line breaks
    /// and no snippet syntax: what is shown is what an accept inserts.
    pub insert_text: String,
    /// The span of one line the text replaces: the empty span at the
    /// position the items were asked for when the producer named none.
    pub range: Range,
    /// The text what was typed is matched against, when it is not
    /// [`insert_text`](Self::insert_text).
    #[cfg_attr(feature = "serde", serde(default))]
    pub filter_text: Option<String>,
    /// The producer's own encoding of the item, opaque to everyone else:
    /// handed back to it when the item is accepted. `None` when the producer
    /// needs nothing back.
    #[cfg_attr(feature = "serde", serde(default))]
    pub payload: Option<String>,
}

/// What an [`InlineCompletionItem`] still proposes at a caret: the text after
/// what has been typed, and the existing text an accept replaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineGhost {
    /// The caret it is shown at.
    pub at: LineCol,
    /// The text still to insert, with `\n` line breaks.
    pub text: String,
    /// The column on the caret's line up to which existing text is replaced
    /// by a whole accept: the caret's own column when nothing is.
    pub replace_end: u32,
}

impl InlineCompletionItem {
    /// An item inserting `text` at `at`.
    #[must_use]
    pub fn new(text: impl Into<String>, at: LineCol) -> Self {
        Self {
            insert_text: text.into(),
            range: Range { start: at, end: at },
            ..Self::default()
        }
    }

    /// What this item still proposes on a line reading `line` with the caret
    /// at `caret`.
    ///
    /// `None` when it no longer applies: its range is on another line or
    /// spans lines, the caret has left the span it replaces from, or what was
    /// typed between the span's start and the caret is not a prefix of what
    /// it proposes (nor of its filter text) -- or when nothing would be left
    /// to insert or replace.
    #[must_use]
    pub fn ghost(&self, line: &str, caret: LineCol) -> Option<InlineGhost> {
        let range = self.range;
        if range.start.line != caret.line
            || range.end.line != caret.line
            || range.start.col > caret.col
            || range.start.col > range.end.col
        {
            return None;
        }
        let chars: Vec<char> = line.chars().collect();
        let start = range.start.col as usize;
        let at = caret.col as usize;
        if at > chars.len() {
            return None;
        }
        let typed: String = chars[start..at].iter().collect();
        let filter = self.filter_text.as_deref().unwrap_or(&self.insert_text);
        if !filter.starts_with(&typed) || !self.insert_text.starts_with(&typed) {
            return None;
        }
        let text = self.insert_text[typed.len()..].to_owned();
        let replace_end = range.end.col.max(caret.col).min(chars.len() as u32);
        if text.is_empty() && replace_end == caret.col {
            return None;
        }
        Some(InlineGhost {
            at: caret,
            text,
            replace_end,
        })
    }
}

impl InlineGhost {
    /// The edit a whole accept applies: the rest of the text, over whatever
    /// existing text the item replaces after the caret.
    #[must_use]
    pub fn accept(&self) -> TextEdit {
        TextEdit {
            range: Range {
                start: self.at,
                end: LineCol::new(self.at.line, self.replace_end),
            },
            new_text: self.text.clone(),
        }
    }

    /// The edit accepting only the next word applies: an insertion at the
    /// caret, replacing nothing, or [`accept`](Self::accept) when the word is
    /// all that is left.
    ///
    /// The next word is the run of spaces and tabs before it plus one run of
    /// word characters (letters, digits, `_`), or one other character. A line
    /// break comes with the indentation after it, so accepting the end of a
    /// line also lands the caret where the next line's text starts.
    #[must_use]
    pub fn accept_word(&self) -> TextEdit {
        let len = next_word_len(&self.text);
        if len >= self.text.len() {
            return self.accept();
        }
        TextEdit {
            range: Range {
                start: self.at,
                end: self.at,
            },
            new_text: self.text[..len].to_owned(),
        }
    }

    /// The text shown on the caret's own row, at the caret: the ghost's first
    /// line, less the existing text it replaces where it ends with exactly
    /// that text, which then reads on after it as it already does.
    ///
    /// `None` when the first line has nothing to show. Tabs are expanded to
    /// `tab_width` spaces, since a renderer paints virtual text as is.
    #[must_use]
    pub fn virtual_text(&self, line: &str, role: ThemeRole, tab_width: u8) -> Option<VirtualText> {
        let first = self.text.split('\n').next().unwrap_or_default();
        let replaced: String = line
            .chars()
            .skip(self.at.col as usize)
            .take(self.replace_end.saturating_sub(self.at.col) as usize)
            .collect();
        let shown = if !self.text.contains('\n') {
            first.strip_suffix(replaced.as_str()).unwrap_or(first)
        } else {
            first
        };
        (!shown.is_empty()).then(|| {
            VirtualText::inline(
                self.at.line,
                self.at.col,
                expand_tabs(shown, tab_width),
                role,
            )
        })
    }

    /// The ghost's further lines, each a row below the caret's line, in order.
    #[must_use]
    pub fn virtual_lines(&self, role: ThemeRole, tab_width: u8) -> Vec<VirtualLine> {
        self.text
            .split('\n')
            .skip(1)
            .map(|text| VirtualLine::below(self.at.line, expand_tabs(text, tab_width), role))
            .collect()
    }
}

/// The byte length of the next word of `text`, as [`InlineGhost::accept_word`]
/// defines it.
fn next_word_len(text: &str) -> usize {
    if let Some(rest) = text.strip_prefix('\n') {
        let indent = rest
            .find(|ch: char| ch != ' ' && ch != '\t')
            .unwrap_or(rest.len());
        return 1 + indent;
    }
    let mut chars = text.char_indices().peekable();
    while chars.peek().is_some_and(|&(_, ch)| ch == ' ' || ch == '\t') {
        chars.next();
    }
    let Some(&(start, first)) = chars.peek() else {
        return text.len();
    };
    if first == '\n' {
        return start;
    }
    if !is_word(first) {
        return start + first.len_utf8();
    }
    chars
        .find(|&(_, ch)| !is_word(ch))
        .map_or(text.len(), |(end, _)| end)
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

fn expand_tabs(text: &str, tab_width: u8) -> String {
    text.replace('\t', &" ".repeat(usize::from(tab_width.max(1))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(text: &str, (line, start, end): (u32, u32, u32)) -> InlineCompletionItem {
        InlineCompletionItem {
            range: Range {
                start: LineCol::new(line, start),
                end: LineCol::new(line, end),
            },
            ..InlineCompletionItem::default()
        }
        .with_text(text)
    }

    impl InlineCompletionItem {
        fn with_text(self, text: &str) -> Self {
            Self {
                insert_text: text.to_owned(),
                ..self
            }
        }
    }

    fn ghost_text(ghost: Option<InlineGhost>) -> Option<String> {
        ghost.map(|ghost| ghost.text)
    }

    #[test]
    fn an_insertion_shows_whole_at_the_caret_it_was_asked_for() {
        let at = LineCol::new(2, 4);
        let ghost = InlineCompletionItem::new("fn main() {}", at).ghost("    ", at);
        assert_eq!(
            ghost,
            Some(InlineGhost {
                at,
                text: "fn main() {}".to_owned(),
                replace_end: 4,
            })
        );
    }

    /// Typing on what the item proposes narrows it; typing anything else
    /// retires it. An insertion asked for before the typing narrows the same
    /// way: its empty span starts where the typing did.
    #[test]
    fn typing_narrows_the_ghost_until_it_diverges() {
        let proposal = item("let x = 1;", (0, 0, 3));
        assert_eq!(
            ghost_text(proposal.ghost("let x", LineCol::new(0, 5))),
            Some(" = 1;".to_owned())
        );
        assert_eq!(
            proposal.ghost("lex", LineCol::new(0, 3)),
            None,
            "a typed character the item does not propose retires it"
        );
        assert_eq!(
            proposal.ghost("let x = 1;", LineCol::new(0, 10)),
            None,
            "nothing is left once everything was typed"
        );
        let insertion = InlineCompletionItem::new("x = 1;", LineCol::new(0, 4));
        assert_eq!(
            ghost_text(insertion.ghost("let x ", LineCol::new(0, 6))),
            Some("= 1;".to_owned())
        );
    }

    #[test]
    fn an_item_out_of_reach_of_the_caret_is_not_shown() {
        let proposal = item("abc", (1, 4, 4));
        assert_eq!(proposal.ghost("    ", LineCol::new(2, 4)), None);
        assert_eq!(proposal.ghost("    ", LineCol::new(1, 2)), None);
        let spanning = InlineCompletionItem {
            range: Range {
                start: LineCol::new(1, 0),
                end: LineCol::new(2, 0),
            },
            ..InlineCompletionItem::default()
        }
        .with_text("abc");
        assert_eq!(spanning.ghost("    ", LineCol::new(1, 4)), None);
        // A caret past the line's end (a stale position) is no prefix at all.
        let stale = LineCol::new(1, 9);
        assert_eq!(
            InlineCompletionItem::new("x", stale).ghost("ab", stale),
            None
        );
    }

    #[test]
    fn a_filter_text_must_also_match_what_was_typed() {
        let proposal = InlineCompletionItem {
            filter_text: Some("zzz".to_owned()),
            ..item("abc", (0, 0, 0))
        };
        assert_eq!(proposal.ghost("a", LineCol::new(0, 1)), None);
        let matching = InlineCompletionItem {
            filter_text: Some("abc".to_owned()),
            ..proposal
        };
        assert_eq!(
            ghost_text(matching.ghost("a", LineCol::new(0, 1))),
            Some("bc".to_owned())
        );
    }

    /// The common Copilot shape: the whole line is replaced, closing bracket
    /// included, so the bracket the editor auto-closed is not doubled.
    #[test]
    fn a_whole_accept_replaces_the_text_the_item_spans() {
        let line = "call()";
        let caret = LineCol::new(0, 5);
        let ghost = item("call(a, b)", (0, 0, 6)).ghost(line, caret);
        let edit = ghost.as_ref().map(InlineGhost::accept);
        assert_eq!(
            edit,
            Some(TextEdit {
                range: Range {
                    start: caret,
                    end: LineCol::new(0, 6),
                },
                new_text: "a, b)".to_owned(),
            })
        );
        let shown = ghost.and_then(|ghost| ghost.virtual_text(line, ThemeRole::Muted, 4));
        assert_eq!(
            shown.map(|text| text.text),
            Some("a, b".to_owned()),
            "the replaced `)` reads on after the ghost instead of twice"
        );
    }

    #[test]
    fn accepting_a_word_takes_one_word_with_its_leading_space() {
        let at = LineCol::new(0, 0);
        let ghost = InlineGhost {
            at,
            text: "  foo_bar(baz)".to_owned(),
            replace_end: 0,
        };
        assert_eq!(ghost.accept_word().new_text, "  foo_bar");
        let punct = InlineGhost {
            text: "(baz)".to_owned(),
            ..ghost.clone()
        };
        assert_eq!(punct.accept_word().new_text, "(");
        let line_end = InlineGhost {
            text: "\n    next".to_owned(),
            ..ghost.clone()
        };
        assert_eq!(line_end.accept_word().new_text, "\n    ");
        let before_break = InlineGhost {
            text: "  \nnext".to_owned(),
            ..ghost
        };
        assert_eq!(before_break.accept_word().new_text, "  ");
    }

    /// The last word is the whole rest: it must also replace what a whole
    /// accept would, or the replaced bracket would be left doubled.
    #[test]
    fn accepting_the_last_word_is_a_whole_accept() {
        let ghost = InlineGhost {
            at: LineCol::new(0, 5),
            text: "x)".to_owned(),
            replace_end: 6,
        };
        assert_eq!(ghost.accept_word().new_text, "x");
        let last = InlineGhost {
            text: "xy".to_owned(),
            ..ghost
        };
        assert_eq!(last.accept_word(), last.accept());
    }

    #[test]
    fn further_lines_are_rows_below_with_tabs_expanded() {
        let ghost = InlineGhost {
            at: LineCol::new(3, 8),
            text: "{\n\tbody();\n}".to_owned(),
            replace_end: 8,
        };
        let first = ghost.virtual_text("", ThemeRole::Muted, 2);
        assert_eq!(
            first,
            Some(VirtualText::inline(3, 8, "{", ThemeRole::Muted))
        );
        assert_eq!(
            ghost.virtual_lines(ThemeRole::Muted, 2),
            vec![
                VirtualLine::below(3, "  body();", ThemeRole::Muted),
                VirtualLine::below(3, "}", ThemeRole::Muted),
            ]
        );
        let starts_on_next_line = InlineGhost {
            text: "\nnext".to_owned(),
            ..ghost
        };
        assert_eq!(
            starts_on_next_line.virtual_text("", ThemeRole::Muted, 2),
            None
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn an_item_round_trips_and_tolerates_missing_optional_fields() -> Result<(), serde_json::Error>
    {
        let full = InlineCompletionItem {
            payload: Some("{}".to_owned()),
            filter_text: Some("f".to_owned()),
            ..item("abc", (1, 2, 3))
        };
        let back: InlineCompletionItem = serde_json::from_str(&serde_json::to_string(&full)?)?;
        assert_eq!(back, full);
        let sparse: InlineCompletionItem = serde_json::from_str(
            r#"{"insert_text":"x","range":{"start":{"line":0,"col":0},"end":{"line":0,"col":0}}}"#,
        )?;
        assert_eq!(sparse, InlineCompletionItem::new("x", LineCol::default()));
        Ok(())
    }
}
