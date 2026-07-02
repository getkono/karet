//! Parsing CBOR diagnostic notation back into a [`CborValue`].
//!
//! This accepts the canonical form produced by [`to_diagnostic`](super::to_diagnostic)
//! plus lenient whitespace and optional trailing commas — enough to re-parse a
//! hand-edited buffer. It does not implement every diagnostic-notation dialect.

use crate::error::CborError;
use crate::value::CborValue;

/// Parse pretty CBOR diagnostic notation into a [`CborValue`].
///
/// # Errors
/// Returns [`CborError::Parse`] (with a line/column) if `text` is malformed.
pub fn from_diagnostic(text: &str) -> Result<CborValue, CborError> {
    let mut parser = Parser::new(text);
    parser.skip_ws();
    let value = parser.parse_value()?;
    parser.skip_ws();
    if !parser.at_end() {
        return Err(parser.error("unexpected trailing characters"));
    }
    Ok(value)
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn new(text: &str) -> Self {
        Self {
            chars: text.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn expect(&mut self, ch: char) -> Result<(), CborError> {
        match self.peek() {
            Some(c) if c == ch => {
                self.pos += 1;
                Ok(())
            },
            _ => Err(self.error(&format!("expected '{ch}'"))),
        }
    }

    fn parse_value(&mut self) -> Result<CborValue, CborError> {
        self.skip_ws();
        let c = self
            .peek()
            .ok_or_else(|| self.error("unexpected end of input"))?;
        match c {
            '[' => self.parse_array(),
            '{' => self.parse_map(),
            '"' => Ok(CborValue::Text(self.parse_string()?)),
            'h' => self.parse_bytes(),
            't' | 'f' => self.parse_bool(),
            'n' => self.parse_null(),
            'N' | 'I' => self.parse_number_or_tag(),
            c if c == '-' || c.is_ascii_digit() => self.parse_number_or_tag(),
            _ => Err(self.error("unexpected character")),
        }
    }

    fn parse_array(&mut self) -> Result<CborValue, CborError> {
        self.expect('[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(']') {
            self.bump();
            return Ok(CborValue::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.bump();
                    self.skip_ws();
                    if self.peek() == Some(']') {
                        self.bump();
                        break;
                    }
                },
                Some(']') => {
                    self.bump();
                    break;
                },
                _ => return Err(self.error("expected ',' or ']' in array")),
            }
        }
        Ok(CborValue::Array(items))
    }

    fn parse_map(&mut self) -> Result<CborValue, CborError> {
        self.expect('{')?;
        let mut pairs = Vec::new();
        self.skip_ws();
        if self.peek() == Some('}') {
            self.bump();
            return Ok(CborValue::Map(pairs));
        }
        loop {
            let key = self.parse_value()?;
            self.skip_ws();
            self.expect(':')?;
            let value = self.parse_value()?;
            pairs.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.bump();
                    self.skip_ws();
                    if self.peek() == Some('}') {
                        self.bump();
                        break;
                    }
                },
                Some('}') => {
                    self.bump();
                    break;
                },
                _ => return Err(self.error("expected ',' or '}' in map")),
            }
        }
        Ok(CborValue::Map(pairs))
    }

    fn parse_string(&mut self) -> Result<String, CborError> {
        let start = self.pos;
        self.expect('"')?;
        let mut s = String::new();
        loop {
            let c = self
                .bump()
                .ok_or_else(|| self.error_at(start, "unterminated string"))?;
            match c {
                '"' => return Ok(s),
                '\\' => {
                    let e = self
                        .bump()
                        .ok_or_else(|| self.error_at(start, "unterminated escape"))?;
                    match e {
                        '"' => s.push('"'),
                        '\\' => s.push('\\'),
                        '/' => s.push('/'),
                        'n' => s.push('\n'),
                        'r' => s.push('\r'),
                        't' => s.push('\t'),
                        'b' => s.push('\u{08}'),
                        'f' => s.push('\u{0c}'),
                        'u' => s.push(self.parse_unicode_escape()?),
                        _ => return Err(self.error("invalid escape sequence")),
                    }
                },
                c => s.push(c),
            }
        }
    }

    /// Parse the four hex digits after `\u`, joining a surrogate pair if present.
    fn parse_unicode_escape(&mut self) -> Result<char, CborError> {
        let cp = self.read_hex4()?;
        if (0xD800..=0xDBFF).contains(&cp) {
            self.expect('\\')?;
            self.expect('u')?;
            let low = self.read_hex4()?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return Err(self.error("invalid low surrogate in \\u escape"));
            }
            let combined = 0x10000 + ((cp - 0xD800) << 10) + (low - 0xDC00);
            char::from_u32(combined).ok_or_else(|| self.error("invalid unicode escape"))
        } else {
            char::from_u32(cp).ok_or_else(|| self.error("invalid unicode escape"))
        }
    }

    fn read_hex4(&mut self) -> Result<u32, CborError> {
        let mut value = 0u32;
        for _ in 0..4 {
            let c = self
                .bump()
                .ok_or_else(|| self.error("truncated \\u escape"))?;
            let digit = c
                .to_digit(16)
                .ok_or_else(|| self.error("invalid hex digit in \\u escape"))?;
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn parse_bytes(&mut self) -> Result<CborValue, CborError> {
        let start = self.pos;
        self.expect('h')?;
        self.expect('\'')?;
        let mut hex = String::new();
        loop {
            let c = self
                .bump()
                .ok_or_else(|| self.error_at(start, "unterminated byte string"))?;
            if c == '\'' {
                break;
            }
            if c.is_whitespace() {
                continue;
            }
            if !c.is_ascii_hexdigit() {
                return Err(self.error("invalid hex digit in byte string"));
            }
            hex.push(c);
        }
        if !hex.len().is_multiple_of(2) {
            return Err(self.error_at(start, "odd number of hex digits in byte string"));
        }
        let bytes = hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let hi = (pair[0] as char).to_digit(16).unwrap_or(0);
                let lo = (pair[1] as char).to_digit(16).unwrap_or(0);
                ((hi << 4) | lo) as u8
            })
            .collect();
        Ok(CborValue::Bytes(bytes))
    }

    fn parse_bool(&mut self) -> Result<CborValue, CborError> {
        if self.consume_keyword("true") {
            Ok(CborValue::Bool(true))
        } else if self.consume_keyword("false") {
            Ok(CborValue::Bool(false))
        } else {
            Err(self.error("invalid literal"))
        }
    }

    fn parse_null(&mut self) -> Result<CborValue, CborError> {
        if self.consume_keyword("null") {
            Ok(CborValue::Null)
        } else {
            Err(self.error("invalid literal"))
        }
    }

    /// Parse a number, or — when an unsigned integer is immediately followed by
    /// `(` — a tagged value.
    fn parse_number_or_tag(&mut self) -> Result<CborValue, CborError> {
        let start = self.pos;
        let word = self.read_number_word();
        if word.is_empty() {
            return Err(self.error_at(start, "expected a number"));
        }
        let after_word = self.pos;
        self.skip_ws();
        if self.peek() == Some('(') {
            let tag: u64 = word
                .parse()
                .map_err(|_| self.error_at(start, "invalid tag number"))?;
            self.bump(); // consume '('
            let inner = self.parse_value()?;
            self.skip_ws();
            self.expect(')')?;
            return Ok(CborValue::Tag(tag, Box::new(inner)));
        }
        self.pos = after_word;
        interpret_number(&word).ok_or_else(|| self.error_at(start, "invalid number"))
    }

    fn read_number_word(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.' {
                s.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        s
    }

    fn consume_keyword(&mut self, keyword: &str) -> bool {
        let len = keyword.chars().count();
        if self.pos + len > self.chars.len() {
            return false;
        }
        if !self.chars[self.pos..self.pos + len]
            .iter()
            .copied()
            .eq(keyword.chars())
        {
            return false;
        }
        // Require a word boundary so `trueish` is not read as `true`.
        if self
            .chars
            .get(self.pos + len)
            .is_some_and(char::is_ascii_alphanumeric)
        {
            return false;
        }
        self.pos += len;
        true
    }

    fn error(&self, message: &str) -> CborError {
        self.error_at(self.pos, message)
    }

    fn error_at(&self, pos: usize, message: &str) -> CborError {
        let mut line = 1;
        let mut column = 1;
        for &c in self.chars.iter().take(pos) {
            if c == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        CborError::Parse {
            line,
            column,
            message: message.to_string(),
        }
    }
}

