//! LSP snippet syntax → plain text degradation.
//!
//! This client does not advertise `snippetSupport`, but some servers send
//! snippet-format insert text anyway, so anything that leaks through is
//! degraded here rather than inserted verbatim. The grammar handled is the LSP
//! snippet grammar: tabstops (`$1`, `${1}`, `$0`), placeholders
//! (`${1:text}`, nested), choices (`${1|one,two|}`), variables (`$VAR`,
//! `${VAR}`, `${VAR:default}`) and variable transforms (`${VAR/re/fmt/}`),
//! plus the `\$`, `\}`, `\\` text escapes (and `\,`, `\|` inside choices).
//!
//! Degradation rules: tabstops vanish, placeholders keep their (recursively
//! degraded) text, choices keep their first option, variables keep their
//! default text or vanish (nothing is resolved here), transforms vanish, and
//! escapes become their literal character. Malformed syntax is left verbatim —
//! degrading must never lose more text than the snippet machinery itself.

/// Reduce LSP snippet `input` to the plain text a snippet engine would show
/// before any user interaction.
pub(crate) fn strip_snippet(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    parse_text(&chars, &mut i, &mut out, false);
    out
}

/// Parse snippet text into `out` until the end of input — or, when
/// `stop_at_brace` is set (inside `${…}`), until the matching unescaped `}`
/// (which is consumed).
fn parse_text(chars: &[char], i: &mut usize, out: &mut String, stop_at_brace: bool) {
    while *i < chars.len() {
        match chars[*i] {
            '\\' if *i + 1 < chars.len() && matches!(chars[*i + 1], '$' | '}' | '\\') => {
                out.push(chars[*i + 1]);
                *i += 2;
            },
            '}' if stop_at_brace => {
                *i += 1; // consume the terminator
                return;
            },
            '$' => parse_dollar(chars, i, out),
            c => {
                out.push(c);
                *i += 1;
            },
        }
    }
}

/// Parse one `$…` construct starting at `chars[*i] == '$'`.
fn parse_dollar(chars: &[char], i: &mut usize, out: &mut String) {
    let start = *i;
    *i += 1; // consume '$'
    match chars.get(*i) {
        // `$1` — a bare tabstop: vanish.
        Some(c) if c.is_ascii_digit() => {
            while chars.get(*i).is_some_and(char::is_ascii_digit) {
                *i += 1;
            }
        },
        // `$VAR` — a bare variable: nothing to resolve, vanish.
        Some(c) if c.is_ascii_alphabetic() || *c == '_' => {
            while chars
                .get(*i)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
            {
                *i += 1;
            }
        },
        Some('{') => {
            *i += 1; // consume '{'
            if !parse_braced(chars, i, out) {
                // Malformed: rewind and emit the `$` literally; the rest of the
                // text re-parses as ordinary characters.
                *i = start + 1;
                out.push('$');
            }
        },
        // A lone `$` (end of input or before punctuation): literal.
        _ => out.push('$'),
    }
}

/// Parse the interior of `${…}` with `*i` just past the `{`. Returns `false`
/// when the syntax is not a recognized construct (the caller then emits the
/// text literally).
fn parse_braced(chars: &[char], i: &mut usize, out: &mut String) -> bool {
    let is_digit = chars.get(*i).is_some_and(char::is_ascii_digit);
    let is_ident = chars
        .get(*i)
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_');
    if !is_digit && !is_ident {
        return false;
    }
    while chars.get(*i).is_some_and(|c| {
        if is_digit {
            c.is_ascii_digit()
        } else {
            c.is_ascii_alphanumeric() || *c == '_'
        }
    }) {
        *i += 1;
    }
    match chars.get(*i) {
        // `${1}` / `${VAR}` — vanish.
        Some('}') => {
            *i += 1;
            true
        },
        // `${1:placeholder}` / `${VAR:default}` — keep the degraded contents.
        Some(':') => {
            *i += 1;
            parse_text(chars, i, out, true);
            true
        },
        // `${1|one,two|}` — keep the first choice.
        Some('|') if is_digit => {
            *i += 1;
            parse_choice(chars, i, out);
            true
        },
        // `${VAR/regex/format/opts}` — unresolvable, vanish.
        Some('/') if is_ident => {
            skip_transform(chars, i);
            true
        },
        _ => false,
    }
}

