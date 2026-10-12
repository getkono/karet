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

/// What karet itself last put on the clipboard, beyond the plain text: the
/// piece each caret copied, and whether the copy was of whole lines (a copy with
/// no selection).
///
/// Plain text carries neither, so a paste recovers them only while the
/// clipboard still holds exactly the text this entry wrote (see
/// [`Clipboard::copied`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copied {
    /// One piece per copied selection (or per copied line), in document order.
    /// A whole-line piece ends with its newline.
    pub pieces: Vec<String>,
    /// Whether the pieces are whole lines, copied with no selection, so a paste
    /// inserts them above the caret's line rather than at the caret.
    pub whole_lines: bool,
}

impl Copied {
    /// The plain text this entry puts on the clipboard: whole lines back to back
    /// (each already ends with its newline), selections joined by newlines.
    #[must_use]
    pub fn text(&self) -> String {
        if self.whole_lines {
            self.pieces.concat()
        } else {
            self.pieces.join("\n")
        }
    }
}

/// Where clipboard text goes and comes from.
enum Store {
    /// The system clipboard: OSC 52 to write, an external tool to read.
    System,
    /// An in-process string, so tests never touch the terminal or the desktop.
    #[cfg(test)]
    Memory(Option<String>),
}

/// A clipboard that writes via OSC 52 (works over SSH/tmux) and falls back to
/// `wl-copy`/`xclip`/`xsel`/`pbcopy`.
pub struct Clipboard {
    store: Store,
    /// The text karet last wrote and what it was copied from, while that write
    /// is the newest one karet made.
    last: Option<(String, Copied)>,
    /// What the primary selection holds in a test: the real one is another
    /// process's, which a test can neither set nor rely on.
    #[cfg(test)]
    pub(crate) primary: Option<String>,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipboard {
    /// Create a clipboard.
    pub fn new() -> Self {
        Self {
            store: Store::System,
            last: None,
            #[cfg(test)]
            primary: None,
        }
    }

    /// A clipboard held in memory, for tests.
    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self {
            store: Store::Memory(None),
            last: None,
            primary: None,
        }
    }

    /// Set the system clipboard to `text` by emitting an OSC 52 escape sequence to
    /// stdout, which most modern terminals honour (and which works over SSH/tmux).
    ///
    /// Forgets the [`Copied`] entry an earlier [`set_copied`](Self::set_copied)
    /// recorded.
    pub fn set(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.last = None;
        self.write(text)
    }

    /// Set the clipboard to `copied`'s text, remembering its pieces and its
    /// line-copy marker for a later paste of that same text.
    pub fn set_copied(&mut self, copied: Copied) -> Result<(), ClipboardError> {
        self.last = None;
        let text = copied.text();
        self.write(&text)?;
        self.last = Some((text, copied));
        Ok(())
    }

    /// The entry karet copied, when `text` (newlines normalized to `\n`) is
    /// exactly what that copy wrote. Text copied anywhere else since reads back
    /// different and gets `None`.
    #[must_use]
    pub fn copied(&self, text: &str) -> Option<&Copied> {
        self.last
            .as_ref()
            .filter(|(written, _)| written == text)
            .map(|(_, copied)| copied)
    }

    fn write(&mut self, text: &str) -> Result<(), ClipboardError> {
        match &mut self.store {
            Store::System => {
                use std::io::Write as _;
                let mut out = std::io::stdout();
                out.write_all(osc52_set_sequence(text).as_bytes())?;
                out.flush()?;
            },
            #[cfg(test)]
            Store::Memory(held) => *held = Some(text.to_owned()),
        }
        Ok(())
    }

    /// Read the system clipboard by shelling out to the first available tool.
    ///
    /// OSC 52 *reads* are widely unsupported (and often disabled for security), so
    /// the read path uses external tools rather than the terminal.
    pub fn get(&self) -> Result<String, ClipboardError> {
        #[cfg(test)]
        if let Store::Memory(held) = &self.store {
            return held.clone().ok_or(ClipboardError::Unavailable);
        }
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

    #[test]
    fn copied_text_joins_selections_and_concatenates_lines() {
        let selections = Copied {
            pieces: vec!["a".into(), "b".into()],
            whole_lines: false,
        };
        assert_eq!(selections.text(), "a\nb");
        let lines = Copied {
            pieces: vec!["a\n".into(), "b\n".into()],
            whole_lines: true,
        };
        assert_eq!(lines.text(), "a\nb\n");
    }

    #[test]
    fn the_marker_holds_only_while_the_clipboard_has_its_text() -> Result<(), ClipboardError> {
        let mut clipboard = Clipboard::in_memory();
        let copied = Copied {
            pieces: vec!["line\n".into()],
            whole_lines: true,
        };
        clipboard.set_copied(copied.clone())?;
        assert_eq!(clipboard.get()?, "line\n");
        assert_eq!(clipboard.copied("line\n"), Some(&copied));
        assert_eq!(clipboard.copied("other"), None);
        clipboard.set("line\n")?;
        assert_eq!(clipboard.copied("line\n"), None);
        Ok(())
    }
}
