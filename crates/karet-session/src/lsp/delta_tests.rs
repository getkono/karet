//! The server's copy of a document, rebuilt from the deltas alone, must equal
//! the buffer after every edit sequence -- multi-caret batches, undo and redo
//! groups, astral characters, and changes composed across the debounce.

use std::collections::HashMap;
use std::path::Path;

use karet_core::Change;
use karet_core::CursorState;
use karet_core::Selection;
use karet_text::EditCause;
use karet_text::EditContext;

use super::*;
use crate::lsp::commands::OpenDocument;
use crate::lsp::commands::remember_document;
use crate::lsp::message::ServerCmd;

/// Apply LSP content changes the way a server does: in order, UTF-16 columns,
/// lines broken at `\n`. `None` for a position the text does not have.
fn apply_lsp(text: &str, edits: &[TextEdit]) -> Option<String> {
    let mut out = text.to_owned();
    for edit in edits {
        let start = offset(&out, edit.range.start)?;
        let end = offset(&out, edit.range.end)?;
        if end < start {
            return None;
        }
        out.replace_range(start..end, &edit.new_text);
    }
    Some(out)
}

fn offset(text: &str, at: LineCol) -> Option<usize> {
    let mut line_start = 0;
    for _ in 0..at.line {
        line_start += text[line_start..].find('\n')? + 1;
    }
    let line_end = text[line_start..]
        .find('\n')
        .map_or(text.len(), |end| line_start + end);
    let mut units = 0;
    for (index, ch) in text[line_start..line_end].char_indices() {
        if units == at.col {
            return Some(line_start + index);
        }
        units += ch.len_utf16() as u32;
    }
    (units == at.col).then_some(line_end)
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

/// Apply `edits` (buffer columns) and check the deltas take `before` to the
/// buffer, returning them.
fn step(buffer: &mut TextBuffer, edits: Vec<TextEdit>) -> Option<Vec<TextEdit>> {
    let before = buffer.text();
    let applied = buffer
        .apply_simple(&Change::new(buffer.version(), edits))
        .ok()?;
    let changes = content_changes(buffer, &applied)?;
    assert_eq!(
        apply_lsp(&before, &changes).as_deref(),
        Some(buffer.text().as_str())
    );
    Some(changes)
}

#[test]
fn a_typed_character_is_one_utf16_range() {
    let mut buffer = TextBuffer::from_text("😀ab\ncd\n");
    // Buffer columns count chars: after the emoji and `a` is char 2, UTF-16 3.
    let changes = step(&mut buffer, vec![edit((0, 2), (0, 2), "x")]);
    assert_eq!(changes, Some(vec![edit((0, 3), (0, 3), "x")]));
}

#[test]
fn a_multi_caret_batch_on_one_line_lands_in_order() {
    let mut buffer = TextBuffer::from_text("a😀b😀c\n");
    let changes = step(
        &mut buffer,
        vec![
            edit((0, 0), (0, 1), "AA"),
            edit((0, 2), (0, 3), "𝄞"),
            edit((0, 4), (0, 5), ""),
        ],
    );
    assert_eq!(changes.map(|changes| changes.len()), Some(3));
    assert_eq!(buffer.text(), "AA😀𝄞😀\n");
}

#[test]
fn joining_and_splitting_lines_carries_the_replaced_newlines() {
    let mut buffer = TextBuffer::from_text("one\ntwo\nthree\n");
    assert!(step(&mut buffer, vec![edit((0, 2), (2, 1), "-\n-")]).is_some());
    assert!(step(&mut buffer, vec![edit((1, 0), (1, 0), "\n\n")]).is_some());
    // Now `on-`, two empty lines, then `-hree`.
    assert!(step(&mut buffer, vec![edit((0, 0), (3, 5), "")]).is_some());
    assert_eq!(buffer.text(), "\n");
}

#[test]
fn undoing_a_typed_run_is_one_frame_but_redoing_it_is_refused() {
    let mut buffer = TextBuffer::from_text("\n");
    for (col, ch) in ["a", "b", "c"].into_iter().enumerate() {
        let ctx = EditContext {
            tick_ms: col as u64,
            cause: EditCause::Type,
            cursor_before: CursorState::single(Selection::caret(LineCol::new(0, col as u32))),
        };
        let at = (0, col as u32);
        let change = Change::new(buffer.version(), vec![edit(at, at, ch)]);
        assert!(buffer.apply(&change, ctx).is_ok());
    }
    let before = buffer.text();
    // Deletions replayed top-down share the pre-undo frame.
    let undone = buffer
        .undo()
        .and_then(|applied| content_changes(&buffer, &applied));
    assert_eq!(undone.as_ref().map(Vec::len), Some(3), "one group of three");
    assert_eq!(
        apply_lsp(&before, &undone.unwrap_or_default()),
        Some(buffer.text())
    );
    assert_eq!(buffer.text(), "\n");
    // Insertions replayed bottom-up do not: each sits in the previous one's
    // result, so the delta is refused rather than mistranslated.
    let redone = buffer.redo();
    assert!(redone.is_some_and(|applied| content_changes(&buffer, &applied).is_none()));
}

/// A deterministic linear-congruential generator, so the sequence below is
/// adversarial and reproducible without a property-test dependency.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % n.max(1)
    }

    fn point(&mut self, buffer: &TextBuffer) -> (u32, u32) {
        let line = self.below(buffer.line_count());
        let len = buffer.line(line).map_or(0, |text| text.chars().count());
        (line as u32, self.below(len + 1) as u32)
    }
}

