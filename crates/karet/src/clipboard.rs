//! OSC 52 clipboard with external-tool fallbacks (merged from `karet-clipboard`).

use base64::Engine as _;

/// Errors from clipboard operations.
#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    /// No clipboard mechanism (OSC 52 or external tool) was available.
    #[error("no clipboard available")]
    Unavailable,
    /// Writing the OSC 52 escape sequence to the terminal failed.
    #[error("clipboard write failed: {0}")]
    Io(#[from] std::io::Error),
}

/// A clipboard that writes via OSC 52 (works over SSH/tmux) and falls back to
/// `wl-copy`/`xclip`/`xsel`/`pbcopy`.
#[derive(Default)]
pub struct Clipboard {
    /// What the primary selection holds in a test: the real one is another
    /// process's, which a test can neither set nor rely on.
    #[cfg(test)]
    pub(crate) primary: Option<String>,
}

impl Clipboard {
    /// Create a clipboard.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the system clipboard to `text` by emitting an OSC 52 escape sequence to
    /// stdout, which most modern terminals honour (and which works over SSH/tmux).
    pub fn set(&self, text: &str) -> Result<(), ClipboardError> {
        use std::io::Write as _;
        let mut out = std::io::stdout();
        out.write_all(osc52_set_sequence(text).as_bytes())?;
        out.flush()?;
        Ok(())
    }

    /// Read the system clipboard by shelling out to the first available tool.
    ///
    /// OSC 52 *reads* are widely unsupported (and often disabled for security), so
    /// the read path uses external tools rather than the terminal.
    pub fn get(&self) -> Result<String, ClipboardError> {
        read_first(CLIPBOARD_PASTE_COMMANDS)
    }

    /// Read the primary selection — the X11 and Wayland "last selected text",
    /// which middle click pastes — by shelling out to the first available
    /// tool. Platforms without one (macOS, Windows) report it unavailable.
    #[cfg(not(test))]
    pub fn get_primary(&self) -> Result<String, ClipboardError> {
        read_first(PRIMARY_PASTE_COMMANDS)
    }

    /// The test stand-in for [`get_primary`](Self::get_primary).
    #[cfg(test)]
    pub fn get_primary(&self) -> Result<String, ClipboardError> {
        self.primary.clone().ok_or(ClipboardError::Unavailable)
    }
}

/// The output of the first of `commands` that runs and succeeds.
fn read_first(commands: &[(&str, &[&str])]) -> Result<String, ClipboardError> {
    for (cmd, args) in commands {
        if let Ok(output) = std::process::Command::new(cmd).args(*args).output()
            && output.status.success()
        {
            return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
        }
    }
    Err(ClipboardError::Unavailable)
}

/// Primary-selection read commands, tried in order (Wayland, then X11).
#[cfg_attr(test, allow(dead_code))] // tests read a stand-in, never another process
const PRIMARY_PASTE_COMMANDS: &[(&str, &[&str])] = &[
    ("wl-paste", &["--primary", "--no-newline"]),
    ("xclip", &["-selection", "primary", "-o"]),
    ("xsel", &["-p", "-o"]),
];

/// Clipboard read commands, tried in order (Wayland, X11, macOS, Windows/WSL).
const CLIPBOARD_PASTE_COMMANDS: &[(&str, &[&str])] = &[
    ("wl-paste", &["--no-newline"]),
    ("xclip", &["-selection", "clipboard", "-o"]),
    ("xsel", &["-b", "-o"]),
    ("pbpaste", &[]),
    (
        "powershell.exe",
        &["-NoProfile", "-Command", "Get-Clipboard"],
    ),
];

/// Build an OSC 52 escape sequence that sets the clipboard to `text`.
pub fn osc52_set_sequence(text: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    format!("\x1b]52;c;{encoded}\x07")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_wraps_base64() {
        let seq = osc52_set_sequence("hi");
        assert!(seq.starts_with("\x1b]52;c;"));
        assert!(seq.ends_with('\x07'));
    }
}
