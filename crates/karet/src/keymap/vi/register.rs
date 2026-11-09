//! Registers: where yanks and deletes go and pastes come from.
//!
//! The vim register set: the unnamed register `"`, the named `a`–`z` (an
//! uppercase name appends), `0` for the last yank, `1`–`9` for the recent
//! multi-line deletes, `-` for the last small delete, the black hole `_`, and
//! `+`/`*` for the system clipboard. Helix uses the same names, so one store
//! serves both grammars.

use std::collections::VecDeque;

/// Text held in a register, and whether it is whole lines (each ending in a
/// line break), which decides how it pastes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Register {
    /// The text.
    pub(crate) text: String,
    /// Whole lines rather than a run of characters.
    pub(crate) linewise: bool,
}

/// The system clipboard behind the `+` and `*` registers.
pub(crate) trait Clipboard {
    /// The clipboard's text, if it can be read.
    fn get(&mut self) -> Option<String>;
    /// Replace the clipboard's text.
    fn set(&mut self, text: &str);
}

/// What put text into a register, which decides the numbered registers it
/// also reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Write {
    /// A yank: also register `0`.
    Yank,
    /// A delete or change: register `1` (shifting the others) when it spans
    /// lines, else `-`.
    Delete,
}

/// Every register's contents.
#[derive(Debug, Default)]
pub(crate) struct Registers {
    unnamed: Option<Register>,
    named: [Option<Register>; 26],
    yank: Option<Register>,
    numbered: VecDeque<Register>,
    small_delete: Option<Register>,
}

/// Whether `name` is a register a key may name after `"`.
pub(super) fn is_valid(name: char) -> bool {
    name.is_ascii_alphanumeric() || matches!(name, '"' | '-' | '_' | '+' | '*')
}

impl Registers {
    /// Store `reg` as `kind` into `name` (the unnamed register when `None`).
    pub(super) fn write(
        &mut self,
        name: Option<char>,
        reg: Register,
        kind: Write,
        clipboard: &mut dyn Clipboard,
    ) {
        match name {
            Some('_') => return,
            Some('+' | '*') => clipboard.set(&reg.text),
            Some(c) if c.is_ascii_uppercase() => {
                let slot = self.named_slot(c.to_ascii_lowercase());
                let joined = match slot.take() {
                    Some(old) if reg.linewise && !old.linewise => Register {
                        text: format!("{}\n{}", old.text, reg.text),
                        linewise: true,
                    },
                    Some(old) => Register {
                        text: old.text + &reg.text,
                        linewise: old.linewise || reg.linewise,
                    },
                    None => reg.clone(),
                };
                *slot = Some(joined.clone());
                self.unnamed = Some(joined);
                return;
            },
            Some(c) if c.is_ascii_lowercase() => {
                *self.named_slot(c) = Some(reg.clone());
            },
            _ => match kind {
                Write::Yank => self.yank = Some(reg.clone()),
                Write::Delete if reg.linewise || reg.text.contains('\n') => {
                    self.numbered.push_front(reg.clone());
                    self.numbered.truncate(9);
                },
                Write::Delete => self.small_delete = Some(reg.clone()),
            },
        }
        self.unnamed = Some(reg);
    }

    /// The contents of `name` (the unnamed register when `None`). Clipboard
    /// text ending in a line break pastes as whole lines.
    pub(super) fn read(
        &self,
        name: Option<char>,
        clipboard: &mut dyn Clipboard,
    ) -> Option<Register> {
        match name {
            None | Some('"') => self.unnamed.clone(),
            Some('+' | '*') => clipboard.get().map(|text| {
                let text = text.replace("\r\n", "\n");
                Register {
                    linewise: text.ends_with('\n'),
                    text,
                }
            }),
            Some('0') => self.yank.clone(),
            Some('-') => self.small_delete.clone(),
            Some(c @ '1'..='9') => self.numbered.get(c as usize - '1' as usize).cloned(),
            Some(c) if c.is_ascii_alphabetic() => self
                .named
                .get((c.to_ascii_lowercase() as u8 - b'a') as usize)
                .cloned()
                .flatten(),
            Some(_) => None,
        }
    }

    fn named_slot(&mut self, c: char) -> &mut Option<Register> {
        let index = (c as u8).saturating_sub(b'a') as usize % 26;
        &mut self.named[index]
    }
}
