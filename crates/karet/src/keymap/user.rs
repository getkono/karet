//! User and project keymap files: `keymap.jsonc`, parsed into the same
//! [`Binding`] shape as the compiled-in table and layered over it.
//!
//! Two files are read, lowest precedence first, each beside its `setting.jsonc`:
//!
//! 1. `$XDG_CONFIG_HOME/karet/keymap.jsonc` — user
//! 2. `$GIT_ROOT/.karet/keymap.jsonc` — project
//!
//! A file is an object with a `bindings` array (plus an optional `"$schema"`, see
//! `docs/keymap.schema.json`). Each entry is applied in order:
//!
//! ```jsonc
//! {
//!   "bindings": [
//!     // Bind: `key` is one chord or a space-separated sequence; `when` names
//!     // the layer it lives in (default `global`).
//!     { "key": "ctrl+k ctrl+c", "command": "toggleLineComment", "when": "editor" },
//!     // Unbind: a `-` before the id removes that command's bindings, narrowed
//!     // to `key` and/or `when` when given.
//!     { "key": "ctrl+q", "command": "-quit" }
//!   ]
//! }
//! ```
//!
//! Loading never fails. A missing file is skipped; a malformed file is ignored
//! whole; a bad entry is skipped alone. Every problem — and every binding that a
//! keymap file makes untypeable — becomes an [`Issue`], shown as a notification and
//! by `karet --doctor`.

use std::path::Path;
use std::path::PathBuf;

use karet_core::Severity;
use karet_session::config::ConfigDiagnostic;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedReceiver;

use super::layer::all_stacks;
use super::layer::takes_sequences;
use super::*;

/// The keymap file's name in both the user and the project config directory.
pub const FILE_NAME: &str = "keymap.jsonc";

/// The most issues [`KeymapReport::diagnostics`] turns into notifications; the
/// rest are summarized in one more, pointing at `--doctor`.
const MAX_NOTIFIED: usize = 3;

/// Which keymap file a binding or a problem came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// `$XDG_CONFIG_HOME/karet/keymap.jsonc`.
    User,
    /// `$GIT_ROOT/.karet/keymap.jsonc`.
    Project,
}

impl Source {
    /// The display label for this layer.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

/// What became of one keymap file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileStatus {
    /// The file was not present.
    Missing,
    /// The file parsed; `entries` is the length of its `bindings` array (some
    /// entries may still have been skipped, each with an [`Issue`]).
    Loaded {
        /// How many entries the file holds.
        entries: usize,
    },
    /// The file could not be read or parsed, and was ignored whole.
    Invalid,
}

/// One keymap file considered while loading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileReport {
    /// Which layer the file is.
    pub source: Source,
    /// The path that was checked.
    pub path: PathBuf,
    /// The outcome for that path.
    pub status: FileStatus,
}

/// One problem with the keymap files: a file or entry that was ignored, or a
/// binding a file made unreachable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    /// The keymap file the problem is in.
    pub path: PathBuf,
    /// A human-readable, located description.
    pub message: String,
}

/// The outcome of loading the keymap files.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeymapReport {
    /// Every file considered, lowest precedence first.
    pub files: Vec<FileReport>,
    /// Every problem found, in file and entry order.
    pub issues: Vec<Issue>,
}

impl KeymapReport {
    /// The issues as configuration diagnostics, for the notification center —
    /// at most [`MAX_NOTIFIED`] of them, plus one summary of the rest.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<ConfigDiagnostic> {
        let mut diags: Vec<ConfigDiagnostic> = self
            .issues
            .iter()
            .take(MAX_NOTIFIED)
            .map(|issue| ConfigDiagnostic {
                path: issue.path.clone(),
                message: format!("{}: {}", issue.path.display(), issue.message),
                severity: Severity::Warning,
            })
            .collect();
        if let Some(last) = self.issues.get(MAX_NOTIFIED) {
            diags.push(ConfigDiagnostic {
                path: last.path.clone(),
                message: format!(
                    "{} more keymap problems; run `karet --doctor` to list them",
                    self.issues.len() - MAX_NOTIFIED
                ),
                severity: Severity::Warning,
            });
        }
        diags
    }
}