const PIECES: [&str; 8] = ["a", "😀", "\n", "é", "xy\nz", "", "𝄞b", "\n\n"];

#[test]
fn random_edit_sequences_keep_the_server_copy_equal_to_the_buffer() {
    for seed in 1..=8u64 {
        let mut rng = Lcg(seed);
        let mut buffer = TextBuffer::from_text("fn 😀() {\n    body\n}\n");
        // What the server holds, and what it held at the last flush.
        let mut server = buffer.text();
        let mut pending: Option<PendingChange> = None;
        let mut incremental = 0;
        for round in 0..400 {
            let before = buffer.text();
            let applied = match rng.below(6) {
                0 => buffer.undo(),
                1 => buffer.redo(),
                n => {
                    let mut points: Vec<_> =
                        (0..2 * n.min(3)).map(|_| rng.point(&buffer)).collect();
                    points.sort_unstable();
                    let edits = points
                        .chunks(2)
                        .map(|pair| edit(pair[0], pair[1], PIECES[rng.below(PIECES.len())]))
                        .collect();
                    buffer
                        .apply_simple(&Change::new(buffer.version(), edits))
                        .ok()
                },
            };
            let Some(applied) = applied else {
                continue;
            };
            let delta = content_changes(&buffer, &applied);
            if let Some(changes) = &delta {
                incremental += 1;
                assert_eq!(
                    apply_lsp(&before, changes),
                    Some(buffer.text()),
                    "seed {seed}, round {round}"
                );
            }
            let delta = delta.map(|edits| DocumentDelta {
                base_version: 0,
                edits,
            });
            pending = Some(PendingChange::coalesce(
                pending.take(),
                (PathBuf::from("/d"), 0, buffer.text()),
                delta,
            ));
            // Flush every few edits, as the debounce does.
            if rng.below(4) == 0
                && let Some(change) = pending.take()
            {
                server = match change.edits {
                    Some(edits) => apply_lsp(&server, &edits).unwrap_or_default(),
                    None => change.text,
                };
                assert_eq!(server, buffer.text(), "seed {seed}, round {round}");
            }
        }
        assert!(incremental > 200, "seed {seed}: only {incremental} deltas");
    }
}

