//! Shared cursor and selection mechanics for the shell's lightweight text fields.

use std::ops::Range;

use unicode_width::UnicodeWidthChar;

/// Cursor, selection anchor, and horizontal viewport for an app-owned text field.
#[derive(Clone, Debug, Default)]
pub(crate) struct TextFieldState {
    cursor: usize,
    anchor: Option<usize>,
    pub(crate) scroll: u16,
}

impl TextFieldState {
    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }

    pub(crate) fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then(|| anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    pub(crate) fn selected_text<'a>(&self, text: &'a str) -> Option<&'a str> {
        self.selection().map(|range| &text[range])
    }

    pub(crate) fn select_all(&mut self, text: &str) {
        self.anchor = (!text.is_empty()).then_some(0);
        self.cursor = text.len();
    }

    pub(crate) fn set_cursor(&mut self, text: &str, cursor: usize, extend: bool) {
        let cursor = floor_boundary(text, cursor.min(text.len()));
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = cursor;
    }

    pub(crate) fn move_left(&mut self, text: &str, extend: bool) {
        if !extend && let Some(selection) = self.selection() {
            self.set_cursor(text, selection.start, false);
            return;
        }
        let target = text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
        self.set_cursor(text, target, extend);
    }

    pub(crate) fn move_right(&mut self, text: &str, extend: bool) {
        if !extend && let Some(selection) = self.selection() {
            self.set_cursor(text, selection.end, false);
            return;
        }
        let target = text[self.cursor..]
            .chars()
            .next()
            .map_or(text.len(), |character| self.cursor + character.len_utf8());
        self.set_cursor(text, target, extend);
    }

    pub(crate) fn move_word_left(&mut self, text: &str, extend: bool) {
        self.set_cursor(text, previous_word_boundary(text, self.cursor), extend);
    }

    pub(crate) fn move_word_right(&mut self, text: &str, extend: bool) {
        self.set_cursor(text, next_word_boundary(text, self.cursor), extend);
    }

    pub(crate) fn move_start(&mut self, text: &str, document: bool, extend: bool) {
        let target = if document {
            0
        } else {
            text[..self.cursor].rfind('\n').map_or(0, |index| index + 1)
        };
        self.set_cursor(text, target, extend);
    }

    pub(crate) fn move_end(&mut self, text: &str, document: bool, extend: bool) {
        let target = if document {
            text.len()
        } else {
            text[self.cursor..]
                .find('\n')
                .map_or(text.len(), |index| self.cursor + index)
        };
        self.set_cursor(text, target, extend);
    }

    pub(crate) fn insert(&mut self, text: &mut String, inserted: &str) {
        self.delete_selection(text);
        text.insert_str(self.cursor, inserted);
        self.cursor += inserted.len();
    }

    pub(crate) fn backspace(&mut self, text: &mut String, word: bool) {
        if self.delete_selection(text) {
            return;
        }
        let start = if word {
            previous_word_boundary(text, self.cursor)
        } else {
            text[..self.cursor]
                .char_indices()
                .next_back()
                .map_or(self.cursor, |(index, _)| index)
        };
        text.drain(start..self.cursor);
        self.cursor = start;
    }

    pub(crate) fn delete(&mut self, text: &mut String, word: bool) {
        if self.delete_selection(text) {
            return;
        }
        let end = if word {
            next_word_boundary(text, self.cursor)
        } else {
            text[self.cursor..]
                .chars()
                .next()
                .map_or(self.cursor, |character| self.cursor + character.len_utf8())
        };
        text.drain(self.cursor..end);
    }

    pub(crate) fn cut(&mut self, text: &mut String) -> Option<String> {
        let range = self.selection()?;
        let selected = text[range.clone()].to_string();
        text.drain(range.clone());
        self.cursor = range.start;
        self.anchor = None;
        Some(selected)
    }

    pub(crate) fn ensure_cursor_visible(&mut self, text: &str, width: u16) {
        let cursor_col = text[..self.cursor]
            .chars()
            .map(|character| character.width().unwrap_or(0).max(1))
            .sum::<usize>();
        let width = usize::from(width.max(1));
        let mut scroll = usize::from(self.scroll);
        if cursor_col < scroll {
            scroll = cursor_col;
        } else if cursor_col >= scroll + width {
            scroll = cursor_col + 1 - width;
        }
        self.scroll = u16::try_from(scroll).unwrap_or(u16::MAX);
    }

    fn delete_selection(&mut self, text: &mut String) -> bool {
        let Some(range) = self.selection() else {
            // A zero-width Shift motion may leave an anchor at the caret. Editing
            // still starts a fresh insertion, rather than growing a selection after
            // the inserted text.
            self.anchor = None;
            return false;
        };
        text.drain(range.clone());
        self.cursor = range.start;
        self.anchor = None;
        true
    }
}

