//! Centralized terminal capability detection.
//!
//! karet targets modern terminals and probes for a handful of features at startup:
//! the kitty keyboard protocol (a hard requirement), the kitty graphics protocol
//! (drives image rendering and the optional graphical caret), and OSC 22 mouse
//! pointer-shape hints, and reads the colour depth from the environment.
//! Historically these probes were scattered inline through `app::run`; this
//! module owns the single implementation so both the app and the `--doctor`
//! diagnostics consume the same primitive.
//!
//! The env-var graphics heuristic itself lives in [`karet_fileview::image`] (a
//! published library API); this module reuses it and layers the runtime handshakes
//! on top.

use std::io::Write;
use std::time::Duration;

use karet_fileview::image::GraphicsProtocol;
use karet_fileview::image::{self};

use crate::color_depth::ColorDepth;

/// Timeout for a single terminal query/response handshake. A terminal that has not
/// answered within this window is treated as not supporting the queried feature.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_millis(200);

/// A point-in-time snapshot of the terminal features karet cares about.
///
/// This is plain data — the impure probing lives in [`probe_all`] and the individual
/// `probe_*` functions, keeping the derived predicates ([`effective_graphics`],
/// [`kitty_graphics_supported`]) pure and unit-testable.
///
/// [`effective_graphics`]: TerminalCapabilities::effective_graphics
/// [`kitty_graphics_supported`]: TerminalCapabilities::kitty_graphics_supported
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerminalCapabilities {
    /// Whether crossterm confirmed kitty keyboard protocol support.
    pub keyboard_enhancement: bool,
    /// The graphics protocol inferred from the environment (the env-var heuristic in
    /// [`karet_fileview::image::detect_protocol`]).
    pub graphics_env: GraphicsProtocol,
    /// The runtime kitty-graphics handshake result: `Some(true)` when the terminal
    /// answered the graphics query, `Some(false)` when it answered DA1 but not the
    /// query, and `None` on timeout or I/O error.
    pub kitty_graphics: Option<bool>,
    /// The runtime OSC 22 pointer-shape handshake result, with the same tri-state
    /// semantics as [`kitty_graphics`](Self::kitty_graphics).
    pub osc22_pointer_shape: Option<bool>,
    /// Whether the environment names a terminal that draws a curly underline
    /// (see [`probe_undercurl`]).
    pub undercurl: bool,
    /// How many colours the terminal shows (see [`probe_color_depth`]).
    pub color_depth: ColorDepth,
}

impl TerminalCapabilities {
    /// The effective graphics protocol: upgrade the env heuristic to Kitty when the
    /// runtime handshake positively confirmed it, and otherwise trust the heuristic.
    /// The handshake never downgrades a terminal the heuristic already trusts —
    /// matching the app's upgrade-only startup semantics.
    #[must_use]
    pub(crate) fn effective_graphics(&self) -> GraphicsProtocol {
        if self.kitty_graphics == Some(true) {
            GraphicsProtocol::Kitty
        } else {
            self.graphics_env
        }
    }

    /// Whether the kitty graphics protocol is available (env heuristic or confirmed
    /// handshake).
    #[must_use]
    pub(crate) fn kitty_graphics_supported(&self) -> bool {
        self.effective_graphics() == GraphicsProtocol::Kitty
    }

    /// Whether OSC 22 pointer-shape hints were confirmed supported.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn pointer_shapes_supported(&self) -> bool {
        self.osc22_pointer_shape == Some(true)
    }
}

/// Query crossterm for kitty keyboard enhancement support. Does not require raw mode.
#[must_use]
pub(crate) fn supports_kitty_keyboard() -> bool {
    matches!(
        crossterm::terminal::supports_keyboard_enhancement(),
        Ok(true)
    )
}

/// Probe every terminal capability in one pass.
///
/// The runtime handshakes ([`probe_kitty_graphics`], [`probe_osc22_pointer_shape`])
/// read replies straight from stdin, so this **must** run in raw mode and **before**
/// any input reader thread starts, or the replies leak into the UI as keystrokes.
#[must_use]
pub(crate) fn probe_all() -> TerminalCapabilities {
    TerminalCapabilities {
        keyboard_enhancement: supports_kitty_keyboard(),
        graphics_env: image::detect_protocol(),
        kitty_graphics: probe_kitty_graphics(PROBE_TIMEOUT),
        osc22_pointer_shape: probe_osc22_pointer_shape(PROBE_TIMEOUT),
        undercurl: probe_undercurl(),
        color_depth: probe_color_depth(),
    }
}