#[test]
fn composing_falls_back_once_any_change_lacks_a_delta_or_outweighs_the_text() {
    let path = || PathBuf::from("/d");
    let delta = |text: &str| {
        Some(DocumentDelta {
            base_version: 0,
            edits: vec![edit((0, 0), (0, 0), text)],
        })
    };
    let first = PendingChange::coalesce(None, (path(), 1, "ab-long-text".into()), delta("a"));
    let both = PendingChange::coalesce(Some(first), (path(), 2, "ab-long-text".into()), delta("b"));
    assert_eq!(both.edits.as_ref().map(Vec::len), Some(2));
    let lost = PendingChange::coalesce(Some(both), (path(), 3, "ab-long-text".into()), None);
    assert_eq!(lost.edits, None);
    let still = PendingChange::coalesce(Some(lost), (path(), 4, "ab-long-text".into()), delta("c"));
    assert_eq!(still.edits, None, "a gap is never bridged by a later delta");
    let heavy = PendingChange::coalesce(None, (path(), 5, "ab".into()), delta("ab"));
    assert_eq!(heavy.edits, None);
}

#[test]
fn line_breaks_lsp_does_not_count_are_refused() {
    assert!(lines_agree("plain\ntext 😀"));
    for odd in [
        "a\rb",
        "a\r\nb",
        "\u{0B}",
        "page\u{0C}break",
        "\u{85}",
        "\u{2028}",
        "\u{2029}",
    ] {
        assert!(!lines_agree(odd), "{odd:?}");
    }
}

fn change(version: i32, text: &str, base: Option<i32>) -> ServerCmd {
    ServerCmd::DidChange {
        path: PathBuf::from("/d"),
        version,
        text: text.to_owned(),
        delta: base.map(|base_version| DocumentDelta {
            base_version,
            edits: vec![edit((0, 0), (0, 0), "x")],
        }),
    }
}

fn kept(cmd: &ServerCmd) -> bool {
    matches!(cmd, ServerCmd::DidChange { delta: Some(_), .. })
}

#[test]
fn the_replay_set_drops_a_delta_from_a_version_it_never_saw() {
    let mut documents = HashMap::<PathBuf, OpenDocument>::new();
    let mut unopened = change(2, "x", Some(1));
    remember_document(&mut documents, &mut unopened);
    assert!(!kept(&unopened), "a document the server never opened");

    let mut open = ServerCmd::DidOpen {
        path: PathBuf::from("/d"),
        language: "rust".into(),
        version: 1,
        text: String::new(),
    };
    remember_document(&mut documents, &mut open);
    let mut next = change(2, "x", Some(1));
    remember_document(&mut documents, &mut next);
    assert!(kept(&next));
    // Version 3 was never queued: the delta from it cannot apply to 2.
    let mut skipped = change(4, "xxx", Some(3));
    remember_document(&mut documents, &mut skipped);
    assert!(!kept(&skipped));
    // The full text resynced the server, so the next delta is good again.
    let mut resumed = change(5, "xxxx", Some(4));
    remember_document(&mut documents, &mut resumed);
    assert!(kept(&resumed));
    assert_eq!(
        documents.get(Path::new("/d")).map(|doc| doc.version),
        Some(5)
    );
}

#[test]
fn the_replay_set_drops_a_delta_across_a_line_break_lsp_does_not_count() {
    let mut documents = HashMap::<PathBuf, OpenDocument>::new();
    let mut open = ServerCmd::DidOpen {
        path: PathBuf::from("/d"),
        language: "c".into(),
        version: 1,
        text: "a\u{0C}b".into(),
    };
    remember_document(&mut documents, &mut open);
    // The form feed is deleted, but the delta was measured with it present.
    let mut removed = change(2, "ab", Some(1));
    remember_document(&mut documents, &mut removed);
    assert!(!kept(&removed));
    let mut clean = change(3, "xab", Some(2));
    remember_document(&mut documents, &mut clean);
    assert!(kept(&clean));
}