/// Parse a choice body with `*i` just past the opening `|`: emit the first
/// option (unescaping `\,`, `\|`, `\\`) and consume through the closing `|}`.
fn parse_choice(chars: &[char], i: &mut usize, out: &mut String) {
    let mut first_done = false;
    while *i < chars.len() {
        match chars[*i] {
            '\\' if *i + 1 < chars.len() && matches!(chars[*i + 1], ',' | '|' | '\\') => {
                if !first_done {
                    out.push(chars[*i + 1]);
                }
                *i += 2;
            },
            ',' => {
                first_done = true;
                *i += 1;
            },
            '|' => {
                *i += 1;
                if chars.get(*i) == Some(&'}') {
                    *i += 1;
                }
                return;
            },
            c => {
                if !first_done {
                    out.push(c);
                }
                *i += 1;
            },
        }
    }
}

/// Skip a variable transform with `*i` at the `/`: consume (emitting nothing)
/// through the closing unescaped `}` or to the end of input.
fn skip_transform(chars: &[char], i: &mut usize) {
    while *i < chars.len() {
        match chars[*i] {
            '\\' if *i + 1 < chars.len() => *i += 2,
            '}' => {
                *i += 1;
                return;
            },
            _ => *i += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(strip_snippet("println!(\"hi\")"), "println!(\"hi\")");
        assert_eq!(strip_snippet(""), "");
    }

    #[test]
    fn tabstops_vanish() {
        assert_eq!(strip_snippet("foo($1)$0"), "foo()");
        assert_eq!(strip_snippet("${1}between${2}"), "between");
        assert_eq!(strip_snippet("$12end"), "end"); // multi-digit
    }

    #[test]
    fn placeholders_keep_their_text() {
        assert_eq!(strip_snippet("${1:value}"), "value");
        assert_eq!(
            strip_snippet("fn ${1:name}(${2:params}) {\n    $0\n}"),
            "fn name(params) {\n    \n}"
        );
    }

    #[test]
    fn nested_placeholders_degrade_recursively() {
        assert_eq!(strip_snippet("${1:foo ${2:bar} baz}"), "foo bar baz");
        assert_eq!(strip_snippet("${1:${2:${3:deep}}}"), "deep");
        // A tabstop nested in a placeholder vanishes; surrounding text stays.
        assert_eq!(strip_snippet("${1:a$2b}"), "ab");
    }

    #[test]
    fn choices_keep_the_first_option() {
        assert_eq!(strip_snippet("${1|one,two,three|}"), "one");
        assert_eq!(strip_snippet("align: ${2|left,right|};"), "align: left;");
        // Escaped separators inside a choice are literal.
        assert_eq!(strip_snippet("${1|a\\,b,c|}"), "a,b");
        assert_eq!(strip_snippet("${1|pipe\\|char,x|}"), "pipe|char");
    }

    #[test]
    fn variables_use_defaults_or_vanish() {
        assert_eq!(strip_snippet("$TM_FILENAME"), "");
        assert_eq!(strip_snippet("${TM_FILENAME}"), "");
        assert_eq!(strip_snippet("${TM_FILENAME:untitled}"), "untitled");
        assert_eq!(strip_snippet("hello $USER!"), "hello !");
        assert_eq!(strip_snippet("$_private rest"), " rest");
    }

    #[test]
    fn transforms_vanish() {
        assert_eq!(strip_snippet("${TM_FILENAME/(.*)\\..+$/$1/}"), "");
        assert_eq!(strip_snippet("a${VAR/x/y/g}b"), "ab");
    }

    #[test]
    fn escapes_become_literals() {
        assert_eq!(strip_snippet("\\$notatabstop"), "$notatabstop");
        assert_eq!(strip_snippet("\\\\$1"), "\\");
        assert_eq!(strip_snippet("${1:brace \\} inside}"), "brace } inside");
        // A backslash before anything else is an ordinary backslash.
        assert_eq!(strip_snippet("C:\\path"), "C:\\path");
    }

    #[test]
    fn lone_dollars_are_literal() {
        assert_eq!(strip_snippet("cost: $ 5"), "cost: $ 5");
        assert_eq!(strip_snippet("end$"), "end$");
        assert_eq!(strip_snippet("$-x"), "$-x");
    }

    #[test]
    fn malformed_braces_are_left_verbatim() {
        assert_eq!(strip_snippet("${?}"), "${?}");
        assert_eq!(strip_snippet("${}"), "${}");
        // Unclosed placeholder: lenient, the content still degrades.
        assert_eq!(strip_snippet("${1:oops"), "oops");
        // `${1x}` is neither a tabstop nor a valid placeholder.
        assert_eq!(strip_snippet("${1x}"), "${1x}");
    }

    #[test]
    fn realistic_server_snippets() {
        // rust-analyzer method call
        assert_eq!(strip_snippet("push(${1:ch})$0"), "push(ch)");
        // typescript-language-server import
        assert_eq!(
            strip_snippet("import { $1 } from \"${2:module}\";$0"),
            "import {  } from \"module\";"
        );
    }
}
