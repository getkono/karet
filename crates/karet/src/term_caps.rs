//! Centralized terminal capability detection.
//!
//! karet targets modern terminals and probes for a handful of features at startup:
//! the kitty keyboard protocol (a hard requirement), the kitty graphics protocol
//! (drives image rendering and the optional graphical caret), and OSC 22 mouse
//! pointer-shape hints. Historically these probes were scattered inline through
//! `app::run`; this module owns the single implementation so both the app and the
//! `--doctor` diagnostics consume the same primitive.
//!
//! The env-var graphics heuristic itself lives in [`karet_fileview::image`] (a
//! published library API); this module reuses it and layers the runtime handshakes
//! on top.

use std::io::Write;
use std::time::Duration;

use karet_fileview::image::GraphicsProtocol;
use karet_fileview::image::{self};

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
    // A Kitty graphics acknowledgement looks like: ESC _ G i=31 ; OK ESC \
    let ok = buf.windows(2).any(|w| w == b"_G") && buf.windows(2).any(|w| w == b"OK");
    Some(ok)
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
    // An OSC 22 reply contains its own introducer echoed back: ESC ] 22 ; ...
    let ok = buf.windows(3).any(|w| w == b"]22");
    Some(ok)
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
}
