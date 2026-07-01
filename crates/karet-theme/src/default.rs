//! The built-in dark theme palette (Tokyo-Night-flavored).

use karet_core::StandardToken;
use karet_core::ThemeRole;

use crate::ROLE_COUNT;
use crate::Rgba;
use crate::TOKEN_COUNT;
use crate::Theme;
use crate::is_dark_color;

const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    Rgba { r, g, b, a: 255 }
}

/// The default dark theme.
pub(crate) fn dark() -> Theme {
    let fg = rgb(0xc0, 0xca, 0xf5);
    let bg = rgb(0x1a, 0x1b, 0x26);

    let mut tokens = [fg; TOKEN_COUNT];
    let mut tok = |t: StandardToken, c: Rgba| tokens[t.id().0 as usize] = c;
    tok(StandardToken::Keyword, rgb(0xbb, 0x9a, 0xf7));
    tok(StandardToken::KeywordControl, rgb(0xbb, 0x9a, 0xf7));
    tok(StandardToken::Function, rgb(0x7a, 0xa2, 0xf7));
    tok(StandardToken::FunctionBuiltin, rgb(0x7a, 0xa2, 0xf7));
    tok(StandardToken::Method, rgb(0x7a, 0xa2, 0xf7));
    tok(StandardToken::Type, rgb(0x2a, 0xc3, 0xde));
    tok(StandardToken::TypeBuiltin, rgb(0x2a, 0xc3, 0xde));
    tok(StandardToken::Variable, rgb(0xc0, 0xca, 0xf5));
    tok(StandardToken::Parameter, rgb(0xe0, 0xaf, 0x68));
    tok(StandardToken::Property, rgb(0x7d, 0xcf, 0xff));
    tok(StandardToken::Constant, rgb(0xff, 0x9e, 0x64));
    tok(StandardToken::String, rgb(0x9e, 0xce, 0x6a));
    tok(StandardToken::StringEscape, rgb(0x89, 0xdd, 0xff));
    tok(StandardToken::Comment, rgb(0x56, 0x5f, 0x89));
    tok(StandardToken::Number, rgb(0xff, 0x9e, 0x64));
    tok(StandardToken::Boolean, rgb(0xff, 0x9e, 0x64));
    tok(StandardToken::Operator, rgb(0x89, 0xdd, 0xff));
    tok(StandardToken::Punctuation, rgb(0x9a, 0xa5, 0xce));
    tok(StandardToken::Tag, rgb(0xf7, 0x76, 0x8e));
    tok(StandardToken::Attribute, rgb(0xe0, 0xaf, 0x68));
    tok(StandardToken::Namespace, rgb(0x73, 0xda, 0xca));
    tok(StandardToken::Label, rgb(0x89, 0xdd, 0xff));

    let mut roles = [fg; ROLE_COUNT];
    let mut role = |r: ThemeRole, c: Rgba| roles[r as usize] = c;
    role(ThemeRole::Background, bg);
    role(ThemeRole::Foreground, rgb(0xc0, 0xca, 0xf5));
    role(ThemeRole::CursorLine, rgb(0x1f, 0x23, 0x35));
    role(ThemeRole::Cursor, rgb(0xc0, 0xca, 0xf5));
    role(ThemeRole::Selection, rgb(0x28, 0x34, 0x57));
    role(ThemeRole::LineNumber, rgb(0x3b, 0x42, 0x61));
    role(ThemeRole::LineNumberActive, rgb(0x73, 0x7a, 0xa2));
    role(ThemeRole::IndentGuide, rgb(0x23, 0x24, 0x33));
    role(ThemeRole::MatchingBracket, rgb(0x54, 0x5c, 0x7e));
    role(ThemeRole::StatusBarBackground, rgb(0x16, 0x16, 0x1e));
    role(ThemeRole::StatusBarForeground, rgb(0xa9, 0xb1, 0xd6));
    role(ThemeRole::DiagnosticError, rgb(0xf7, 0x76, 0x8e));
    role(ThemeRole::DiagnosticWarning, rgb(0xe0, 0xaf, 0x68));
    role(ThemeRole::DiagnosticInfo, rgb(0x7d, 0xcf, 0xff));
    role(ThemeRole::DiagnosticHint, rgb(0x1a, 0xbc, 0x9c));
    role(ThemeRole::DiffAdded, rgb(0x1e, 0x33, 0x28));
    role(ThemeRole::DiffRemoved, rgb(0x33, 0x18, 0x1c));
    role(ThemeRole::DiffModified, rgb(0x1f, 0x2a, 0x40));
    role(ThemeRole::SearchMatch, rgb(0x3d, 0x59, 0xa1));
    role(ThemeRole::Breakpoint, rgb(0xdb, 0x4b, 0x4b));

    Theme {
        tokens,
        fallback_fg: fg,
        roles,
        dark: is_dark_color(bg),
    }
}