/// The keymap files for a workspace rooted at `roots`, lowest precedence first.
#[must_use]
pub fn paths(roots: &[PathBuf]) -> Vec<(Source, PathBuf)> {
    use karet_session::config::load::project_config_dir;
    use karet_session::config::load::user_config_dir;
    [
        user_config_dir().map(|dir| (Source::User, dir.join(FILE_NAME))),
        project_config_dir(roots).map(|dir| (Source::Project, dir.join(FILE_NAME))),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Load and merge the keymap files for a workspace rooted at `roots`.
#[must_use]
pub fn load(roots: &[PathBuf]) -> (Keymap, KeymapReport) {
    load_files(&paths(roots))
}

/// Load and merge the keymap files at `files` (lowest precedence first) over the
/// compiled-in defaults.
#[must_use]
pub fn load_files(files: &[(Source, PathBuf)]) -> (Keymap, KeymapReport) {
    let inputs: Vec<Input<'_>> = files
        .iter()
        .map(|(source, path)| Input {
            source: *source,
            path,
            text: match std::fs::read_to_string(path) {
                Ok(text) => Ok(Some(text)),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(err) => Err(format!("could not read keymap file: {err}")),
            },
        })
        .collect();
    merge(&inputs)
}

/// Watch the keymap files at `paths` for changes, so a save reloads them. `None`
/// when the platform watch backend cannot start: the keymap then stays as loaded.
#[must_use]
pub fn watch(
    paths: &[PathBuf],
) -> Option<(
    karet_watch::Watcher,
    UnboundedReceiver<karet_watch::FsEvent>,
)> {
    karet_watch::Watcher::spawn_with_paths(&[], &[], paths).ok()
}

/// One keymap file's contents, as read: `Ok(None)` for a missing file.
struct Input<'a> {
    source: Source,
    path: &'a Path,
    text: Result<Option<String>, String>,
}

/// A keymap file's top level.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    /// The editor-autocomplete schema reference; accepted and ignored.
    #[serde(rename = "$schema", default)]
    _schema: Option<serde::de::IgnoredAny>,
    /// The entries, kept raw so one bad entry is reported alone.
    #[serde(default)]
    bindings: Vec<Value>,
}

/// One entry of a keymap file's `bindings` array.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    key: Option<String>,
    command: String,
    when: Option<String>,
}

/// One validated entry.
enum Op {
    /// Bind `seq` to `command` in `layer`.
    Bind {
        layer: Layer,
        seq: Vec<KeyChord>,
        command: Command,
    },
    /// Remove `command`'s bindings, narrowed to `layer` and `seq` when given.
    Unbind {
        layer: Option<Layer>,
        seq: Option<Vec<KeyChord>>,
        command: Command,
    },
}

