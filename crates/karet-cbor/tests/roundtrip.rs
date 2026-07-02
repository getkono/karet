//! End-to-end file round-trip exercised as an external consumer of `karet-cbor`:
//! encode a realistic value to a `.cbor` file, decode it to editable diagnostic
//! text, edit that text, and re-encode — mirroring what the editor does on open
//! and save.

use karet_cbor::CborValue;
use karet_cbor::decode;
use karet_cbor::decode_to_text;
use karet_cbor::encode;
use karet_cbor::encode_from_text;

/// A realistic nested document exercising every value kind.
fn sample() -> CborValue {
    CborValue::Map(vec![
        (
            CborValue::Text("name".to_string()),
            CborValue::Text("ada".to_string()),
        ),
        (CborValue::Text("age".to_string()), CborValue::Integer(37)),
        (CborValue::Text("score".to_string()), CborValue::Float(9.5)),
        (CborValue::Text("active".to_string()), CborValue::Bool(true)),
        (CborValue::Text("nickname".to_string()), CborValue::Null),
        (
            CborValue::Text("avatar".to_string()),
            CborValue::Bytes(vec![0xde, 0xad, 0xbe, 0xef]),
        ),
        (
            CborValue::Text("created".to_string()),
            CborValue::Tag(
                0,
                Box::new(CborValue::Text("2020-01-01T00:00:00Z".to_string())),
            ),
        ),
        (
            CborValue::Text("tags".to_string()),
            CborValue::Array(vec![
                CborValue::Text("x".to_string()),
                CborValue::Integer(1),
            ]),
        ),
    ])
}

#[test]
fn file_open_edit_save_round_trip() {
    let Ok(dir) = tempfile::tempdir() else {
        return;
    };
    let path = dir.path().join("doc.cbor");

    // Author a .cbor file (as some external tool would).
    let encoded = encode(&sample());
    assert!(encoded.is_ok(), "encode failed");
    let Some(bytes) = encoded.ok() else { return };
    if std::fs::write(&path, &bytes).is_err() {
        return;
    }

    // "Open": decode the file to editable diagnostic text.
    let on_disk = std::fs::read(&path).unwrap_or_default();
    let decoded = decode_to_text(&on_disk);
    assert!(decoded.is_ok(), "decode_to_text failed");
    let Some(text) = decoded.ok() else { return };
    println!(
        "--- decoded {} ({} bytes) ---\n{text}",
        path.display(),
        on_disk.len()
    );

    // The decode is human-readable and lossless: byte strings, tags, and the
    // float all survive in a form JSON could not represent.
    assert!(text.contains("\"name\": \"ada\""));
    assert!(text.contains("h'deadbeef'"));
    assert!(text.contains("0(\"2020-01-01T00:00:00Z\")"));
    assert!(text.contains("9.5"));

    // "Edit": bump the age 37 → 38 in the text.
    let edited = text.replace("\"age\": 37", "\"age\": 38");
    assert_ne!(edited, text, "the edit should change the text");

    // "Save": re-encode and write back.
    let reencoded = encode_from_text(&edited);
    assert!(reencoded.is_ok(), "encode_from_text failed");
    let Some(reencoded) = reencoded.ok() else {
        return;
    };
    if std::fs::write(&path, &reencoded).is_err() {
        return;
    }

    // Re-open from disk: the edit persisted and it is still valid CBOR.
    let reloaded = decode(&std::fs::read(&path).unwrap_or_default());
    assert!(reloaded.is_ok(), "re-decode failed");
    let Some(value) = reloaded.ok() else { return };
    assert!(matches!(value, CborValue::Map(_)), "expected a map");
    let CborValue::Map(pairs) = value else { return };
    let age = pairs
        .iter()
        .find(|(k, _)| *k == CborValue::Text("age".to_string()))
        .map(|(_, v)| v);
    assert_eq!(age, Some(&CborValue::Integer(38)));
    // Everything else is unchanged (still eight entries).
    assert_eq!(pairs.len(), 8);
}
