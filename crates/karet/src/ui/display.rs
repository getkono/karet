//! The `editor.*` display settings — `lineNumbers`, `cursorLine`, `rulers`,
//! `guides`, `renderWhitespace`, `bracketPairColorization`, the wrap column
//! (`wordWrap` + `wordWrapColumn`), `wrappingIndent`, `scrollOff`,
//! `stickyScroll` and `inlineDiagnostics` — resolved for one
//! pane and handed to its editor widget, with whether the terminal curls a
//! diagnostic's underline.
//!
//! Read from the live settings on every frame, so a configuration reload
//! takes effect on the next paint without any reload-specific plumbing.

use karet_core::Severity;
use karet_editor::Editor;
use karet_editor::InlineDiagnostics;
use karet_editor::LineNumbers;
use karet_editor::RenderWhitespace;
use karet_editor::WrappingIndent;
use karet_session::config::schema;
use karet_session::config::schema::ResolvedEditor;

/// The language-resolved display settings an editor pane paints with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct EditorDisplay<'a> {
    line_numbers: LineNumbers,
    cursor_line: bool,
    rulers: &'a [u16],
    /// Indentation guides, and whether the caret's block is emphasised.
    indent_guides: (bool, bool),
    render_whitespace: RenderWhitespace,
    rainbow_brackets: bool,
    /// The column soft wrapping is capped at, when the mode asks for one.
    wrap_column: Option<u16>,
    wrapping_indent: WrappingIndent,
    scroll_off: u16,
    sticky_scroll: bool,
    /// Which lines show their diagnostics' messages, and the least severe
    /// one shown.
    inline_diagnostics: (InlineDiagnostics, Severity),
    /// Whether a diagnostic's underline is curly: the terminal's, not a
    /// setting, so off until [`with_undercurl`](Self::with_undercurl).
    undercurl: bool,
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
            indent_guides: {
                let guides = resolved.guides();
                (guides.indentation, guides.highlight_active_indentation)
            },
            render_whitespace: match resolved.render_whitespace() {
                schema::RenderWhitespace::None => RenderWhitespace::None,
                schema::RenderWhitespace::Boundary => RenderWhitespace::Boundary,
                schema::RenderWhitespace::Selection => RenderWhitespace::Selection,
                schema::RenderWhitespace::Trailing => RenderWhitespace::Trailing,
                schema::RenderWhitespace::All => RenderWhitespace::All,
            },
            rainbow_brackets: resolved.bracket_pair_colorization(),
            wrap_column: resolved.wrap_column(),
            wrapping_indent: match resolved.wrapping_indent() {
                schema::WrappingIndent::None => WrappingIndent::None,
                schema::WrappingIndent::Same => WrappingIndent::Same,
                schema::WrappingIndent::Indent => WrappingIndent::Indent,
            },
            scroll_off: resolved.scroll_off(),
            sticky_scroll: resolved.sticky_scroll(),
            inline_diagnostics: {
                let inline = resolved.inline_diagnostics();
                let mode = match inline.mode() {
                    schema::InlineDiagnosticsMode::Off => InlineDiagnostics::Off,
                    schema::InlineDiagnosticsMode::CurrentLine => InlineDiagnostics::CurrentLine,
                    schema::InlineDiagnosticsMode::All => InlineDiagnostics::All,
                };
                (mode, inline.min_severity().severity())
            },
            undercurl: false,
        }
    }

    /// These settings, curling a diagnostic's underline when `undercurl`
    /// says the terminal draws one.
    pub(super) fn with_undercurl(self, undercurl: bool) -> Self {
        Self { undercurl, ..self }
    }

    /// Which lines show their diagnostics' messages, and the least severe
    /// diagnostic whose message is shown.
    pub(super) fn inline_diagnostics(self) -> (InlineDiagnostics, Severity) {
        self.inline_diagnostics
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
            .indent_guides(self.indent_guides.0)
            .active_indent_guide(self.indent_guides.1)
            .render_whitespace(self.render_whitespace)
            .rainbow_brackets(self.rainbow_brackets)
            .wrap_column(self.wrap_column)
            .wrapping_indent(self.wrapping_indent)
            .scroll_off(self.scroll_off)
            .sticky_scroll(self.sticky_scroll)
            .undercurl(self.undercurl)
    }

    /// `editor`, a read-only pane that mirrors another's scroll, painting
    /// with the settings that mean something there: the rulers, whether
    /// lines are numbered, and the structure drawn over the text (guides,
    /// whitespace, bracket colours). It has no caret to count relative
    /// numbers from, so they show absolute, no caret to keep a margin
    /// around, and no caret's block to emphasise.
    pub(super) fn apply_read_only<'e>(self, editor: Editor<'e>) -> Editor<'e>
    where
        'a: 'e,
    {
        let numbers = match self.line_numbers {
            LineNumbers::Relative => LineNumbers::Absolute,
            numbers => numbers,
        };
        editor
            .line_numbers(numbers)
            .rulers(self.rulers)
            .indent_guides(self.indent_guides.0)
            .render_whitespace(self.render_whitespace)
            .rainbow_brackets(self.rainbow_brackets)
            .wrap_column(self.wrap_column)
            .wrapping_indent(self.wrapping_indent)
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
        settings.editor.guides = schema::Guides {
            indentation: false,
            highlight_active_indentation: false,
        };
        settings.editor.render_whitespace = schema::RenderWhitespace::Boundary;
        settings.editor.bracket_pair_colorization.enabled = true;
        settings.editor.word_wrap = Some(schema::WordWrap::Bounded);
        settings.editor.word_wrap_column = 72;
        settings.editor.wrapping_indent = schema::WrappingIndent::Indent;
        settings.editor.scroll_off = 5;
        settings.editor.sticky_scroll = false;
        settings.editor.inline_diagnostics = schema::InlineDiagnostics {
            mode: schema::InlineDiagnosticsMode::All,
            min_severity: schema::DiagnosticSeverity::Warning,
        };
        let base = EditorDisplay::resolve(settings.editor.for_language(None));
        assert_eq!(
            base,
            EditorDisplay {
                line_numbers: LineNumbers::Relative,
                cursor_line: false,
                rulers: &[80, 100],
                indent_guides: (false, false),
                render_whitespace: RenderWhitespace::Boundary,
                rainbow_brackets: true,
                wrap_column: Some(72),
                wrapping_indent: WrappingIndent::Indent,
                scroll_off: 5,
                sticky_scroll: false,
                inline_diagnostics: (InlineDiagnostics::All, Severity::Warning),
                undercurl: false,
            }
        );
        assert!(base.with_undercurl(true).undercurl);

        let defaults = Settings::default();
        let defaults = EditorDisplay::resolve(defaults.editor.for_language(None));
        assert_eq!(defaults.line_numbers, LineNumbers::Absolute);
        assert!(defaults.cursor_line);
        assert_eq!(defaults.scroll_off, 3);
        assert!(defaults.rulers.is_empty());
        assert_eq!(defaults.indent_guides, (true, true));
        assert_eq!(defaults.render_whitespace, RenderWhitespace::None);
        assert!(!defaults.rainbow_brackets);
        assert_eq!(defaults.wrap_column, None, "wrapping follows the viewport");
        assert_eq!(defaults.wrapping_indent, WrappingIndent::Same);
        assert_eq!(
            defaults.inline_diagnostics(),
            (InlineDiagnostics::CurrentLine, Severity::Hint)
        );
    }

    #[test]
    fn the_default_settings_draw_indentation_guides_and_the_keys_turn_them_off() {
        use karet_core::LineCol;
        use karet_editor::EditorState;
        use karet_text::TextBuffer;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::widgets::StatefulWidget;

        let buffer = TextBuffer::from_text("{\n    a  \n}\n");
        let area = Rect::new(0, 0, 10, 3);
        let render = |settings: Settings| {
            let display = EditorDisplay::resolve(settings.editor.for_language(None));
            let mut state = EditorState::new();
            state.place_caret(LineCol::new(1, 4));
            let mut target = Buffer::empty(area);
            display
                .apply(Editor::new(&buffer))
                .render(area, &mut target, &mut state);
            (2..9)
                .map(|x| target[(x, 1)].symbol().to_owned())
                .collect::<String>()
        };
        let mut settings = Settings::default();
        settings.editor.line_numbers = schema::LineNumbers::Off;
        assert_eq!(render(settings.clone()), "│   a  ");

        settings.editor.guides.indentation = false;
        settings.editor.render_whitespace = schema::RenderWhitespace::Trailing;
        assert_eq!(render(settings), "    a··");
    }

    #[test]
    fn inline_diagnostics_resolve_per_language_field_by_field() {
        let parsed: Option<schema::Editor> = serde_json::from_str(
            r#"{
                "inlineDiagnostics": { "mode": "all", "minSeverity": "info" },
                "[rust]": { "inlineDiagnostics": { "mode": "off" } },
                "[python]": { "inlineDiagnostics": { "minSeverity": "error" } }
            }"#,
        )
        .ok();
        let Some(editor) = parsed else {
            panic!("the settings parse");
        };
        let resolve =
            |language| EditorDisplay::resolve(editor.for_language(language)).inline_diagnostics();
        assert_eq!(
            resolve(None),
            (InlineDiagnostics::All, Severity::Information)
        );
        assert_eq!(
            resolve(Some("Rust")),
            (InlineDiagnostics::Off, Severity::Information)
        );
        assert_eq!(
            resolve(Some("python")),
            (InlineDiagnostics::All, Severity::Error)
        );
    }

    #[test]
    fn a_read_only_pane_numbers_relative_as_absolute_and_keeps_off_and_rulers() {
        use karet_core::LineCol;
        use karet_editor::EditorState;
        use karet_text::TextBuffer;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::widgets::StatefulWidget;

        let buffer = TextBuffer::from_text("a\nb\nc\nd\n");
        let area = Rect::new(0, 0, 10, 4);
        let render = |numbers: schema::LineNumbers| {
            let mut settings = Settings::default();
            settings.editor.line_numbers = numbers;
            settings.editor.rulers = vec![2];
            let display = EditorDisplay::resolve(settings.editor.for_language(None));
            let mut state = EditorState::new();
            state.place_caret(LineCol::new(2, 0));
            let mut target = Buffer::empty(area);
            display
                .apply_read_only(Editor::new(&buffer))
                .render(area, &mut target, &mut state);
            let gutters: Vec<String> = (0..4)
                .map(|y| (0..3).map(|x| target[(x, y)].symbol().to_owned()).collect())
                .collect();
            let ruler = target[(5, 0)].bg;
            (gutters, ruler, target[(4, 0)].bg)
        };

        let (gutters, ruler, beside) = render(schema::LineNumbers::Relative);
        assert_eq!(
            gutters,
            [" 1 ", " 2 ", " 3 ", " 4 "],
            "no caret to count from: absolute numbers, not 2 1 3 1"
        );
        assert_ne!(ruler, beside, "the ruler still tints column 2");

        let (gutters, ..) = render(schema::LineNumbers::Off);
        assert_eq!(gutters, ["  a", "  b", "  c", "  d"], "off stays off");
    }

    #[test]
    fn a_read_only_pane_draws_guides_whitespace_and_bracket_colours() {
        use karet_editor::EditorState;
        use karet_text::TextBuffer;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::widgets::StatefulWidget;

        let buffer = TextBuffer::from_text("{\n    a  \n}\n");
        let area = Rect::new(0, 0, 10, 3);
        let render = |settings: &Settings| {
            let display = EditorDisplay::resolve(settings.editor.for_language(None));
            let mut target = Buffer::empty(area);
            display.apply_read_only(Editor::new(&buffer)).render(
                area,
                &mut target,
                &mut EditorState::new(),
            );
            let text = (2..9)
                .map(|x| target[(x, 1)].symbol().to_owned())
                .collect::<String>();
            (text, target[(2, 0)].fg)
        };
        let mut settings = Settings::default();
        settings.editor.line_numbers = schema::LineNumbers::Off;
        let (text, plain_brace) = render(&settings);
        assert_eq!(text, "│   a  ", "the default guides reach a read-only pane");

        settings.editor.render_whitespace = schema::RenderWhitespace::Trailing;
        settings.editor.bracket_pair_colorization.enabled = true;
        let (text, brace) = render(&settings);
        assert_eq!(text, "│   a··", "and so does visible whitespace");
        assert_ne!(brace, plain_brace, "and the bracket colours");

        settings.editor.guides.indentation = false;
        assert_eq!(render(&settings).0, "    a··", "guides off stays off");
    }
}
