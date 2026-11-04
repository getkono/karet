//! LSP snippet syntax → the text a snippet inserts, and where its stops sit.
//!
//! The grammar handled is the LSP snippet grammar: tabstops (`$1`, `${1}`,
//! `$0`), placeholders (`${1:text}`, nested), choices (`${1|one,two|}`),
//! variables (`$VAR`, `${VAR}`, `${VAR:default}`) and variable transforms
//! (`${VAR/re/fmt/}`), plus the `\$`, `\}`, `\\` text escapes (and `\,`, `\|`
//! inside choices).
//!
//! The inserted text is what a snippet shows before anyone edits it:
//! placeholders keep their (recursively expanded) text, choices their first
//! option, and a bare tabstop mirrors the text of the placeholder or choice
//! with its number, if there is one. Variables keep their default text or
//! vanish (nothing is resolved here), transforms vanish, and escapes become
//! their literal character. Malformed syntax is left verbatim -- parsing must
//! never lose more text than the snippet machinery itself.
//!
//! The stops come out in the order a session visits them (`$1`, `$2`, …,
//! the final `$0` last), each with every span its number occupies in the
//! text. A snippet with stops but no `$0` gets one at its end, where a session
//! leaves the caret; a snippet with no stops at all has none.

use std::collections::BTreeMap;
use std::collections::HashMap;

use karet_core::Tabstop;

/// A parsed snippet: the text it inserts and its stops.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Snippet {
    /// What the snippet inserts before any user interaction.
    pub text: String,
    /// Its stops, in visiting order (see the module docs).
    pub tabstops: Vec<Tabstop>,
}

/// One piece of a parsed snippet.
#[derive(Debug)]
enum Node {
    /// Literal text.
    Text(String),
    /// A tabstop, a placeholder (`body`), or a choice (`choices`).
    Stop {
        index: u32,
        body: Option<Vec<Node>>,
        choices: Vec<String>,
    },
}

/// Parse LSP snippet `input` into the text it inserts and its stops.
pub(crate) fn parse_snippet(input: &str) -> Snippet {
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let nodes = parse_text(&chars, &mut i, false);
    let mut defs = HashMap::new();
    collect_defs(&nodes, &mut defs);
    let mut render = Render {
        defs,
        ..Render::default()
    };
    render.nodes(&nodes);
    render.finish()
}

/// Every stop number's mirrored text: the plain text of its first placeholder
/// or the first option of its first choice, in document order.
fn collect_defs(nodes: &[Node], defs: &mut HashMap<u32, String>) {
    for node in nodes {
        if let Node::Stop {
            index,
            body,
            choices,
        } = node
        {
            let text = match (body, choices.first()) {
                (Some(body), _) => Some(plain(body)),
                (None, Some(first)) => Some(first.clone()),
                (None, None) => None,
            };
            if let Some(text) = text {
                defs.entry(*index).or_insert(text);
            }
            if let Some(body) = body {
                collect_defs(body, defs);
            }
        }
    }
}

/// The text `nodes` show, a bare tabstop inside them showing nothing (a
/// mirror inside a mirror is not followed, so this always ends).
fn plain(nodes: &[Node]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(text),
            Node::Stop {
                body: Some(body), ..
            } => out.push_str(&plain(body)),
            Node::Stop { choices, .. } => {
                out.push_str(choices.first().map_or("", String::as_str));
            },
        }
    }
    out
}

/// The rendering pass: text out, spans recorded per stop number.
#[derive(Default)]
struct Render {
    defs: HashMap<u32, String>,
    text: String,
    len: u32,
    spans: BTreeMap<u32, Vec<(u32, u32)>>,
    choices: HashMap<u32, Vec<String>>,
    /// The placeholders being rendered, innermost last: a mirror of one of
    /// them inside it shows nothing rather than repeating its own text.
    open: Vec<u32>,
}

impl Render {
    fn push(&mut self, text: &str) {
        self.text.push_str(text);
        let count = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
        self.len = self.len.saturating_add(count);
    }