/// Apply `inputs` (lowest precedence first) over the defaults.
fn merge(inputs: &[Input<'_>]) -> (Keymap, KeymapReport) {
    let mut merger = Merger {
        defaults: BINDINGS.to_vec(),
        added: Vec::new(),
        report: KeymapReport::default(),
    };
    for input in inputs {
        let status = match &input.text {
            Ok(None) => FileStatus::Missing,
            Ok(Some(text)) => match parse_file(text) {
                Ok(entries) => {
                    let count = entries.len();
                    for (index, entry) in entries.into_iter().enumerate() {
                        merger.apply(input, index, entry);
                    }
                    FileStatus::Loaded { entries: count }
                },
                Err(message) => {
                    merger.issue(input.path, message);
                    FileStatus::Invalid
                },
            },
            Err(message) => {
                merger.issue(input.path, message.clone());
                FileStatus::Invalid
            },
        };
        merger.report.files.push(FileReport {
            source: input.source,
            path: input.path.to_path_buf(),
            status,
        });
    }
    merger.finish()
}

/// The merge in progress: file bindings apply over what is left of the defaults.
struct Merger {
    defaults: Vec<Binding>,
    /// File bindings in application order (the latest is most preferred).
    added: Vec<Binding>,
    report: KeymapReport,
}

impl Merger {
    fn issue(&mut self, path: &Path, message: String) {
        self.report.issues.push(Issue {
            path: path.to_path_buf(),
            message,
        });
    }

    /// Apply entry `index` of `input`, reporting it when it is unusable.
    fn apply(&mut self, input: &Input<'_>, index: usize, entry: Value) {
        let at = format!("binding {}", index + 1);
        let op = if entry.is_object() {
            serde_json::from_value::<RawEntry>(entry)
                .map_err(|err| err.to_string())
                .and_then(parse_entry)
        } else {
            // Checked up front: serde would also accept the struct as an array.
            Err("expected an object with `key` and `command`".to_string())
        };
        match op {
            Err(message) => self.issue(input.path, format!("{at}: {message}; skipped")),
            Ok(Op::Bind {
                layer,
                seq,
                command,
            }) => {
                let binding = Binding {
                    layer,
                    chord: seq[0],
                    rest: Cow::Owned(seq[1..].to_vec()),
                    command,
                    origin: Origin::File {
                        source: input.source,
                        entry: index,
                    },
                };
                // An earlier binding of the same keys in the same layer is replaced.
                // Replacing a default, or a lower file's binding, is the point; the
                // same file binding a key twice is almost certainly a mistake.
                for old in drain(&mut self.added, |old| same_trigger(old, &binding)) {
                    if let Origin::File { source, entry } = old.origin
                        && source == input.source
                    {
                        self.issue(
                            input.path,
                            format!(
                                "{at} rebinds {} in `{}`, overriding binding {}",
                                old.hint(ChordStyle::Verbose),
                                layer.id(),
                                entry + 1
                            ),
                        );
                    }
                }
                drain(&mut self.defaults, |old| same_trigger(old, &binding));
                self.added.push(binding);
            },
            Ok(Op::Unbind {
                layer,
                seq,
                command,
            }) => {
                let hit = |bind: &Binding| {
                    bind.command == command
                        && layer.is_none_or(|layer| bind.layer == layer)
                        && seq.as_deref().is_none_or(|seq| bind.trigger_is(seq))
                };
                let removed =
                    drain(&mut self.added, hit).len() + drain(&mut self.defaults, hit).len();
                if removed == 0 {
                    self.issue(input.path, format!("{at}: unbinds nothing"));
                }
            },
        }
    }

    /// The merged keymap, after reporting every binding a file made untypeable.
    fn finish(mut self) -> (Keymap, KeymapReport) {
        let bindings: Vec<Binding> = self.added.into_iter().rev().chain(self.defaults).collect();
        let paths: Vec<(Source, PathBuf)> = self
            .report
            .files
            .iter()
            .map(|file| (file.source, file.path.clone()))
            .collect();
        for (short, long) in shadowed(&bindings) {
            let (short, long) = (&bindings[short], &bindings[long]);
            // Blame the file binding; when both are, the shorter one did it.
            let culprit = if matches!(short.origin, Origin::File { .. }) {
                short
            } else {
                long
            };
            let Origin::File { source, entry } = culprit.origin else {
                continue;
            };
            let Some((_, path)) = paths.iter().find(|(s, _)| *s == source) else {
                continue;
            };
            self.report.issues.push(Issue {
                path: path.clone(),
                message: format!(
                    "binding {}: {} ({} in `{}`) is a complete binding, so {} ({} in `{}`) \
                     can never be typed",
                    entry + 1,
                    short.hint(ChordStyle::Verbose),
                    describe(short.command),
                    short.layer.id(),
                    long.hint(ChordStyle::Verbose),
                    describe(long.command),
                    long.layer.id(),
                ),
            });
        }
        (Keymap { bindings }, self.report)
    }
}

/// Remove and return every element of `items` matching `pred`, keeping the order
/// of the rest.
fn drain(items: &mut Vec<Binding>, pred: impl Fn(&Binding) -> bool) -> Vec<Binding> {
    let (hit, kept): (Vec<Binding>, Vec<Binding>) =
        std::mem::take(items).into_iter().partition(pred);
    *items = kept;
    hit
}

/// Whether two bindings are triggered by the same keys in the same layer.
fn same_trigger(a: &Binding, b: &Binding) -> bool {
    a.layer == b.layer
        && a.seq_len() == b.seq_len()
        && (0..a.seq_len()).all(|i| a.chord_at(i) == b.chord_at(i))
}

/// A command for a message: its id, or its label when it has none.
fn describe(command: Command) -> &'static str {
    command.id().unwrap_or_else(|| command.label())
}

/// Every `(shorter, longer)` pair of binding indices where both can be live at
/// once, `shorter`'s keys are a strict prefix of `longer`'s, and at least one of
/// them came from a keymap file. The resolver commits on the first complete
/// match, so `longer` can never fire in that context.
fn shadowed(bindings: &[Binding]) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let from_file = |b: &Binding| matches!(b.origin, Origin::File { .. });
    for stack in all_stacks() {
        let live: Vec<usize> = (0..bindings.len())
            .filter(|&i| stack.contains(&bindings[i].layer))
            .collect();
        for &s in &live {
            for &l in &live {
                let (short, long) = (&bindings[s], &bindings[l]);
                if (from_file(short) || from_file(long))
                    && short.seq_len() < long.seq_len()
                    && long.trigger_starts_with(short)
                    && !pairs.contains(&(s, l))
                {
                    pairs.push((s, l));
                }
            }
        }
    }
    pairs
}

impl Binding {
    /// Whether this binding's whole trigger is exactly `seq`.
    fn trigger_is(&self, seq: &[KeyChord]) -> bool {
        self.seq_len() == seq.len() && seq.iter().enumerate().all(|(i, &c)| self.chord_at(i) == c)
    }

    /// Whether `prefix`'s whole trigger begins this binding's trigger.
    fn trigger_starts_with(&self, prefix: &Binding) -> bool {
        prefix.seq_len() <= self.seq_len()
            && (0..prefix.seq_len()).all(|i| self.chord_at(i) == prefix.chord_at(i))
    }
}