/// A connected client whose server answered the handshake (asking for
/// incremental sync) and then stopped reading, so the outbound queue fills
/// while the connection stays alive. The returned sender releases the server.
async fn stalled_client()
-> Result<(LspClient, tokio::sync::oneshot::Sender<()>), Box<dyn std::error::Error>> {
    use karet_jsonrpc::framing::content_length as codec;
    let (client_end, server_end) = tokio::io::duplex(256);
    let (client_read, client_write) = tokio::io::split(client_end);
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let (server_read, mut server_write) = tokio::io::split(server_end);
        let mut reader = tokio::io::BufReader::new(server_read);
        let bytes = codec::read_frame(&mut reader).await.ok()??;
        let init: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let reply = serde_json::json!({"jsonrpc": "2.0", "id": init["id"], "result": {
            "capabilities": {"textDocumentSync": {"openClose": true, "change": 2}}}});
        codec::write_frame(&mut server_write, reply.to_string().as_bytes())
            .await
            .ok()?;
        codec::read_frame(&mut reader).await.ok()??; // initialized
        // Hold the stream open, reading nothing more.
        let _ = released.await;
        Some(())
    });
    let client = LspClient::connect(client_read, client_write, Path::new("/")).await?;
    Ok((client, release))
}

/// Regression: a `didChange` that fails to send without closing the
/// connection (a full outbound queue) left the server's copy behind while the
/// replay set had already advanced its version, so the next delta passed the
/// base-version guard and was applied to stale server text.
#[tokio::test]
async fn a_change_that_failed_to_send_forces_the_next_one_whole()
-> Result<(), Box<dyn std::error::Error>> {
    use super::super::commands::flush_pending;
    use super::super::health::FailureTally;
    use super::super::slot::SlotKey;
    use super::super::slot::SlotToken;

    let (client, _release) = stalled_client().await?;
    let mut documents = HashMap::<PathBuf, OpenDocument>::new();
    let mut open = ServerCmd::DidOpen {
        path: PathBuf::from("/d"),
        language: "rust".into(),
        version: 1,
        text: String::new(),
    };
    remember_document(&mut documents, &mut open);
    client.did_open(Path::new("/d"), "rust", 1, "").await?;
    // Fill the queue: the server reads nothing, so it eventually refuses.
    let mut filled = false;
    for _ in 0..10_000 {
        if client.did_change(Path::new("/d"), 1, "").await.is_err() {
            filled = true;
            break;
        }
    }
    assert!(filled, "the outbound queue never filled");

    let mut first = change(2, "x", Some(1));
    remember_document(&mut documents, &mut first);
    assert!(kept(&first));
    let ServerCmd::DidChange {
        path,
        version,
        text,
        delta,
    } = first
    else {
        return Err("not a change".into());
    };
    let mut pending = Some(PendingChange::coalesce(None, (path, version, text), delta));
    let (updates, _rx) = tokio::sync::mpsc::unbounded_channel();
    let (mut tally, mut dead) = (FailureTally::default(), false);
    let key = SlotKey::new(crate::api::LanguageServerId::new("rust"), "/");
    let queue = (&mut pending, &mut documents);
    flush_pending(
        &client,
        queue,
        &mut dead,
        &mut tally,
        &updates,
        &key,
        SlotToken::default(),
    )
    .await;
    assert!(!dead, "a full queue is not a dead connection");
    assert!(pending.is_none());

    // The server never saw version 2, so version 3 must not travel as ranges.
    let mut next = change(3, "xx", Some(2));
    remember_document(&mut documents, &mut next);
    assert!(
        !kept(&next),
        "a delta against text the server never received"
    );
    // That whole text resynced the server, so deltas resume after it.
    let mut after = change(4, "xxx", Some(3));
    remember_document(&mut documents, &mut after);
    assert!(kept(&after));
    Ok(())
}
