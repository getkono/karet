use super::*;

/// Lex `chunks` in order through one tokenizer, then flush it.
fn lex(chunks: &[&str]) -> Vec<Token> {
    let mut tokenizer = Tokenizer::default();
    let mut out = Vec::new();
    for chunk in chunks {
        tokenizer.feed(chunk, &mut out);
    }
    tokenizer.flush(&mut out);
    out
}

fn open(name: &str, attrs: &[(&str, &str)]) -> Token {
    Token::Open {
        name: name.to_owned(),
        attrs: attrs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        self_closing: false,
    }
}

fn text(text: &str) -> Token {
    Token::Text(text.to_owned())
}

fn close(name: &str) -> Token {
    Token::Close(name.to_owned())
}

#[test]
fn tags_text_and_close_tags_lex_in_order() {
    assert_eq!(
        lex(&["<p>Hi <b>there</b></p>"]),
        vec![
            open("p", &[]),
            text("Hi "),
            open("b", &[]),
            text("there"),
            close("b"),
            close("p"),
        ]
    );
}

#[test]
fn every_attribute_form_is_read_and_names_are_lowercased() {
    assert_eq!(
        lex(&[r#"<IMG SRC="a b.png" alt='say "hi"' width=200 hidden data-x = "y">"#]),
        vec![open(
            "img",
            &[
                ("src", "a b.png"),
                ("alt", "say \"hi\""),
                ("width", "200"),
                ("hidden", ""),
                ("data-x", "y"),
            ]
        )]
    );
}

#[test]
fn a_self_closing_tag_says_so() {
    assert_eq!(
        lex(&["<br/><img src=x />"]),
        vec![
            Token::Open {
                name: "br".to_owned(),
                attrs: Vec::new(),
                self_closing: true,
            },
            Token::Open {
                name: "img".to_owned(),
                attrs: vec![("src".to_owned(), "x".to_owned())],
                self_closing: true,
            },
        ]
    );
}

#[test]
fn character_references_decode_in_text_and_attribute_values() {
    assert_eq!(
        lex(&[r#"<a title="&quot;q&quot;">&lt;tag&gt; &amp; &#39;&#x41;&nbsp;&copy;</a>"#]),
        vec![
            open("a", &[("title", "\"q\"")]),
            text("<tag> & 'A\u{a0}©"),
            close("a"),
        ]
    );
}

#[test]
fn an_unknown_or_malformed_reference_is_kept_as_written() {
    assert_eq!(
        decode_entities("&bogus; & &#xZZ; &#0; &#xD800; &#99999999; &amp"),
        "&bogus; & &#xZZ; &#0; &#xD800; &#99999999; &amp"
    );
}

#[test]
fn a_less_than_that_opens_no_tag_is_text() {
    assert_eq!(lex(&["a < b <3 </ 5"]), vec![text("a < b <3 </ 5")]);
}

#[test]
fn comments_doctypes_cdata_and_instructions_are_dropped() {
    assert_eq!(
        lex(&["a<!-- <b>hidden</b> -->b<!DOCTYPE html>c<![CDATA[x]]>d<?php x ?>e"]),
        vec![text("abcde")]
    );
}

#[test]
fn a_comment_split_across_chunks_is_still_dropped() {
    assert_eq!(
        lex(&["a<!-- one\n", "two\n", "three -->b\n"]),
        vec![text("ab\n")]
    );
}

#[test]
fn a_tag_split_across_chunks_is_reassembled() {
    assert_eq!(
        lex(&["<img\n", "  src=\"logo.png\"\n", "  width=\"200\">\n"]),
        vec![
            open("img", &[("src", "logo.png"), ("width", "200")]),
            text("\n"),
        ]
    );
}

#[test]
fn an_unterminated_tag_is_released_as_text_and_an_unterminated_comment_dropped() {
    assert_eq!(lex(&["x <b class=\"y"]), vec![text("x <b class=\"y")]);
    assert_eq!(lex(&["x <!-- never closed"]), vec![text("x ")]);
}

#[test]
fn held_back_markup_is_bounded() {
    // Past the cap, the `<` gives up waiting and becomes text; lexing carries on.
    let long = format!("<a title=\"{}", "x".repeat(PENDING_CAP + 1));
    let tokens = lex(&[&long, "\">b"]);
    let joined: String = tokens
        .iter()
        .map(|token| match token {
            Token::Text(text) => text.as_str(),
            _ => "",
        })
        .collect();
    assert!(joined.starts_with("<a title="), "{:?}", tokens.first());
    assert!(joined.ends_with("\">b"));
}

#[test]
fn raw_element_content_is_skipped_unread_across_chunks() {
    assert_eq!(
        lex(&[
            "<script>if (a<b) { x = \"</div>\"; }\n",
            "more();</SCRIPT >after"
        ]),
        vec![open("script", &[]), close("script"), text("after")]
    );
    // A longer name is not the close tag.
    assert_eq!(
        lex(&["<style></styles>x</style>y"]),
        vec![open("style", &[]), close("style"), text("y")]
    );
}

#[test]
fn an_unclosed_raw_element_is_closed_at_flush() {
    assert_eq!(
        lex(&["<script>forever"]),
        vec![open("script", &[]), close("script")]
    );
    // But flushing only the held-back markup leaves it open, for a lone inline tag.
    let mut tokenizer = Tokenizer::default();
    let mut out = Vec::new();
    tokenizer.feed("<script>", &mut out);
    tokenizer.flush_pending(&mut out);
    assert_eq!(out, vec![open("script", &[])]);
}

#[test]
fn attr_finds_a_value_by_name() {
    let attrs = vec![("src".to_owned(), "x".to_owned())];
    assert_eq!(attr(&attrs, "src"), Some("x"));
    assert_eq!(attr(&attrs, "alt"), None);
}

#[test]
fn lexing_never_panics_on_hostile_input() {
    for input in [
        "<",
        "<<",
        "</",
        "<!",
        "<!-",
        "<a",
        "<a ",
        "<a b",
        "<a b=",
        "<a b='",
        "<a/",
        "<a/b>",
        "&",
        "&#",
        "&#x",
        "<a =x>",
        "<a ==>",
        "<é>",
        "é<a é=é>é",
        "<a\u{0}>",
    ] {
        let _ = lex(&[input]);
        // Every split point must be survivable too.
        for (at, _) in input.char_indices() {
            let _ = lex(&[&input[..at], &input[at..]]);
        }
    }
}