    fn nodes(&mut self, nodes: &[Node]) {
        for node in nodes {
            match node {
                Node::Text(text) => self.push(text),
                Node::Stop {
                    index,
                    body,
                    choices,
                } => {
                    let start = self.len;
                    if let Some(body) = body {
                        self.open.push(*index);
                        self.nodes(body);
                        self.open.pop();
                    } else if let Some(first) = choices.first() {
                        self.push(first);
                        self.choices
                            .entry(*index)
                            .or_insert_with(|| choices.clone());
                    } else if let Some(mirror) = self
                        .defs
                        .get(index)
                        .filter(|_| !self.open.contains(index))
                        .cloned()
                    {
                        self.push(&mirror);
                    }
                    self.spans
                        .entry(*index)
                        .or_default()
                        .push((start, self.len));
                },
            }
        }
    }

    fn finish(mut self) -> Snippet {
        if self.spans.is_empty() {
            return Snippet {
                text: self.text,
                tabstops: Vec::new(),
            };
        }
        let end = self.len;
        let last = self.spans.remove(&0).unwrap_or_else(|| vec![(end, end)]);
        let tabstops = self
            .spans
            .into_iter()
            .chain(std::iter::once((0, last)))
            .map(|(index, mut spans)| {
                spans.sort_unstable();
                spans.dedup();
                Tabstop {
                    index,
                    spans,
                    choices: self.choices.remove(&index).unwrap_or_default(),
                }
            })
            .collect();
        Snippet {
            text: self.text,
            tabstops,
        }
    }
}

/// Flush pending literal text into `nodes`.
fn flush(text: &mut String, nodes: &mut Vec<Node>) {
    if !text.is_empty() {
        nodes.push(Node::Text(std::mem::take(text)));
    }
}

/// Parse snippet text until the end of input -- or, when `stop_at_brace` is
/// set (inside `${…}`), until the matching unescaped `}` (which is consumed).
fn parse_text(chars: &[char], i: &mut usize, stop_at_brace: bool) -> Vec<Node> {
    let mut nodes = Vec::new();
    let mut text = String::new();
    while *i < chars.len() {
        match chars[*i] {
            '\\' if *i + 1 < chars.len() && matches!(chars[*i + 1], '$' | '}' | '\\') => {
                text.push(chars[*i + 1]);
                *i += 2;
            },
            '}' if stop_at_brace => {
                *i += 1; // consume the terminator
                break;
            },
            '$' => {
                flush(&mut text, &mut nodes);
                parse_dollar(chars, i, &mut nodes, &mut text);
            },
            c => {
                text.push(c);
                *i += 1;
            },
        }
    }
    flush(&mut text, &mut nodes);
    nodes
}

/// The number of a tabstop starting at `chars[*i]`, consuming its digits.
fn parse_index(chars: &[char], i: &mut usize) -> u32 {
    let mut index: u32 = 0;
    while let Some(digit) = chars.get(*i).and_then(|c| c.to_digit(10)) {
        index = index.saturating_mul(10).saturating_add(digit);
        *i += 1;
    }
    index
}

/// Whether `c` may continue a variable name.
fn is_name_char(c: &char) -> bool {
    c.is_ascii_alphanumeric() || *c == '_'
}

/// Parse one `$…` construct starting at `chars[*i] == '$'`, into `nodes`, or
/// into `text` where it is literal.
fn parse_dollar(chars: &[char], i: &mut usize, nodes: &mut Vec<Node>, text: &mut String) {
    let start = *i;
    *i += 1; // consume '$'
    match chars.get(*i) {
        // `$1` -- a bare tabstop.
        Some(c) if c.is_ascii_digit() => nodes.push(Node::Stop {
            index: parse_index(chars, i),
            body: None,
            choices: Vec::new(),
        }),
        // `$VAR` -- a bare variable: nothing to resolve, vanish.
        Some(c) if c.is_ascii_alphabetic() || *c == '_' => {
            while chars.get(*i).is_some_and(is_name_char) {
                *i += 1;
            }
        },
        Some('{') => {
            *i += 1; // consume '{'
            if !parse_braced(chars, i, nodes) {
                // Malformed: rewind and emit the `$` literally; the rest of the
                // text re-parses as ordinary characters.
                *i = start + 1;
                text.push('$');
            }
        },
        // A lone `$` (end of input or before punctuation): literal.
        _ => text.push('$'),
    }
}

