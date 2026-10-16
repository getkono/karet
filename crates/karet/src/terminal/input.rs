//! What the shell receives for a key, a paste, or a mouse event: the xterm
//! encodings a program running under `TERM=xterm-256color` expects.
//!
//! Pure functions of the event and the emulator's mode flags, so every
//! encoding is unit-tested without a PTY.

use alacritty_terminal::term::TermMode;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

const ESC: u8 = 0x1b;

/// The xterm modifier parameter (`1 + shift + 2·alt + 4·ctrl`), or `None`
/// when no modifier is held.
fn modifier_param(mods: KeyModifiers) -> Option<u8> {
    let mut param = 1;
    if mods.contains(KeyModifiers::SHIFT) {
        param += 1;
    }
    if mods.contains(KeyModifiers::ALT) {
        param += 2;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        param += 4;
    }
    (param > 1).then_some(param)
}

/// A cursor-style key (`ESC [ X`, `ESC O X` in application mode, or
/// `ESC [ 1 ; m X` with modifiers).
fn cursor_key(letter: u8, mods: KeyModifiers, app_cursor: bool) -> Vec<u8> {
    match modifier_param(mods) {
        Some(param) => format!("\x1b[1;{param}{}", letter as char).into_bytes(),
        None if app_cursor => vec![ESC, b'O', letter],
        None => vec![ESC, b'[', letter],
    }
}

/// A tilde key (`ESC [ n ~`, or `ESC [ n ; m ~` with modifiers).
fn tilde_key(number: u8, mods: KeyModifiers) -> Vec<u8> {
    match modifier_param(mods) {
        Some(param) => format!("\x1b[{number};{param}~").into_bytes(),
        None => format!("\x1b[{number}~").into_bytes(),
    }
}

/// The control byte Ctrl+`c` produces, when it has one.
fn control_byte(c: char) -> Option<u8> {
    let c = c.to_ascii_lowercase();
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        '@' | ' ' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '/' | '7' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// Prefix `bytes` with ESC when Alt is held (the "meta sends escape" convention).
fn with_alt(mods: KeyModifiers, mut bytes: Vec<u8>) -> Vec<u8> {
    if mods.contains(KeyModifiers::ALT) {
        bytes.insert(0, ESC);
    }
    bytes
}

/// The bytes a key press sends to the shell under `mode`, or `None` for a key
/// with no terminal encoding (a bare modifier, a media key, …).
#[must_use]
pub(crate) fn encode_key(key: KeyEvent, mode: TermMode) -> Option<Vec<u8>> {
    let mods = key.modifiers;
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let bytes = match key.code {
        KeyCode::Char(c) => {
            if mods.contains(KeyModifiers::CONTROL) {
                with_alt(mods, vec![control_byte(c)?])
            } else {
                let mut buf = [0_u8; 4];
                with_alt(mods, c.encode_utf8(&mut buf).as_bytes().to_vec())
            }
        },
        KeyCode::Enter => with_alt(mods, vec![b'\r']),
        KeyCode::Tab if mods.contains(KeyModifiers::SHIFT) => b"\x1b[Z".to_vec(),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Tab => with_alt(mods, vec![b'\t']),
        KeyCode::Backspace if mods.contains(KeyModifiers::CONTROL) => with_alt(mods, vec![0x08]),
        KeyCode::Backspace => with_alt(mods, vec![0x7f]),
        KeyCode::Esc => with_alt(mods, vec![ESC]),
        KeyCode::Up => cursor_key(b'A', mods, app_cursor),
        KeyCode::Down => cursor_key(b'B', mods, app_cursor),
        KeyCode::Right => cursor_key(b'C', mods, app_cursor),
        KeyCode::Left => cursor_key(b'D', mods, app_cursor),
        KeyCode::Home => cursor_key(b'H', mods, app_cursor),
        KeyCode::End => cursor_key(b'F', mods, app_cursor),
        KeyCode::Insert => tilde_key(2, mods),
        KeyCode::Delete => tilde_key(3, mods),
        KeyCode::PageUp => tilde_key(5, mods),
        KeyCode::PageDown => tilde_key(6, mods),
        KeyCode::F(n @ 1..=4) => {
            let letter = b"PQRS"[usize::from(n - 1)];
            match modifier_param(mods) {
                Some(param) => format!("\x1b[1;{param}{}", letter as char).into_bytes(),
                None => vec![ESC, b'O', letter],
            }
        },
        KeyCode::F(n @ 5..=12) => {
            const NUMBERS: [u8; 8] = [15, 17, 18, 19, 20, 21, 23, 24];
            tilde_key(NUMBERS[usize::from(n - 5)], mods)
        },
        _ => return None,
    };
    Some(bytes)
}

/// The bytes a paste of `text` sends. Line breaks become carriage returns (what
/// the Enter key sends). Under bracketed-paste mode the text is wrapped in the
/// `ESC [ 200 ~` … `ESC [ 201 ~` guards, with every ESC removed from it first
/// so pasted text can never close the bracket early and run as keystrokes.
#[must_use]
pub(crate) fn encode_paste(text: &str, mode: TermMode) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend(text.bytes().filter(|byte| *byte != ESC));
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        text.into_bytes()
    }
}

