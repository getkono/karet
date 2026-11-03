//! Completion candidates: what a producer offers at a position, how each one
//! edits the document when accepted, and what it can fill in later on demand.

use super::Markup;
use crate::coord::Range;
use crate::edit::TextEdit;

/// The kind of a [`CompletionItem`] (drives its icon).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum CompletionKind {
    /// Plain text.
    Text,
    /// A method.
    Method,
    /// A function.
    Function,
    /// A field.
    Field,
    /// A variable.
    Variable,
    /// A class.
    Class,
    /// An interface.
    Interface,
    /// A module.
    Module,
    /// A property.
    Property,
    /// A keyword.
    Keyword,
    /// A snippet.
    Snippet,
    /// A constant.
    Constant,
    /// A struct.
    Struct,
    /// An enum.
    Enum,
    /// Forward-compatibility fallback: an unrecognized value from a newer peer.
    #[cfg_attr(feature = "serde", serde(other))]
    Unknown,
}

/// A completion candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CompletionItem {
    /// The label shown in the list.
    pub label: String,
    /// The completion kind.
    pub kind: CompletionKind,
    /// Optional detail (e.g. a type or signature).
    pub detail: Option<String>,
    /// Optional documentation.
    pub documentation: Option<Markup>,
    /// The text inserted when no `edit` is supplied.
    pub insert_text: String,
    /// A precise edit to apply instead of `insert_text`. Its range is the one an
    /// accept *inserts* over: up to the position the candidates were asked for.
    pub edit: Option<TextEdit>,
    /// A sort key overriding `label`.
    pub sort_text: Option<String>,
    /// Whether the item is deprecated.
    pub deprecated: bool,
    /// The text the typed prefix is matched against, when it is not the label.
    #[cfg_attr(feature = "serde", serde(default))]
    pub filter_text: Option<String>,
    /// The range an accept *replaces* instead of [`edit`](Self::edit)'s, when
    /// the producer offered both: it also covers the rest of the word after
    /// the position the candidates were asked for. Same text, same line.
    #[cfg_attr(feature = "serde", serde(default))]
    pub replace_range: Option<Range>,
    /// Edits elsewhere in the document that accepting also applies -- an
    /// import the candidate needs, say. They never overlap [`edit`](Self::edit)
    /// or each other.
    #[cfg_attr(feature = "serde", serde(default))]
    pub additional_edits: Vec<TextEdit>,
    /// The characters that, typed while this item is selected, accept it before
    /// they are inserted. `None` defers to the producer's list-wide set.
    #[cfg_attr(feature = "serde", serde(default))]
    pub commit_characters: Option<Vec<char>>,
    /// The producer's own encoding of the item, opaque to everyone else.
    ///
    /// Handed back to the producer to fill in what it deferred (documentation,
    /// detail, additional edits); `None` when there is nothing left to fill in.
    #[cfg_attr(feature = "serde", serde(default))]
    pub payload: Option<String>,
    /// The snippet stops in the inserted text -- [`edit`](Self::edit)'s
    /// `new_text` where there is an edit, else
    /// [`insert_text`](Self::insert_text) -- in the order a snippet session
    /// visits them, the final stop (`$0`) last. Empty for an item that is
    /// plain text: the inserted text is then all there is, the caret after it.
    ///
    /// The inserted text is already what a snippet shows before anyone edits
    /// it (placeholders filled in, the first choice chosen), so a consumer
    /// with no snippet session can ignore this and insert the text as is.
    #[cfg_attr(feature = "serde", serde(default))]
    pub tabstops: Vec<Tabstop>,
}

/// One stop of a snippet: where a session puts the caret (or carets) next.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Tabstop {
    /// The snippet's number for it (`$1` is 1); 0 is the final stop.
    pub index: u32,
    /// Where it sits in the inserted text, as `[start, end)` character
    /// offsets, in text order: one span per occurrence, so a stop that is
    /// mirrored has several, all holding the same text. An empty span is a
    /// bare caret position; a non-empty one is a placeholder, selected when
    /// the session reaches it.
    pub spans: Vec<(u32, u32)>,
    /// The options a choice stop (`${1|one,two|}`) offers, the first of them
    /// already inserted; empty for any other stop.
    pub choices: Vec<String>,
}

impl CompletionItem {
    /// A plain candidate: `label`, inserted as itself, with nothing else.
    #[must_use]
    pub fn new(label: impl Into<String>, kind: CompletionKind) -> Self {
        let label = label.into();
        Self {
            insert_text: label.clone(),
            label,
            kind,
            detail: None,
            documentation: None,
            edit: None,
            sort_text: None,
            deprecated: false,
            filter_text: None,
            replace_range: None,
            additional_edits: Vec::new(),
            commit_characters: None,
            payload: None,
            tabstops: Vec::new(),
        }
    }

    /// The text a typed prefix is matched against: the filter text when the
    /// producer gave one, else the label.
    #[must_use]
    pub fn filter_key(&self) -> &str {
        self.filter_text.as_deref().unwrap_or(&self.label)
    }

    /// Whether typing `ch` while this item is selected accepts it: the item's
    /// own commit characters when it has them, else `defaults`, the producer's
    /// list-wide set.
    #[must_use]
    pub fn commits_on(&self, ch: char, defaults: &[char]) -> bool {
        self.commit_characters
            .as_deref()
            .unwrap_or(defaults)
            .contains(&ch)
    }