/// Whether the terminal draws a curly underline (SGR `4:3`), which diagnostics
/// are underlined with when it does.
///
/// Read from the environment, like the graphics heuristic: no query reports
/// underline styles, and a terminal that does not know `4:3` can read it as
/// underline-then-italic, so only a terminal known to draw it qualifies.
#[must_use]
pub(crate) fn probe_undercurl() -> bool {
    undercurl_in(|key| std::env::var(key).ok())
}

/// [`probe_undercurl`], reading the environment through `var`.
///
/// kitty, Ghostty, WezTerm, foot, Alacritty, iTerm2, VS Code's terminal, and
/// VTE 0.51.2 or later (GNOME Terminal, Tilix, …) draw it. tmux forwards
/// `4:3` only when the outer terminal is declared to take it, which nothing
/// here can see, so inside tmux or screen it is off.
pub(crate) fn undercurl_in(var: impl Fn(&str) -> Option<String>) -> bool {
    let term = var("TERM").unwrap_or_default();
    if var("TMUX").is_some() || term.starts_with("tmux") || term.starts_with("screen") {
        return false;
    }
    let program = var("TERM_PROGRAM").unwrap_or_default().to_ascii_lowercase();
    let named = ["kitty", "ghostty", "wezterm", "foot", "alacritty"]
        .iter()
        .any(|name| term.contains(name))
        || ["ghostty", "wezterm", "iterm.app", "vscode"].contains(&program.as_str());
    let vte = var("VTE_VERSION")
        .and_then(|version| version.trim().parse::<u32>().ok())
        .is_some_and(|version| version >= 5102);
    named
        || vte
        || var("KITTY_WINDOW_ID").is_some()
        || var("WEZTERM_EXECUTABLE").is_some()
        || var("ALACRITTY_WINDOW_ID").is_some()
}

/// How many colours the terminal shows: 24-bit when the environment says so,
/// otherwise the xterm 256-colour palette.
///
/// Read from the environment: the query that would report it (`DECRQSS` on an
/// RGB SGR) is answered inconsistently, and terminfo's `RGB`/`Tc` flags are
/// absent from most installed databases even where the terminal supports it.
#[must_use]
pub(crate) fn probe_color_depth() -> ColorDepth {
    color_depth_in(|key| std::env::var(key).ok())
}

/// [`probe_color_depth`], reading the environment through `var`.
///
/// `COLORTERM=truecolor` (or `24bit`) is the convention every truecolor
/// terminal follows, and the one way a user can assert it where nothing else
/// here can see it, so it wins outright. A `-direct` (or `truecolor`/`24bit`)
/// `TERM` says the same. Failing both, a terminal named in the environment that
/// is known to draw 24-bit colour qualifies — except inside tmux or screen,
/// which pass RGB through only when configured to, which nothing here can see.
pub(crate) fn color_depth_in(var: impl Fn(&str) -> Option<String>) -> ColorDepth {
    let colorterm = var("COLORTERM").unwrap_or_default().to_ascii_lowercase();
    if colorterm == "truecolor" || colorterm == "24bit" {
        return ColorDepth::TrueColor;
    }
    let term = var("TERM").unwrap_or_default().to_ascii_lowercase();
    if term.ends_with("-direct") || term.contains("truecolor") || term.contains("24bit") {
        return ColorDepth::TrueColor;
    }
    if var("TMUX").is_some() || term.starts_with("tmux") || term.starts_with("screen") {
        return ColorDepth::Indexed256;
    }
    let program = var("TERM_PROGRAM").unwrap_or_default().to_ascii_lowercase();
    let named = [
        "kitty",
        "ghostty",
        "wezterm",
        "foot",
        "alacritty",
        "contour",
    ]
    .iter()
    .any(|name| term.contains(name))
        || ["ghostty", "wezterm", "iterm.app", "vscode"].contains(&program.as_str());
    // VTE has drawn 24-bit colour since 0.36.
    let vte = var("VTE_VERSION")
        .and_then(|version| version.trim().parse::<u32>().ok())
        .is_some_and(|version| version >= 3600);
    let known = named
        || vte
        || var("KITTY_WINDOW_ID").is_some()
        || var("WEZTERM_EXECUTABLE").is_some()
        || var("ALACRITTY_WINDOW_ID").is_some()
        || var("KONSOLE_VERSION").is_some()
        || var("WT_SESSION").is_some();
    if known {
        ColorDepth::TrueColor
    } else {
        ColorDepth::Indexed256
    }
}