/// The mouse buttons a terminal program can be told about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MouseButtonKind {
    /// The primary button.
    Left,
    /// The middle button.
    Middle,
    /// The secondary button.
    Right,
    /// No button (plain motion).
    None,
}

/// What happened to the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MouseAction {
    /// A button went down.
    Press(MouseButtonKind),
    /// A button came up.
    Release(MouseButtonKind),
    /// The pointer moved, with this button held (`None` for bare motion).
    Motion(MouseButtonKind),
    /// One wheel notch away from the user.
    WheelUp,
    /// One wheel notch towards the user.
    WheelDown,
}

/// A mouse event in grid coordinates (0-based cell column and row).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MouseReport {
    /// What happened.
    pub(crate) action: MouseAction,
    /// The 0-based grid column.
    pub(crate) column: u16,
    /// The 0-based grid row.
    pub(crate) row: u16,
    /// The modifiers held.
    pub(crate) modifiers: KeyModifiers,
}

/// The report a program that enabled mouse tracking receives for `report`, or
/// `None` when its tracking mode does not cover the event (motion without
/// `MOUSE_MOTION`, a drag without `MOUSE_DRAG`, or no tracking at all).
///
/// SGR encoding (`ESC [ < b ; x ; y M|m`) when the program asked for it, else
/// the legacy X10 encoding, which cannot address cells past column/row 223.
#[must_use]
pub(crate) fn encode_mouse(report: MouseReport, mode: TermMode) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    let button_code = |button: MouseButtonKind| match button {
        MouseButtonKind::Left => 0,
        MouseButtonKind::Middle => 1,
        MouseButtonKind::Right => 2,
        MouseButtonKind::None => 3,
    };
    let (mut code, released) = match report.action {
        MouseAction::Press(MouseButtonKind::None) => return None,
        MouseAction::Press(button) => (button_code(button), false),
        MouseAction::Release(button) => (button_code(button), true),
        MouseAction::Motion(button) => {
            let allowed = if button == MouseButtonKind::None {
                mode.contains(TermMode::MOUSE_MOTION)
            } else {
                mode.intersects(TermMode::MOUSE_MOTION | TermMode::MOUSE_DRAG)
            };
            if !allowed {
                return None;
            }
            (32 + button_code(button), false)
        },
        MouseAction::WheelUp => (64, false),
        MouseAction::WheelDown => (65, false),
    };
    if report.modifiers.contains(KeyModifiers::SHIFT) {
        code += 4;
    }
    if report.modifiers.contains(KeyModifiers::ALT) {
        code += 8;
    }
    if report.modifiers.contains(KeyModifiers::CONTROL) {
        code += 16;
    }
    let column = u32::from(report.column) + 1;
    let row = u32::from(report.row) + 1;
    if mode.contains(TermMode::SGR_MOUSE) {
        let suffix = if released { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{column};{row}{suffix}").into_bytes());
    }
    // X10: a release does not say which button, and coordinates ride as bytes.
    let code = if released { 3 + (code & !3) } else { code };
    if column > 223 || row > 223 {
        return None;
    }
    let byte = |value: u32| u8::try_from(32 + value).ok();
    Some(vec![
        ESC,
        b'[',
        b'M',
        byte(code)?,
        byte(column)?,
        byte(row)?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn plain(code: KeyCode) -> Option<Vec<u8>> {
        encode_key(key(code, KeyModifiers::NONE), TermMode::default())
    }

    #[test]
    fn printable_characters_are_sent_as_utf8() {
        assert_eq!(plain(KeyCode::Char('a')), Some(b"a".to_vec()));
        assert_eq!(
            encode_key(
                key(KeyCode::Char('A'), KeyModifiers::SHIFT),
                TermMode::default()
            ),
            Some(b"A".to_vec())
        );
        assert_eq!(plain(KeyCode::Char('é')), Some("é".as_bytes().to_vec()));
    }

    #[test]
    fn control_letters_and_symbols_map_to_c0_bytes() {
        let ctrl = |c| {
            encode_key(
                key(KeyCode::Char(c), KeyModifiers::CONTROL),
                TermMode::default(),
            )
        };
        assert_eq!(ctrl('c'), Some(vec![0x03]), "Ctrl+C interrupts");
        assert_eq!(ctrl('C'), Some(vec![0x03]), "case does not matter");
        assert_eq!(ctrl('d'), Some(vec![0x04]));
        assert_eq!(ctrl('['), Some(vec![0x1b]));
        assert_eq!(ctrl(' '), Some(vec![0x00]));
        assert_eq!(ctrl('_'), Some(vec![0x1f]));
        assert_eq!(ctrl('`'), None, "no control byte: nothing is sent");
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(
            encode_key(
                key(KeyCode::Char('b'), KeyModifiers::ALT),
                TermMode::default()
            ),
            Some(b"\x1bb".to_vec())
        );
        assert_eq!(
            encode_key(
                key(
                    KeyCode::Char('x'),
                    KeyModifiers::ALT | KeyModifiers::CONTROL
                ),
                TermMode::default()
            ),
            Some(vec![0x1b, 0x18])
        );
    }

    #[test]
    fn editing_keys_use_their_terminal_bytes() {
        assert_eq!(plain(KeyCode::Enter), Some(b"\r".to_vec()));
        assert_eq!(plain(KeyCode::Tab), Some(b"\t".to_vec()));
        assert_eq!(plain(KeyCode::BackTab), Some(b"\x1b[Z".to_vec()));
        assert_eq!(
            encode_key(key(KeyCode::Tab, KeyModifiers::SHIFT), TermMode::default()),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(plain(KeyCode::Backspace), Some(vec![0x7f]));
        assert_eq!(
            encode_key(
                key(KeyCode::Backspace, KeyModifiers::CONTROL),
                TermMode::default()
            ),
            Some(vec![0x08])
        );
        assert_eq!(plain(KeyCode::Esc), Some(vec![0x1b]));
        assert_eq!(plain(KeyCode::Delete), Some(b"\x1b[3~".to_vec()));
        assert_eq!(plain(KeyCode::PageUp), Some(b"\x1b[5~".to_vec()));
    }

    #[test]
    fn arrows_follow_the_cursor_key_mode() {
        assert_eq!(plain(KeyCode::Up), Some(b"\x1b[A".to_vec()));
        let app = TermMode::default() | TermMode::APP_CURSOR;
        assert_eq!(
            encode_key(key(KeyCode::Up, KeyModifiers::NONE), app),
            Some(b"\x1bOA".to_vec()),
            "application cursor mode (vim, less) gets SS3"
        );
        assert_eq!(
            encode_key(key(KeyCode::Left, KeyModifiers::CONTROL), app),
            Some(b"\x1b[1;5D".to_vec()),
            "a modified arrow always carries its parameter"
        );
        assert_eq!(plain(KeyCode::Home), Some(b"\x1b[H".to_vec()));
    }

    #[test]
    fn function_keys_use_ss3_then_tilde_codes() {
        assert_eq!(plain(KeyCode::F(1)), Some(b"\x1bOP".to_vec()));
        assert_eq!(plain(KeyCode::F(5)), Some(b"\x1b[15~".to_vec()));
        assert_eq!(plain(KeyCode::F(12)), Some(b"\x1b[24~".to_vec()));
        assert_eq!(
            encode_key(key(KeyCode::F(2), KeyModifiers::SHIFT), TermMode::default()),
            Some(b"\x1b[1;2Q".to_vec())
        );
        assert_eq!(plain(KeyCode::F(13)), None);
        assert_eq!(plain(KeyCode::CapsLock), None);
    }

    #[test]
    fn a_plain_paste_turns_newlines_into_returns() {
        assert_eq!(encode_paste("a\nb\r\nc", TermMode::default()), b"a\rb\rc");
    }

    #[test]
    fn a_bracketed_paste_is_guarded_and_cannot_close_its_bracket() {
        let mode = TermMode::default() | TermMode::BRACKETED_PASTE;
        assert_eq!(
            encode_paste("ls\n\x1b[201~rm -rf /\n", mode),
            b"\x1b[200~ls\r[201~rm -rf /\r\x1b[201~".to_vec()
        );
    }

    fn report(action: MouseAction, column: u16, row: u16) -> MouseReport {
        MouseReport {
            action,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_reports_need_tracking_enabled() {
        let press = report(MouseAction::Press(MouseButtonKind::Left), 0, 0);
        assert_eq!(encode_mouse(press, TermMode::default()), None);
    }

    #[test]
    fn sgr_mouse_reports_press_release_and_wheel() {
        let mode = TermMode::MOUSE_REPORT_CLICK | TermMode::SGR_MOUSE;
        let encode = |action, column, row| encode_mouse(report(action, column, row), mode);
        assert_eq!(
            encode(MouseAction::Press(MouseButtonKind::Left), 4, 1),
            Some(b"\x1b[<0;5;2M".to_vec())
        );
        assert_eq!(
            encode(MouseAction::Release(MouseButtonKind::Right), 0, 0),
            Some(b"\x1b[<2;1;1m".to_vec())
        );
        assert_eq!(
            encode(MouseAction::WheelDown, 9, 9),
            Some(b"\x1b[<65;10;10M".to_vec())
        );
        assert_eq!(
            encode(MouseAction::Motion(MouseButtonKind::Left), 1, 1),
            None,
            "click tracking alone reports no drags"
        );
        let modified = MouseReport {
            modifiers: KeyModifiers::CONTROL,
            ..report(MouseAction::Press(MouseButtonKind::Left), 0, 0)
        };
        assert_eq!(
            encode_mouse(modified, mode),
            Some(b"\x1b[<16;1;1M".to_vec())
        );
    }

    #[test]
    fn drag_and_motion_follow_their_tracking_modes() {
        let drag = TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let held = report(MouseAction::Motion(MouseButtonKind::Left), 2, 3);
        let bare = report(MouseAction::Motion(MouseButtonKind::None), 2, 3);
        assert_eq!(encode_mouse(held, drag), Some(b"\x1b[<32;3;4M".to_vec()));
        assert_eq!(
            encode_mouse(bare, drag),
            None,
            "drag mode ignores bare motion"
        );
        let motion = TermMode::MOUSE_MOTION | TermMode::SGR_MOUSE;
        assert_eq!(encode_mouse(bare, motion), Some(b"\x1b[<35;3;4M".to_vec()));
    }

    #[test]
    fn legacy_mouse_reports_ride_as_offset_bytes() {
        let mode = TermMode::MOUSE_REPORT_CLICK;
        assert_eq!(
            encode_mouse(
                report(MouseAction::Press(MouseButtonKind::Left), 0, 0),
                mode
            ),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
        assert_eq!(
            encode_mouse(
                report(MouseAction::Release(MouseButtonKind::Left), 0, 0),
                mode
            ),
            Some(vec![0x1b, b'[', b'M', 35, 33, 33]),
            "a legacy release names no button"
        );
        assert_eq!(
            encode_mouse(
                report(MouseAction::Press(MouseButtonKind::Left), 300, 0),
                mode
            ),
            None,
            "past column 223 the legacy encoding cannot address the cell"
        );
    }
}
