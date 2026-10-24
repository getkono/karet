//! End-to-end file round-trip exercised as an external consumer of `karet-cbor`:
//! encode a realistic value to a `.cbor` file, decode it to editable diagnostic
//! text, edit that text, and re-encode — mirroring what the editor does on open
//! and save.

use karet_cbor::CborError;
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
fn file_open_edit_save_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("doc.cbor");

    // Author a .cbor file (as some external tool would).
    let bytes = encode(&sample())?;
    std::fs::write(&path, &bytes)?;

    // "Open": decode the file to editable diagnostic text.
    let on_disk = std::fs::read(&path)?;
    let text = decode_to_text(&on_disk)?;
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
    let reencoded = encode_from_text(&edited)?;
    std::fs::write(&path, &reencoded)?;

    // Re-open from disk: the edit persisted and it is still valid CBOR.
    let value = decode(&std::fs::read(&path)?)?;
    let CborValue::Map(pairs) = value else {
        return Err(format!("expected a map, got {value:?}").into());
    };
    let age = pairs
        .iter()
        .find(|(k, _)| *k == CborValue::Text("age".to_string()))
        .map(|(_, v)| v);
    assert_eq!(age, Some(&CborValue::Integer(38)));
    // Everything else is unchanged (still eight entries).
    assert_eq!(pairs.len(), 8);
    Ok(())
}

#[test]
fn saving_an_out_of_range_integer_is_rejected() {
    // One past 2^64 - 1 parses as diagnostic text but has no CBOR encoding.
    assert!(matches!(
        encode_from_text("[18446744073709551616]"),
        Err(CborError::IntegerRange(18_446_744_073_709_551_616))
    ));
    assert!(matches!(
        encode_from_text("-18446744073709551617"),
        Err(CborError::IntegerRange(-18_446_744_073_709_551_617))
    ));
}

/// Text nesting `depth` containers, cycling array → map → tag, around `1`.
fn mixed_nesting(depth: usize) -> String {
    let (mut open, mut close) = (String::new(), String::new());
    for level in 0..depth {
        let (o, c) = match level % 3 {
            0 => ("[", "]"),
            1 => ("{\"k\": ", "}"),
            _ => ("24(", ")"),
        };
        open.push_str(o);
        close.insert_str(0, c);
    }
    format!("{open}1{close}")
}

#[test]
fn text_at_the_depth_limit_saves_and_reopens() -> Result<(), Box<dyn std::error::Error>> {
    // Parse, encode, decode, print, and drop all handle MAX_DEPTH levels, and
    // the decoder accepts every depth the parser does: a save never writes a
    // file the editor cannot reopen.
    let text = mixed_nesting(karet_cbor::MAX_DEPTH);
    let bytes = encode_from_text(&text)?;
    let reopened = decode_to_text(&bytes)?;
    assert_eq!(encode_from_text(&reopened)?, bytes);
    assert_eq!(
        karet_cbor::from_diagnostic(&reopened)?,
        karet_cbor::from_diagnostic(&text)?
    );
    Ok(())
}

#[test]
fn text_past_the_depth_limit_is_refused_before_saving() {
    assert!(matches!(
        encode_from_text(&mixed_nesting(karet_cbor::MAX_DEPTH + 1)),
        Err(CborError::Parse { .. })
    ));
}

#[test]
fn a_cbor_sequence_fails_to_open() -> Result<(), Box<dyn std::error::Error>> {
    // Opening only the first item would make the next save drop the rest.
    let mut bytes = encode(&sample())?;
    bytes.extend(encode(&CborValue::Integer(2))?);
    assert!(matches!(decode_to_text(&bytes), Err(CborError::Decode(_))));
    Ok(())
}

#[test]
fn truncated_cbor_fails_to_open() -> Result<(), Box<dyn std::error::Error>> {
    let bytes = encode(&sample())?;
    for len in 0..bytes.len() {
        assert!(
            decode_to_text(&bytes[..len]).is_err(),
            "a {len}-byte prefix of a {}-byte document should not decode",
            bytes.len()
        );
    }
    Ok(())
}