/// Probe whether the terminal speaks the Kitty graphics protocol via a real handshake.
///
/// Emits a graphics *query* (`a=q`, which does not display anything) followed by a
/// Primary Device Attributes request (`ESC [ c`) as a terminator, then reads the
/// reply straight from stdin. Returns `Some(true)` when the terminal answers the
/// graphics query, `Some(false)` when it answers DA1 but not the graphics query,
/// and `None` on timeout or I/O error.
///
/// Must run in raw mode and **before** the input reader thread starts, so the
/// query responses are consumed here rather than leaking into the UI as keystrokes.
/// Unlike the env-var [`detect_protocol`](image::detect_protocol) heuristic, this
/// recognizes any graphics-capable terminal, not just an allowlist.
pub(crate) fn probe_kitty_graphics(timeout: Duration) -> Option<bool> {
    use std::io::Read;

    // `i=31` is an arbitrary image id echoed back in the reply; `\x1b[c` (DA1) is
    // answered by every terminal and marks the end of the responses to read.
    let query = "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[c";
    let mut stdout = std::io::stdout();
    write!(stdout, "{query}").ok()?;
    stdout.flush().ok()?;

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        let mut saw_csi = false;
        loop {
            match stdin.read(&mut byte) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let b = byte[0];
                    buf.push(b);
                    // Stop once the DA1 reply (CSI … 'c') has been fully consumed.
                    saw_csi |= b == b'[';
                    if saw_csi && b == b'c' {
                        break;
                    }
                },
            }
        }
        let _ = tx.send(buf);
    });

    let buf = rx.recv_timeout(timeout).ok()?;
    Some(kitty_graphics_acknowledged(&buf))
}

/// Whether `reply`, the bytes read back after [`probe_kitty_graphics`]'s query,
/// holds a Kitty graphics acknowledgement: `ESC _ G i=31 ; OK ESC \`.
fn kitty_graphics_acknowledged(reply: &[u8]) -> bool {
    reply.windows(2).any(|w| w == b"_G") && reply.windows(2).any(|w| w == b"OK")
}

/// Probe whether the terminal supports OSC 22 (mouse pointer-shape hints, e.g.
/// hovering a resize divider showing the OS's resize cursor) by sending its
/// query form (`ESC ] 22 ; ? ESC \`) and checking for an OSC 22 reply before
/// the DA1 terminator. `Some(true)`/`Some(false)` when the terminal answered
/// before `timeout`, `None` on timeout or I/O error — in which case the
/// caller must not send pointer-shape hints (they'd be silently ignored at
/// best, or misinterpreted at worst). Same raw-mode/before-input-thread
/// constraint and terminating-DA1 trick as [`probe_kitty_graphics`].
pub(crate) fn probe_osc22_pointer_shape(timeout: Duration) -> Option<bool> {
    use std::io::Read;

    let query = "\x1b]22;?\x1b\\\x1b[c";
    let mut stdout = std::io::stdout();
    write!(stdout, "{query}").ok()?;
    stdout.flush().ok()?;

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        let mut saw_csi = false;
        loop {
            match stdin.read(&mut byte) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let b = byte[0];
                    buf.push(b);
                    saw_csi |= b == b'[';
                    if saw_csi && b == b'c' {
                        break;
                    }
                },
            }
        }
        let _ = tx.send(buf);
    });

    let buf = rx.recv_timeout(timeout).ok()?;
    Some(osc22_answered(&buf))
}

