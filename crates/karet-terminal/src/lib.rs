//! `karet-terminal` — an embeddable VT/ANSI terminal emulator for karet.
//!
//! Parses terminal output into a screen + scrollback model, spawns a PTY and
//! pumps its IO. The engine is renderer-agnostic and dependency-light (it reports
//! a plain cell grid); enable the `view` feature for a ratatui widget. Positions
//! are plain `u16` rows/cols so the headless engine pulls in no coordinate crate.
//!
//! This is the implementation *skeleton*: the public joints are defined; the
//! vt100/PTY logic is filled in separately.

/// Errors produced by the terminal engine.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TermError {
    /// A PTY operation failed.
    #[error("pty error: {0}")]
    Pty(String),
    /// A terminal I/O error.
    #[error("terminal i/o error")]
    Io,
}

/// A single cell of the terminal grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cell {
    /// The character displayed.
    pub ch: char,
    /// Whether the cell is bold.
    pub bold: bool,
    /// Whether foreground/background are swapped.
    pub inverse: bool,
}

/// An embeddable terminal: a parsed screen grid plus scrollback.
pub struct Terminal {}

impl Terminal {
    /// Create a terminal sized `rows` × `cols`.
    #[must_use]
    pub fn new(rows: u16, cols: u16) -> Self {
        let _ = (rows, cols);
        todo!()
    }

    /// Feed output bytes from the PTY into the parser.
    pub fn feed(&mut self, bytes: &[u8]) {
        let _ = bytes;
        todo!()
    }

    /// Resize the grid to `rows` × `cols`.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let _ = (rows, cols);
        todo!()
    }

    /// The cell at `(row, col)`, if on screen.
    #[must_use]
    pub fn cell(&self, row: u16, col: u16) -> Option<Cell> {
        let _ = (row, col);
        todo!()
    }
}

/// A command to run inside a PTY.
#[derive(Clone, Debug)]
pub struct CommandSpec {
    /// The program to execute.
    pub program: String,
    /// Command-line arguments.
    pub args: Vec<String>,
}

/// A pseudo-terminal hosting a child process.
pub struct Pty {}

impl Pty {
    /// Spawn `cmd` in a PTY sized `rows` × `cols`.
    ///
    /// # Errors
    /// Returns [`TermError::Pty`] if the child cannot be spawned.
    pub fn spawn(cmd: &CommandSpec, rows: u16, cols: u16) -> Result<Self, TermError> {
        let _ = (cmd, rows, cols);
        todo!()
    }

    /// Read available child output into `buf`, returning the byte count.
    ///
    /// # Errors
    /// Returns [`TermError::Io`] on read failure.
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, TermError> {
        let _ = buf;
        todo!()
    }

    /// Write `bytes` to the child's input.
    ///
    /// # Errors
    /// Returns [`TermError::Io`] on write failure.
    pub async fn write(&mut self, bytes: &[u8]) -> Result<(), TermError> {
        let _ = bytes;
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_defaults_blank() {
        let c = Cell::default();
        assert!(!c.bold);
        assert!(!c.inverse);
    }

    #[test]
    fn error_displays() {
        assert_eq!(TermError::Io.to_string(), "terminal i/o error");
    }
}
