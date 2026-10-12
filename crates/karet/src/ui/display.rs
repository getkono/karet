//! The `editor.*` display settings — `lineNumbers`, `cursorLine`, `rulers`,
//! `scrollOff` and `stickyScroll` — resolved for one pane and handed to its
//! editor widget.
//!
//! Read from the live settings on every frame, so a configuration reload
//! takes effect on the next paint without any reload-specific plumbing.

use karet_editor::Editor;
use karet_editor::LineNumbers;
use karet_session::config::schema;
use karet_session::config::schema::ResolvedEditor;

/// The language-resolved display settings an editor pane paints with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EditorDisplay<'a> {
    line_numbers: LineNumbers,
    cursor_line: bool,
    rulers: &'a [u16],
    scroll_off: u16,
    sticky_scroll: bool,
}

impl<'a> EditorDisplay<'a> {
    /// The display settings `resolved` holds for one language.
    pub(super) fn resolve(resolved: ResolvedEditor<'a>) -> Self {
        Self {
            line_numbers: match resolved.line_numbers() {
                schema::LineNumbers::On => LineNumbers::Absolute,
                schema::LineNumbers::Relative => LineNumbers::Relative,
                schema::LineNumbers::Off => LineNumbers::Off,
            },
            cursor_line: resolved.cursor_line(),
            rulers: resolved.rulers(),
            scroll_off: resolved.scroll_off(),
            sticky_scroll: resolved.sticky_scroll(),
        }
    }

    /// `editor` painting with every one of these settings.
    pub(super) fn apply<'e>(self, editor: Editor<'e>) -> Editor<'e>
    where
        'a: 'e,
    {
        editor
            .line_numbers(self.line_numbers)
            .cursor_line(self.cursor_line)
            .rulers(self.rulers)
            .scroll_off(self.scroll_off)
            .sticky_scroll(self.sticky_scroll)
    }

    /// `editor`, a read-only pane that mirrors another's scroll, painting
    /// with the settings that mean something there: the rulers and whether
    /// lines are numbered. It has no caret to count relative numbers from,
    /// so they show absolute, and no caret to keep a margin around.
    pub(super) fn apply_read_only<'e>(self, editor: Editor<'e>) -> Editor<'e>
    where
        'a: 'e,
    {
        let numbers = match self.line_numbers {
            LineNumbers::Relative => LineNumbers::Absolute,
            numbers => numbers,
        };
        editor.line_numbers(numbers).rulers(self.rulers)
    }
}

#[cfg(test)]
mod tests {
    use karet_session::config::schema::Settings;

    use super::*;

    #[test]
    fn every_setting_is_resolved_and_the_defaults_match_the_schema() {
        let mut settings = Settings::default();
        settings.editor.line_numbers = schema::LineNumbers::Relative;
        settings.editor.cursor_line = false;
        settings.editor.rulers = vec![80, 100];
        settings.editor.scroll_off = 5;
        settings.editor.sticky_scroll = false;
        let base = EditorDisplay::resolve(settings.editor.for_language(None));
        assert_eq!(
            base,
            EditorDisplay {
                line_numbers: LineNumbers::Relative,
                cursor_line: false,
                rulers: &[80, 100],
                scroll_off: 5,
                sticky_scroll: false,
            }
        );

        let defaults = Settings::default();
        let defaults = EditorDisplay::resolve(defaults.editor.for_language(None));
        assert_eq!(defaults.line_numbers, LineNumbers::Absolute);
        assert!(defaults.cursor_line);
        assert_eq!(defaults.scroll_off, 3);
        assert!(defaults.rulers.is_empty());
    }
}