/// Interpret a bareword number token as an integer or a float.
fn interpret_number(word: &str) -> Option<CborValue> {
    match word {
        "NaN" => return Some(CborValue::Float(f64::NAN)),
        "Infinity" => return Some(CborValue::Float(f64::INFINITY)),
        "-Infinity" => return Some(CborValue::Float(f64::NEG_INFINITY)),
        _ => {},
    }
    if word.contains(['.', 'e', 'E']) {
        word.parse::<f64>().ok().map(CborValue::Float)
    } else {
        word.parse::<i128>().ok().map(CborValue::Integer)
    }
}

#[cfg(test)]
mod tests {
    use super::super::to_diagnostic;
    use super::*;

    fn round_trip(value: CborValue) {
        let text = to_diagnostic(&value);
        assert_eq!(
            from_diagnostic(&text).ok(),
            Some(value),
            "round trip failed for: {text}"
        );
    }

    #[test]
    fn round_trips_scalars() {
        round_trip(CborValue::Null);
        round_trip(CborValue::Bool(true));
        round_trip(CborValue::Bool(false));
        round_trip(CborValue::Integer(0));
        round_trip(CborValue::Integer(-12345));
        round_trip(CborValue::Integer(i64::MAX as i128 + 1));
        round_trip(CborValue::Float(1.5));
        round_trip(CborValue::Float(1000.0));
        round_trip(CborValue::Float(-0.25));
        round_trip(CborValue::Float(f64::INFINITY));
        round_trip(CborValue::Float(f64::NEG_INFINITY));
        round_trip(CborValue::Text("hello \"world\"\n\t".to_string()));
        round_trip(CborValue::Text("unicode: café ☕".to_string()));
        round_trip(CborValue::Bytes(vec![0, 1, 2, 250, 255]));
    }

