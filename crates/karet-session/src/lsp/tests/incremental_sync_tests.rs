//! Incremental document sync through the server task: what reaches the server
//! must rebuild the buffer exactly, whichever way each change travelled.
//!
//! The server side is a mirror that applies every `didChange` the way a
//! server does, so the assertion is the one that matters: after an edit
//! sequence its text equals the buffer's.

use karet_text::TextBuffer;
use serde_json::Value;

use super::*;

/// The double's view of one document, kept from the notifications alone.
#[derive(Default)]
struct Mirror {
    text: String,
    version: i64,
    /// How many content changes carried a range, and how many the whole text.
    ranged: usize,
    whole: usize,
}

impl Mirror {
    /// Fold one observed message in; `false` for one that cannot apply.
    fn observe(&mut self, message: &Value) -> bool {
        let params = &message["params"];
        match message["method"].as_str() {
            Some("textDocument/didOpen") => {
                self.text = params["textDocument"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                self.version = params["textDocument"]["version"].as_i64().unwrap_or(-1);
            },
            Some("textDocument/didChange") => {
                for change in params["contentChanges"].as_array().into_iter().flatten() {
                    let text = change["text"].as_str().unwrap_or_default();
                    if change.get("range").is_none() {
                        self.whole += 1;
                        text.clone_into(&mut self.text);
                        continue;
                    }
                    self.ranged += 1;
                    let start = offset(&self.text, &change["range"]["start"]);
                    let end = offset(&self.text, &change["range"]["end"]);
                    let (Some(start), Some(end)) = (start, end) else {
                        return false;
                    };
                    if end < start {
                        return false;
                    }
                    self.text.replace_range(start..end, text);
                }
                self.version = params["textDocument"]["version"].as_i64().unwrap_or(-1);
            },
            _ => {},
        }
        true
    }
}

/// The byte offset of an LSP position (UTF-16 columns, `\n` lines).
fn offset(text: &str, position: &Value) -> Option<usize> {
    let line = position["line"].as_u64()?;
    let column = position["character"].as_u64()?;
    let mut start = 0;
    for _ in 0..line {
        start += text[start..].find('\n')? + 1;
    }
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |end| start + end);
    let mut units = 0;
    for (index, ch) in text[start..end].char_indices() {
        if units == column {
            return Some(start + index);
        }
        units += ch.len_utf16() as u64;
    }
    (units == column).then_some(end)
}

fn edit(start: (u32, u32), end: (u32, u32), new_text: &str) -> TextEdit {
    TextEdit {
        range: Range {
            start: LineCol::new(start.0, start.1),
            end: LineCol::new(end.0, end.1),
        },
        new_text: new_text.to_owned(),
    }
}

/// Open `buffer` on a server behaving as `behavior`, run `edits` through the
/// manager -- each `None` an undo -- and return the mirror once it has caught
/// up with the buffer's version.
async fn sync(
    behavior: Behavior,
    buffer: &mut TextBuffer,
    edits: Vec<Option<Vec<TextEdit>>>,
    forward: impl Fn(usize) -> bool,
) -> Result<Mirror, Box<dyn std::error::Error + Send + Sync>> {
    let path = crate::lsp::absolute_path(Path::new("/tmp/incremental.rs"));
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let (mut manager, _updates) = LspManager::new(LspSettings::default(), None, None, None);
    manager.set_connector(test_connector(
        behavior,
        Some(observed_tx),
        Arc::new(AtomicUsize::new(0)),
    ));
    let text = buffer.text();
    let _ = manager.document_opened(Some("rust"), Some("rust"), &path, buffer.version(), || text);
    for (index, step) in edits.into_iter().enumerate() {
        let applied = match step {
            Some(edits) => buffer
                .apply_simple(&Change::new(buffer.version(), edits))
                .ok(),
            None => buffer.undo(),
        }
        .ok_or("an edit in the sequence did not apply")?;
        // A change the session never queued, as when the command queue is full.
        if forward(index) {
            let _ = manager.document_changed(Some("rust"), &path, buffer, Some(&applied));
        }
    }
    let target = i64::try_from(buffer.version())?;
    let mut mirror = Mirror::default();
    while mirror.version != target {
        let message = tokio::time::timeout(Duration::from_secs(2), observed_rx.recv())
            .await?
            .ok_or("the server stopped before catching up")?;
        assert!(
            mirror.observe(&message),
            "a change did not apply: {message}"
        );
    }
    Ok(mirror)
}

fn sequence() -> Vec<Option<Vec<TextEdit>>> {
    vec![
        Some(vec![edit((0, 3), (0, 3), "😀")]),
        Some(vec![
            edit((1, 4), (1, 4), "x"),
            edit((0, 0), (0, 2), "pub fn"),
        ]),
        Some(vec![edit((1, 0), (2, 0), "")]),
        None,
        Some(vec![edit((2, 1), (2, 1), "\n// é𝄞\n")]),
    ]
}

/// The whole sequence lands inside one debounce window, so it reaches the
/// server as one composed `didChange`. Falsified by `PendingChange::coalesce`
/// keeping only the latest change's edits, or by sending columns in chars:
/// the emoji puts every later column on its line off by one.
#[tokio::test]
async fn an_incremental_server_rebuilds_the_buffer_from_ranges() -> TestResult {
    let mut buffer = TextBuffer::from_text("fn main() {\n    body\n}\n");
    let mirror = sync(Behavior::Incremental, &mut buffer, sequence(), |_| true).await?;
    assert_eq!(mirror.text, buffer.text());
    assert!(mirror.ranged > 0, "nothing travelled as a range");
    assert_eq!(mirror.whole, 0, "an edit fell back to the whole text");
    Ok(())
}

#[tokio::test]
async fn a_full_sync_server_still_gets_the_whole_text() -> TestResult {
    let mut buffer = TextBuffer::from_text("fn main() {\n    body\n}\n");
    let mirror = sync(Behavior::Normal, &mut buffer, sequence(), |_| true).await?;
    assert_eq!(mirror.text, buffer.text());
    assert_eq!(mirror.ranged, 0, "a full-sync server was sent a range");
    Ok(())
}

/// Falsified by dropping the base-version check in `remember_document`: the
/// change after the gap applies to text the server never had.
#[tokio::test]
async fn a_change_lost_before_the_task_is_repaired_by_the_whole_text() -> TestResult {
    let mut buffer = TextBuffer::from_text("fn main() {\n    body\n}\n");
    let mirror = sync(Behavior::Incremental, &mut buffer, sequence(), |index| {
        index != 1
    })
    .await?;
    assert_eq!(mirror.text, buffer.text());
    assert!(mirror.whole > 0, "the gap was bridged with a stale delta");
    Ok(())
}
