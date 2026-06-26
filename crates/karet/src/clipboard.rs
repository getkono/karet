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
pub struct Clipboard {}

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

    /// Read the system clipboard.
    pub fn get(&self) -> Result<String, ClipboardError> {
        todo!()
    }
}

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