/// Parse the interior of `${…}` with `*i` just past the `{`. Returns `false`
/// when the syntax is not a recognized construct (the caller then emits the
/// text literally).
fn parse_braced(chars: &[char], i: &mut usize, nodes: &mut Vec<Node>) -> bool {
    let is_digit = chars.get(*i).is_some_and(char::is_ascii_digit);
    let is_ident = chars
        .get(*i)
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == '_');
    let index = if is_digit {
        Some(parse_index(chars, i))
    } else if is_ident {
        while chars.get(*i).is_some_and(is_name_char) {
            *i += 1;
        }
        None
    } else {
        return false;
    };
    let stop = |body, choices| Node::Stop {
        index: index.unwrap_or_default(),
        body,
        choices,
    };
    match chars.get(*i) {
        // `${1}` is a tabstop; `${VAR}` vanishes.
        Some('}') => {
            *i += 1;
            if index.is_some() {
                nodes.push(stop(None, Vec::new()));
            }
            true
        },
        // `${1:placeholder}`; `${VAR:default}` inlines its default.
        Some(':') => {
            *i += 1;
            let body = parse_text(chars, i, true);
            if index.is_some() {
                nodes.push(stop(Some(body), Vec::new()));
            } else {
                nodes.extend(body);
            }
            true
        },
        // `${1|one,two|}` -- a choice.
        Some('|') if is_digit => {
            *i += 1;
            nodes.push(stop(None, parse_choice(chars, i)));
            true
        },
        // `${VAR/regex/format/opts}` -- unresolvable, vanish.
        Some('/') if is_ident => {
            skip_transform(chars, i);
            true
        },
        _ => false,
    }
}

