//! Rendering a [`CborValue`] as pretty CBOR diagnostic notation.

use std::fmt::Write as _;

use crate::value::CborValue;

/// One indentation level.
const INDENT: &str = "  ";

/// Render `value` as pretty, multi-line CBOR diagnostic notation.
///
/// Scalars render inline; arrays and maps render one element per line, indented.
/// The output round-trips through [`from_diagnostic`](super::from_diagnostic).
#[must_use]
pub fn to_diagnostic(value: &CborValue) -> String {
    let mut out = String::new();
    write_value(&mut out, value, 0);
    out
}

fn write_value(out: &mut String, value: &CborValue, depth: usize) {
    match value {
        CborValue::Null => out.push_str("null"),
        CborValue::Bool(true) => out.push_str("true"),
        CborValue::Bool(false) => out.push_str("false"),
        CborValue::Integer(i) => {
            let _ = write!(out, "{i}");
        },
        CborValue::Float(f) => out.push_str(&format_float(*f)),
        CborValue::Text(s) => write_text(out, s),
        CborValue::Bytes(bytes) => write_bytes(out, bytes),
        CborValue::Tag(tag, inner) => {
            let _ = write!(out, "{tag}(");
            write_value(out, inner, depth);
            out.push(')');
        },
        CborValue::Array(items) => write_array(out, items, depth),
        CborValue::Map(pairs) => write_map(out, pairs, depth),
    }
}

/// Format a float so it always reads back as a float (never as an integer), and
/// render the non-finite values with their diagnostic-notation spellings.
fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f < 0.0 {
            "-Infinity".to_string()
        } else {
            "Infinity".to_string()
        };
    }
    let s = format!("{f}");
    if s.contains(['.', 'e', 'E']) {
        s
    } else {
        format!("{s}.0")
    }
}

fn write_text(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            },
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_bytes(out: &mut String, bytes: &[u8]) {
    out.push_str("h'");
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out.push('\'');
}

fn write_array(out: &mut String, items: &[CborValue], depth: usize) {
    if items.is_empty() {
        out.push_str("[]");
        return;
    }
    out.push_str("[\n");
    let inner = depth + 1;
    for (i, item) in items.iter().enumerate() {
        push_indent(out, inner);
        write_value(out, item, inner);
        if i + 1 < items.len() {
            out.push(',');
        }
        out.push('\n');
    }
    push_indent(out, depth);
    out.push(']');
}

fn write_map(out: &mut String, pairs: &[(CborValue, CborValue)], depth: usize) {
    if pairs.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push_str("{\n");
    let inner = depth + 1;
    for (i, (key, value)) in pairs.iter().enumerate() {
        push_indent(out, inner);
        write_value(out, key, inner);
        out.push_str(": ");
        write_value(out, value, inner);
        if i + 1 < pairs.len() {
            out.push(',');
        }
        out.push('\n');
    }
    push_indent(out, depth);
    out.push('}');
}

fn push_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str(INDENT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars() {
        assert_eq!(to_diagnostic(&CborValue::Null), "null");
        assert_eq!(to_diagnostic(&CborValue::Bool(true)), "true");
        assert_eq!(to_diagnostic(&CborValue::Bool(false)), "false");
        assert_eq!(to_diagnostic(&CborValue::Integer(42)), "42");
        assert_eq!(to_diagnostic(&CborValue::Integer(-7)), "-7");
        assert_eq!(to_diagnostic(&CborValue::Float(1.0)), "1.0");
        assert_eq!(to_diagnostic(&CborValue::Text("hi".to_string())), "\"hi\"");
        assert_eq!(
            to_diagnostic(&CborValue::Bytes(vec![1, 2, 255])),
            "h'0102ff'"
        );
    }

    #[test]
    fn floats_are_distinguishable_from_integers() {
        assert_eq!(to_diagnostic(&CborValue::Float(1000.0)), "1000.0");
        assert_eq!(to_diagnostic(&CborValue::Float(f64::INFINITY)), "Infinity");
        assert_eq!(
            to_diagnostic(&CborValue::Float(f64::NEG_INFINITY)),
            "-Infinity"
        );
        assert_eq!(to_diagnostic(&CborValue::Float(f64::NAN)), "NaN");
    }

    #[test]
    fn escapes_control_characters() {
        assert_eq!(
            to_diagnostic(&CborValue::Text("a\n\"b".to_string())),
            "\"a\\n\\\"b\""
        );
    }

    #[test]
    fn tag() {
        let value = CborValue::Tag(0, Box::new(CborValue::Text("x".to_string())));
        assert_eq!(to_diagnostic(&value), "0(\"x\")");
    }

    #[test]
    fn empty_containers() {
        assert_eq!(to_diagnostic(&CborValue::Array(vec![])), "[]");
        assert_eq!(to_diagnostic(&CborValue::Map(vec![])), "{}");
    }

    #[test]
    fn nested_indentation() {
        let value = CborValue::Array(vec![CborValue::Integer(1), CborValue::Integer(2)]);
        assert_eq!(to_diagnostic(&value), "[\n  1,\n  2\n]");
        let map = CborValue::Map(vec![(
            CborValue::Text("a".to_string()),
            CborValue::Integer(1),
        )]);
        assert_eq!(to_diagnostic(&map), "{\n  \"a\": 1\n}");
    }
}