    #[test]
    fn round_trips_containers() {
        round_trip(CborValue::Array(vec![]));
        round_trip(CborValue::Map(vec![]));
        round_trip(CborValue::Tag(1363896240, Box::new(CborValue::Integer(1))));
        round_trip(CborValue::Map(vec![
            (
                CborValue::Text("name".to_string()),
                CborValue::Text("ada".to_string()),
            ),
            (CborValue::Integer(1), CborValue::Bytes(vec![0xde, 0xad])),
            (
                CborValue::Text("nested".to_string()),
                CborValue::Array(vec![
                    CborValue::Bool(true),
                    CborValue::Null,
                    CborValue::Tag(0, Box::new(CborValue::Text("2020-01-01".to_string()))),
                ]),
            ),
        ]));
    }

    #[test]
    fn tolerates_trailing_commas_and_whitespace() {
        assert_eq!(
            from_diagnostic("[ 1, 2, ]").ok(),
            Some(CborValue::Array(vec![
                CborValue::Integer(1),
                CborValue::Integer(2)
            ]))
        );
        assert_eq!(
            from_diagnostic("{\n  \"a\" : 1 ,\n}").ok(),
            Some(CborValue::Map(vec![(
                CborValue::Text("a".to_string()),
                CborValue::Integer(1)
            )]))
        );
    }

    #[test]
    fn integer_key_and_tag() {
        assert_eq!(
            from_diagnostic("42(\"x\")").ok(),
            Some(CborValue::Tag(
                42,
                Box::new(CborValue::Text("x".to_string()))
            ))
        );
        assert_eq!(
            from_diagnostic("{5: false}").ok(),
            Some(CborValue::Map(vec![(
                CborValue::Integer(5),
                CborValue::Bool(false)
            )]))
        );
    }

    #[test]
    fn reports_error_position() {
        assert!(matches!(
            from_diagnostic("@"),
            Err(CborError::Parse {
                line: 1,
                column: 1,
                ..
            })
        ));
        assert!(matches!(
            from_diagnostic("[1, 2"),
            Err(CborError::Parse { .. })
        ));
        assert!(matches!(
            from_diagnostic("1 2"),
            Err(CborError::Parse { line: 1, .. })
        ));
        assert!(matches!(
            from_diagnostic("h'0'"),
            Err(CborError::Parse { .. })
        ));
    }
}