/// Parse a keymap file's text into its raw entries.
fn parse_file(text: &str) -> Result<Vec<Value>, String> {
    let value: Option<Value> = jsonc_parser::parse_to_serde_value(text, &Default::default())
        .map_err(|err| format!("invalid JSONC: {err}"))?;
    match value {
        // An empty (or comment-only) file binds nothing.
        None => Ok(Vec::new()),
        // Checked up front: serde would also accept a struct written as an array.
        Some(value) if !value.is_object() => {
            Err("invalid keymap file: expected an object with a `bindings` array".to_string())
        },
        Some(value) => serde_json::from_value::<RawFile>(value)
            .map(|file| file.bindings)
            .map_err(|err| format!("invalid keymap file: {err}")),
    }
}

/// Validate one raw entry.
fn parse_entry(raw: RawEntry) -> Result<Op, String> {
    let layer = raw
        .when
        .as_deref()
        .map(|when| {
            Layer::from_id(when).ok_or_else(|| {
                let known: Vec<&str> = Layer::ALL.iter().map(|layer| layer.id()).collect();
                format!(
                    "unknown `when` layer `{when}` (known: {})",
                    known.join(", ")
                )
            })
        })
        .transpose()?;
    let seq = raw.key.as_deref().map(parse_sequence).transpose()?;
    let (unbind, id) = match raw.command.strip_prefix('-') {
        Some(id) => (true, id),
        None => (false, raw.command.as_str()),
    };
    let command = Command::from_id(id).ok_or_else(|| format!("unknown command `{id}`"))?;
    if unbind {
        return Ok(Op::Unbind {
            layer,
            seq,
            command,
        });
    }
    let seq = seq.ok_or_else(|| "a binding needs a `key`".to_string())?;
    let layer = layer.unwrap_or(Layer::Global);
    // Modal input resolves one chord at a time, so a sequence there loads but
    // never fires; refuse it rather than keep a binding nobody can type.
    if seq.len() > 1 && !takes_sequences(layer) {
        return Err(format!(
            "`{}` is a modal layer, which takes single chords only; a key sequence \
             there can never be typed",
            layer.id()
        ));
    }
    Ok(Op::Bind {
        layer,
        seq,
        command,
    })
}

/// Parse a key sequence: one or more chords separated by spaces
/// (`"ctrl+k ctrl+w"`).
fn parse_sequence(text: &str) -> Result<Vec<KeyChord>, String> {
    let seq: Vec<KeyChord> = text
        .split_whitespace()
        .map(parse_chord)
        .collect::<Result<_, _>>()?;
    if seq.is_empty() {
        return Err("empty `key`".to_string());
    }
    Ok(seq)
}

/// Parse one chord (`"ctrl+shift+p"`, `"alt+1"`, `"G"`, `"ctrl++"`) into its
/// canonical form, so it compares equal to the live key press it names.
fn parse_chord(text: &str) -> Result<KeyChord, String> {
    // A trailing `+` after a `+` separator is the plus key itself.
    let (mods, key) = if text == "+" {
        ("", "+")
    } else if let Some(mods) = text.strip_suffix("++") {
        (mods, "+")
    } else {
        text.rsplit_once('+').unwrap_or(("", text))
    };
    let (mut ctrl, mut shift, mut alt) = (false, false, false);
    let modifiers = if mods.is_empty() {
        Vec::new()
    } else {
        mods.split('+').collect()
    };
    for modifier in modifiers {
        match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "shift" => shift = true,
            "alt" | "option" => alt = true,
            other => return Err(format!("unknown modifier `{other}` in key `{text}`")),
        }
    }
    let mut code = parse_code(key).ok_or_else(|| format!("unknown key `{key}` in `{text}`"))?;
    if let KeyCode::Char(ch) = code
        && !ctrl
        && !alt
    {
        if ch.is_alphabetic() && shift {
            // A bare letter carries Shift in its case: `shift+g` is `G`.
            code = KeyCode::Char(ch.to_uppercase().next().unwrap_or(ch));
            shift = false;
        } else if shift && ch != ' ' {
            return Err(format!(
                "`{text}`: terminals report a shifted symbol as the symbol itself; \
                 bind the character Shift produces instead"
            ));
        }
    }
    Ok(chord(ctrl, shift, alt, code).canonical())
}

/// The key a chord's final token names, case-insensitively for named keys.
fn parse_code(key: &str) -> Option<KeyCode> {
    let lower = key.to_ascii_lowercase();
    let named = match lower.as_str() {
        "tab" => KeyCode::Tab,
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        _ => {
            if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
                && (1..=24).contains(&n)
            {
                return Some(KeyCode::F(n));
            }
            let mut chars = key.chars();
            return match (chars.next(), chars.next()) {
                (Some(ch), None) => Some(KeyCode::Char(ch)),
                _ => None,
            };
        },
    };
    Some(named)
}

#[cfg(test)]
mod tests;