/// Parse a choice body with `*i` just past the opening `|`: its options
/// (unescaping `\,`, `\|`, `\\`), consuming through the closing `|}`.
fn parse_choice(chars: &[char], i: &mut usize) -> Vec<String> {
    let mut options = vec![String::new()];
    while *i < chars.len() {
        match chars[*i] {
            '\\' if *i + 1 < chars.len() && matches!(chars[*i + 1], ',' | '|' | '\\') => {
                if let Some(option) = options.last_mut() {
                    option.push(chars[*i + 1]);
                }
                *i += 2;
            },
            ',' => {
                options.push(String::new());
                *i += 1;
            },
            '|' => {
                *i += 1;
                if chars.get(*i) == Some(&'}') {
                    *i += 1;
                }
                break;
            },
            c => {
                if let Some(option) = options.last_mut() {
                    option.push(c);
                }
                *i += 1;
            },
        }
    }
    options
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

    fn text(input: &str) -> String {
        parse_snippet(input).text
    }

    fn stop(index: u32, spans: &[(u32, u32)]) -> Tabstop {
        Tabstop {
            index,
            spans: spans.to_vec(),
            choices: Vec::new(),
        }
    }

    #[test]
    fn plain_text_is_unchanged_and_has_no_stops() {
        assert_eq!(
            parse_snippet("println!(\"hi\")"),
            Snippet {
                text: "println!(\"hi\")".to_owned(),
                tabstops: Vec::new(),
            }
        );
        assert_eq!(parse_snippet(""), Snippet::default());
    }

    #[test]
    fn tabstops_insert_nothing_and_mark_their_spot() {
        let parsed = parse_snippet("foo($1)$0");
        assert_eq!(parsed.text, "foo()");
        assert_eq!(
            parsed.tabstops,
            vec![stop(1, &[(4, 4)]), stop(0, &[(5, 5)])]
        );
        assert_eq!(text("${1}between${2}"), "between");
        assert_eq!(text("$12end"), "end"); // multi-digit
        assert_eq!(parse_snippet("$12end").tabstops[0].index, 12);
    }

    /// Falsified by stops visited in text order, or `$0` anywhere but last.
    #[test]
    fn stops_are_visited_by_number_with_the_final_stop_last() {
        let parsed = parse_snippet("$0a$2b$1");
        assert_eq!(parsed.text, "ab");
        let order: Vec<u32> = parsed.tabstops.iter().map(|t| t.index).collect();
        assert_eq!(order, vec![1, 2, 0]);
    }

    /// A snippet without `$0` ends where its text does; one without any stop
    /// has nothing for a session to visit.
    #[test]
    fn a_missing_final_stop_lands_at_the_end() {
        let parsed = parse_snippet("push(${1:ch})");
        assert_eq!(parsed.text, "push(ch)");
        assert_eq!(
            parsed.tabstops,
            vec![stop(1, &[(5, 7)]), stop(0, &[(8, 8)])]
        );
        assert!(parse_snippet("\\$1 $HOME").tabstops.is_empty());
    }

    #[test]
    fn placeholders_keep_their_text_and_span_it() {
        let parsed = parse_snippet("fn ${1:name}(${2:params}) {\n    $0\n}");
        assert_eq!(parsed.text, "fn name(params) {\n    \n}");
        assert_eq!(
            parsed.tabstops,
            vec![
                stop(1, &[(3, 7)]),
                stop(2, &[(8, 14)]),
                stop(0, &[(22, 22)])
            ]
        );
    }

    #[test]
    fn nested_placeholders_expand_recursively() {
        let parsed = parse_snippet("${1:foo ${2:bar} baz}");
        assert_eq!(parsed.text, "foo bar baz");
        assert_eq!(
            parsed.tabstops[..2],
            [stop(1, &[(0, 11)]), stop(2, &[(4, 7)])]
        );
        assert_eq!(text("${1:${2:${3:deep}}}"), "deep");
        // A bare tabstop nested in a placeholder inserts nothing.
        assert_eq!(text("${1:a$2b}"), "ab");
    }

    /// Falsified by a mirror left empty, or one taking its text only when the
    /// placeholder comes first.
    #[test]
    fn a_bare_tabstop_mirrors_its_placeholder_wherever_it_is() {
        let parsed = parse_snippet("${1:x} = $1; $1 + ${2:y}");
        assert_eq!(parsed.text, "x = x; x + y");
        assert_eq!(parsed.tabstops[0], stop(1, &[(0, 1), (4, 5), (7, 8)]));
        let before = parse_snippet("$1 ${1:val}");
        assert_eq!(before.text, "val val");
        assert_eq!(before.tabstops[0], stop(1, &[(0, 3), (4, 7)]));
        // A mirror inside its own placeholder does not recurse.
        assert_eq!(text("${1:a$1}"), "a");
    }

    #[test]
    fn choices_insert_the_first_option_and_keep_them_all() {
        let parsed = parse_snippet("align: ${2|left,right|};");
        assert_eq!(parsed.text, "align: left;");
        assert_eq!(
            parsed.tabstops[0],
            Tabstop {
                index: 2,
                spans: vec![(7, 11)],
                choices: vec!["left".to_owned(), "right".to_owned()],
            }
        );
        // Escaped separators inside a choice are literal.
        assert_eq!(text("${1|a\\,b,c|}"), "a,b");
        assert_eq!(text("${1|pipe\\|char,x|}"), "pipe|char");
        // A mirror of a choice shows the chosen option.
        assert_eq!(text("${1|on,off|}/$1"), "on/on");
    }

    #[test]
    fn variables_use_defaults_or_vanish() {
        assert_eq!(text("$TM_FILENAME"), "");
        assert_eq!(text("${TM_FILENAME}"), "");
        assert_eq!(text("${TM_FILENAME:untitled}"), "untitled");
        assert_eq!(text("hello $USER!"), "hello !");
        assert_eq!(text("$_private rest"), " rest");
        // A default may hold a stop of its own.
        let parsed = parse_snippet("${NAME:${1:anon}}");
        assert_eq!(parsed.text, "anon");
        assert_eq!(parsed.tabstops[0], stop(1, &[(0, 4)]));
    }

    #[test]
    fn transforms_vanish() {
        assert_eq!(text("${TM_FILENAME/(.*)\\..+$/$1/}"), "");
        assert_eq!(text("a${VAR/x/y/g}b"), "ab");
    }

    #[test]
    fn escapes_become_literals() {
        assert_eq!(text("\\$notatabstop"), "$notatabstop");
        assert_eq!(text("\\\\$1"), "\\");
        assert_eq!(text("${1:brace \\} inside}"), "brace } inside");
        // A backslash before anything else is an ordinary backslash.
        assert_eq!(text("C:\\path"), "C:\\path");
    }

    #[test]
    fn lone_dollars_are_literal() {
        assert_eq!(text("cost: $ 5"), "cost: $ 5");
        assert_eq!(text("end$"), "end$");
        assert_eq!(text("$-x"), "$-x");
    }

    #[test]
    fn malformed_braces_are_left_verbatim() {
        assert_eq!(text("${?}"), "${?}");
        assert_eq!(text("${}"), "${}");
        // Unclosed placeholder: lenient, the content still expands.
        assert_eq!(text("${1:oops"), "oops");
        // `${1x}` is neither a tabstop nor a valid placeholder.
        assert_eq!(
            parse_snippet("${1x}"),
            Snippet {
                text: "${1x}".to_owned(),
                tabstops: Vec::new(),
            }
        );
    }

    /// Spans count characters, not bytes or UTF-16 units.
    #[test]
    fn spans_are_character_offsets() {
        let parsed = parse_snippet("😀${1:é}");
        assert_eq!(parsed.tabstops[0], stop(1, &[(1, 2)]));
    }

    /// A deterministic pseudo-random stream (an LCG), so a failure replays.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, bound: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(self.0 >> 33).unwrap_or_default() % bound
        }

        /// A string of up to `max` characters drawn from `alphabet`.
        fn string(&mut self, alphabet: &[char], max: usize) -> String {
            let len = self.below(max + 1);
            (0..len)
                .map(|_| alphabet[self.below(alphabet.len())])
                .collect()
        }
    }

    /// Falsified by a panic, a span outside the inserted text or reversed,
    /// unsorted or duplicated spans, or stops out of visiting order.
    #[test]
    fn generated_snippets_keep_every_span_inside_the_text() {
        const SYNTAX: &[char] = &[
            '$', '$', '{', '}', '0', '1', '2', ':', '|', ',', '\\', '/', 'a', '_', 'é', '😀', ' ',
        ];
        let mut rng = Rng(0x5eed);
        for _ in 0..4000 {
            let input = rng.string(SYNTAX, 24);
            let parsed = parse_snippet(&input);
            let len = u32::try_from(parsed.text.chars().count()).unwrap_or(u32::MAX);
            for stop in &parsed.tabstops {
                for &(start, end) in &stop.spans {
                    assert!(
                        start <= end && end <= len,
                        "{input:?}: span {start}..{end} of {len}"
                    );
                }
                assert!(
                    stop.spans.windows(2).all(|pair| pair[0] < pair[1]),
                    "{input:?}: spans not sorted and distinct"
                );
            }
            let order: Vec<u32> = parsed.tabstops.iter().map(|stop| stop.index).collect();
            if let Some((last, numbered)) = order.split_last() {
                assert_eq!(*last, 0, "{input:?}: the final stop is not last");
                assert!(
                    numbered.windows(2).all(|pair| pair[0] < pair[1]) && !numbered.contains(&0),
                    "{input:?}: stops visited as {order:?}"
                );
            }
        }
    }

    /// Falsified by snippet-free text being altered, or a construct the
    /// grammar rejects losing or rewriting any of what follows it.
    #[test]
    fn generated_malformed_snippets_are_left_verbatim() {
        // No `$` and no `\`: nothing here can start a construct or an escape.
        const PLAIN: &[char] = &['{', '}', '0', '1', ':', '|', ',', '/', 'a', '_', 'é', ' '];
        let mut rng = Rng(0xbad5);
        for _ in 0..2000 {
            let rest = rng.string(PLAIN, 16);
            assert_eq!(
                parse_snippet(&rest),
                Snippet {
                    text: rest.clone(),
                    tabstops: Vec::new()
                }
            );
            for opener in ["${?", "${}", "${-", "${1x", "${ 1"] {
                let input = format!("{opener}{rest}");
                let parsed = parse_snippet(&input);
                assert_eq!(parsed.text, input, "rewritten");
                assert!(parsed.tabstops.is_empty(), "{input:?}: grew a stop");
            }
        }
    }

    #[test]
    fn realistic_server_snippets() {
        // rust-analyzer method call
        assert_eq!(text("push(${1:ch})$0"), "push(ch)");
        // typescript-language-server import
        let parsed = parse_snippet("import { $1 } from \"${2:module}\";$0");
        assert_eq!(parsed.text, "import {  } from \"module\";");
        assert_eq!(parsed.tabstops[0], stop(1, &[(9, 9)]));
        // An absurdly long stop number saturates instead of overflowing.
        assert_eq!(parse_snippet("$99999999999").tabstops[0].index, u32::MAX);
    }
}