/// Whether `reply`, the bytes read back after [`probe_osc22_pointer_shape`]'s
/// query, holds an OSC 22 reply, which echoes its own introducer: `ESC ] 22 ; …`.
fn osc22_answered(reply: &[u8]) -> bool {
    reply.windows(3).any(|w| w == b"]22")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(
        keyboard_enhancement: bool,
        graphics_env: GraphicsProtocol,
        kitty_graphics: Option<bool>,
        osc22_pointer_shape: Option<bool>,
    ) -> TerminalCapabilities {
        TerminalCapabilities {
            keyboard_enhancement,
            graphics_env,
            kitty_graphics,
            osc22_pointer_shape,
            undercurl: false,
            color_depth: ColorDepth::TrueColor,
        }
    }

    /// [`color_depth_in`] over exactly the `vars` given.
    fn depth(vars: &[(&str, &str)]) -> ColorDepth {
        color_depth_in(|key| {
            vars.iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn truecolor_is_detected_from_colorterm_term_and_known_terminals() {
        for vars in [
            &[("COLORTERM", "truecolor")][..],
            &[("COLORTERM", "24bit"), ("TERM", "xterm-256color")],
            &[("COLORTERM", "TrueColor"), ("TERM", "linux")],
            // COLORTERM is the user's override, so it holds inside tmux too.
            &[("COLORTERM", "truecolor"), ("TERM", "tmux-256color")],
            &[("TERM", "xterm-direct")],
            &[
                ("TERM", "konsole-direct"),
                ("TMUX", "/tmp/tmux-1000/default"),
            ],
            &[("TERM", "xterm-kitty")],
            &[("TERM", "xterm-ghostty")],
            &[("TERM", "foot")],
            &[("TERM", "alacritty")],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "WezTerm")],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "iTerm.app")],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "vscode")],
            &[("TERM", "xterm-256color"), ("KITTY_WINDOW_ID", "1")],
            &[("TERM", "xterm-256color"), ("VTE_VERSION", "3600")],
            &[("TERM", "xterm-256color"), ("KONSOLE_VERSION", "230401")],
            &[("WT_SESSION", "0b5c1e3a")],
        ] {
            assert_eq!(depth(vars), ColorDepth::TrueColor, "{vars:?}");
        }
    }

    #[test]
    fn everything_else_falls_back_to_256_colours() {
        for vars in [
            &[][..],
            &[("TERM", "xterm-256color")],
            &[("TERM", "xterm-256color"), ("COLORTERM", "yes")],
            &[("TERM", "xterm-256color"), ("VTE_VERSION", "3599")],
            &[("TERM", "xterm-256color"), ("VTE_VERSION", "not a version")],
            &[
                ("TERM", "xterm-256color"),
                ("TERM_PROGRAM", "Apple_Terminal"),
            ],
            // A truecolor terminal behind tmux or screen without COLORTERM:
            // the multiplexer may not pass RGB through.
            &[("TERM", "tmux-256color"), ("TERM_PROGRAM", "tmux")],
            &[("TERM", "screen-256color"), ("KITTY_WINDOW_ID", "1")],
            &[
                ("TERM", "xterm-kitty"),
                ("TMUX", "/tmp/tmux-1000/default,1,0"),
            ],
        ] {
            assert_eq!(depth(vars), ColorDepth::Indexed256, "{vars:?}");
        }
    }

    /// [`undercurl_in`] over exactly the `vars` given.
    fn undercurl(vars: &[(&str, &str)]) -> bool {
        undercurl_in(|key| {
            vars.iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn undercurl_is_on_for_the_terminals_known_to_draw_it() {
        for vars in [
            &[("TERM", "xterm-kitty")][..],
            &[("TERM", "xterm-ghostty")],
            &[("TERM", "foot-extra")],
            &[("TERM", "alacritty")],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "WezTerm")],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "iTerm.app")],
            &[("TERM", "xterm-256color"), ("TERM_PROGRAM", "vscode")],
            &[("TERM", "xterm-256color"), ("KITTY_WINDOW_ID", "1")],
            &[("TERM", "xterm-256color"), ("VTE_VERSION", "7600")],
            &[("TERM", "xterm-256color"), ("VTE_VERSION", "5102")],
        ] {
            assert!(undercurl(vars), "{vars:?}");
        }
    }

    #[test]
    fn undercurl_is_off_for_an_unknown_terminal_an_old_vte_and_tmux() {
        for vars in [
            &[][..],
            &[("TERM", "xterm-256color")],
            &[("TERM", "linux")],
            &[("TERM", "xterm-256color"), ("VTE_VERSION", "5101")],
            &[("TERM", "xterm-256color"), ("VTE_VERSION", "not a version")],
            &[
                ("TERM", "xterm-256color"),
                ("TERM_PROGRAM", "Apple_Terminal"),
            ],
            &[("TERM", "tmux-256color"), ("TERM_PROGRAM", "tmux")],
            &[("TERM", "screen-256color")],
            &[
                ("TERM", "xterm-kitty"),
                ("TMUX", "/tmp/tmux-1000/default,1,0"),
            ],
        ] {
            assert!(!undercurl(vars), "{vars:?}");
        }
    }

    #[test]
    fn handshake_upgrades_halfblocks_to_kitty() {
        let c = caps(true, GraphicsProtocol::Halfblocks, Some(true), None);
        assert_eq!(c.effective_graphics(), GraphicsProtocol::Kitty);
        assert!(c.kitty_graphics_supported());
    }

    #[test]
    fn handshake_never_downgrades_the_env_heuristic() {
        // Heuristic already trusts Kitty; a `Some(false)`/`None` handshake keeps it.
        for handshake in [Some(false), None] {
            let c = caps(true, GraphicsProtocol::Kitty, handshake, None);
            assert_eq!(c.effective_graphics(), GraphicsProtocol::Kitty);
            assert!(c.kitty_graphics_supported());
        }
    }

    #[test]
    fn no_handshake_and_no_heuristic_stays_halfblocks() {
        for handshake in [Some(false), None] {
            let c = caps(true, GraphicsProtocol::Halfblocks, handshake, None);
            assert_eq!(c.effective_graphics(), GraphicsProtocol::Halfblocks);
            assert!(!c.kitty_graphics_supported());
        }
    }

    #[test]
    fn pointer_shapes_supported_only_on_positive_confirmation() {
        assert!(caps(true, GraphicsProtocol::Kitty, None, Some(true)).pointer_shapes_supported());
        assert!(!caps(true, GraphicsProtocol::Kitty, None, Some(false)).pointer_shapes_supported());
        assert!(!caps(true, GraphicsProtocol::Kitty, None, None).pointer_shapes_supported());
    }

    /// A graphics acknowledgement before the DA1 reply means kitty graphics; a
    /// DA1 reply alone, an error reply, or nothing does not.
    #[test]
    fn kitty_graphics_reply_is_classified() {
        let rows: [(&str, &[u8], bool); 6] = [
            ("ack then DA1", b"\x1b_Gi=31;OK\x1b\\\x1b[?62;22c", true),
            ("ack alone", b"\x1b_Gi=31;OK\x1b\\", true),
            ("DA1 only", b"\x1b[?62;22c", false),
            (
                "an error reply",
                b"\x1b_Gi=31;ENOTSUPPORTED\x1b\\\x1b[?62c",
                false,
            ),
            ("OK outside any graphics reply", b"\x1b[?62;OKc", false),
            ("nothing", b"", false),
        ];
        for (name, reply, expect) in rows {
            assert_eq!(kitty_graphics_acknowledged(reply), expect, "{name}");
        }
    }

    /// An echoed `ESC ] 22` before the DA1 reply means OSC 22 pointer shapes; a
    /// DA1 reply alone, another OSC, or nothing does not.
    #[test]
    fn osc22_reply_is_classified() {
        let rows: [(&str, &[u8], bool); 5] = [
            ("reply then DA1", b"\x1b]22;default\x1b\\\x1b[?62c", true),
            ("BEL-terminated reply", b"\x1b]22;text\x07", true),
            ("DA1 only", b"\x1b[?62;22c", false),
            ("another OSC", b"\x1b]2;title\x1b\\\x1b[?62c", false),
            ("nothing", b"", false),
        ];
        for (name, reply, expect) in rows {
            assert_eq!(osc22_answered(reply), expect, "{name}");
        }
    }
}