    /// Take what a resolve filled in from `resolved`: its detail and
    /// documentation where it has them, and its additional edits where this
    /// item has none. What an accept inserts is kept -- a resolve fills an item
    /// in, it does not change what it does -- and the payload is spent, so the
    /// item is not resolved again.
    pub fn adopt_resolved(&mut self, resolved: Self) {
        if resolved.detail.is_some() {
            self.detail = resolved.detail;
        }
        if resolved.documentation.is_some() {
            self.documentation = resolved.documentation;
        }
        if self.additional_edits.is_empty() {
            self.additional_edits = resolved.additional_edits;
        }
        self.payload = None;
    }
}

/// A producer's answer to a completion request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CompletionList {
    /// The candidates.
    pub items: Vec<CompletionItem>,
    /// Whether the producer left candidates out, so narrowing the typed prefix
    /// should ask again ([`CompletionTrigger::Incomplete`]) rather than only
    /// filter these.
    pub incomplete: bool,
}

/// Why candidates are being asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CompletionTrigger {
    /// The user asked (Ctrl+Space), or typed an identifier character.
    #[default]
    Invoked,
    /// The user typed one of the producer's trigger characters.
    Character(char),
    /// The user kept typing over an incomplete list.
    Incomplete,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MarkupKind;
    use crate::coord::LineCol;

    fn edit(line: u32, text: &str) -> TextEdit {
        TextEdit {
            range: Range {
                start: LineCol::new(line, 0),
                end: LineCol::new(line, 0),
            },
            new_text: text.to_owned(),
        }
    }

    #[test]
    fn a_new_item_inserts_its_label_and_filters_on_it() {
        let item = CompletionItem::new("push", CompletionKind::Method);
        assert_eq!(item.insert_text, "push");
        assert_eq!(item.filter_key(), "push");
        let filtered = CompletionItem {
            filter_text: Some("vec_push".to_owned()),
            ..item
        };
        assert_eq!(filtered.filter_key(), "vec_push");
    }

    /// An item's own commit characters replace the list-wide set rather than
    /// adding to it, and an explicitly empty set commits on nothing.
    #[test]
    fn an_items_commit_characters_override_the_defaults() {
        let item = CompletionItem::new("x", CompletionKind::Variable);
        assert!(item.commits_on('.', &['.', ';']));
        assert!(!item.commits_on('(', &['.', ';']));
        let own = CompletionItem {
            commit_characters: Some(vec!['(']),
            ..item.clone()
        };
        assert!(own.commits_on('(', &['.']));
        assert!(!own.commits_on('.', &['.']));
        let none = CompletionItem {
            commit_characters: Some(Vec::new()),
            ..item
        };
        assert!(!none.commits_on('.', &['.']));
    }

    /// Falsified by a resolve overwriting what an accept inserts, clearing a
    /// detail it did not send, or leaving the payload to be resolved again.
    #[test]
    fn adopting_a_resolved_item_fills_in_and_spends_the_payload() {
        let mut held = CompletionItem {
            detail: Some("fn()".to_owned()),
            payload: Some("{}".to_owned()),
            ..CompletionItem::new("push", CompletionKind::Method)
        };
        let docs = Markup {
            kind: MarkupKind::Markdown,
            value: "Appends".to_owned(),
        };
        held.adopt_resolved(CompletionItem {
            documentation: Some(docs.clone()),
            additional_edits: vec![edit(0, "use std::vec::Vec;\n")],
            insert_text: "other".to_owned(),
            tabstops: vec![Tabstop {
                index: 0,
                spans: vec![(0, 0)],
                choices: Vec::new(),
            }],
            payload: Some("{\"again\":1}".to_owned()),
            ..CompletionItem::new("push", CompletionKind::Method)
        });
        assert_eq!(held.detail.as_deref(), Some("fn()"));
        assert_eq!(held.documentation, Some(docs));
        assert_eq!(held.additional_edits.len(), 1);
        assert_eq!(held.insert_text, "push");
        assert!(
            held.tabstops.is_empty(),
            "the stops belong to what is inserted"
        );
        assert_eq!(held.payload, None);
    }

    /// Additional edits that came with the list are the producer's word;
    /// a resolve answer does not replace them.
    #[test]
    fn eager_additional_edits_survive_a_resolve() {
        let mut held = CompletionItem {
            additional_edits: vec![edit(0, "a")],
            ..CompletionItem::new("x", CompletionKind::Text)
        };
        held.adopt_resolved(CompletionItem {
            additional_edits: vec![edit(1, "b")],
            ..CompletionItem::new("x", CompletionKind::Text)
        });
        assert_eq!(held.additional_edits, vec![edit(0, "a")]);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn an_item_from_an_older_peer_reads_with_nothing_extra() -> Result<(), serde_json::Error> {
        let read: CompletionItem = serde_json::from_str(
            r#"{"label":"a","kind":"Text","detail":null,"documentation":null,
                "insert_text":"a","edit":null,"sort_text":null,"deprecated":false}"#,
        )?;
        assert_eq!(read, CompletionItem::new("a", CompletionKind::Text));
        let snippet = CompletionItem {
            insert_text: "push(ch)".to_owned(),
            tabstops: vec![
                Tabstop {
                    index: 1,
                    spans: vec![(5, 7)],
                    choices: vec!["ch".to_owned(), "c".to_owned()],
                },
                Tabstop {
                    index: 0,
                    spans: vec![(8, 8)],
                    choices: Vec::new(),
                },
            ],
            ..CompletionItem::new("push", CompletionKind::Method)
        };
        let list = CompletionList {
            items: vec![read, snippet],
            incomplete: true,
        };
        let back: CompletionList = serde_json::from_str(&serde_json::to_string(&list)?)?;
        assert_eq!(back, list);
        let trigger: CompletionTrigger =
            serde_json::from_str(&serde_json::to_string(&CompletionTrigger::Character('.'))?)?;
        assert_eq!(trigger, CompletionTrigger::Character('.'));
        Ok(())
    }
}