pub(crate) fn byte_at_cell(text: &str, target: usize) -> usize {
    let mut cell = 0usize;
    for (index, character) in text.char_indices() {
        let width = character.width().unwrap_or(0).max(1);
        if target < cell + width {
            return index;
        }
        cell += width;
    }
    text.len()
}

fn floor_boundary(text: &str, mut index: usize) -> usize {
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn previous_word_boundary(text: &str, cursor: usize) -> usize {
    let mut chars = text[..cursor].char_indices().rev().peekable();
    while chars
        .peek()
        .is_some_and(|(_, character)| character.is_whitespace())
    {
        chars.next();
    }
    let Some((_, first)) = chars.peek().copied() else {
        return 0;
    };
    let class = word_class(first);
    let mut start = cursor;
    while let Some((index, character)) = chars.peek().copied() {
        if word_class(character) != class {
            break;
        }
        start = index;
        chars.next();
    }
    start
}

fn next_word_boundary(text: &str, cursor: usize) -> usize {
    let mut chars = text[cursor..]
        .char_indices()
        .map(|(index, character)| (cursor + index, character))
        .peekable();
    while chars
        .peek()
        .is_some_and(|(_, character)| character.is_whitespace())
    {
        chars.next();
    }
    let Some((_, first)) = chars.peek().copied() else {
        return text.len();
    };
    let class = word_class(first);
    let mut end = cursor;
    while let Some((index, character)) = chars.peek().copied() {
        if word_class(character) != class {
            break;
        }
        end = index + character.len_utf8();
        chars.next();
    }
    end
}

fn word_class(character: char) -> u8 {
    if character.is_whitespace() {
        0
    } else if character.is_alphanumeric() || character == '_' {
        1
    } else {
        2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_replaces_and_word_deletion_respects_utf8() {
        let mut text = "héllo  world.test".to_string();
        let mut state = TextFieldState::default();
        state.set_cursor(&text, 6, false);
        state.move_word_right(&text, true);
        assert_eq!(state.selected_text(&text), Some("  world"));
        state.insert(&mut text, " there");
        assert_eq!(text, "héllo there.test");
        state.backspace(&mut text, true);
        assert_eq!(text, "héllo .test");
    }

    #[test]
    fn cell_hit_testing_accounts_for_wide_characters() {
        assert_eq!(byte_at_cell("a界b", 0), 0);
        assert_eq!(byte_at_cell("a界b", 1), 1);
        assert_eq!(byte_at_cell("a界b", 2), 1);
        assert_eq!(byte_at_cell("a界b", 3), 4);
        assert_eq!(byte_at_cell("a界b", 4), 5);
    }

    #[test]
    fn typing_after_a_zero_width_shift_motion_does_not_select_the_insert() {
        let mut text = String::new();
        let mut state = TextFieldState::default();
        state.move_left(&text, true);
        state.insert(&mut text, "x");
        assert_eq!(text, "x");
        assert!(state.selection().is_none());
    }
}
