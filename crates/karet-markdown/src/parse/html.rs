//! Embedded HTML → the render model: the curated subset of elements it can show.
//!
//! Tags map onto the same frame stack markdown builds with, so an element may open in
//! one HTML block and close in a later one with markdown in between (`<div
//! align="center">`, a blank line, `# Title`, a blank line, `</div>`). The subset:
//!
//! - text formatting: `b`/`strong`, `i`/`em`, `s`/`del`/`strike`, `code`/`kbd`/`tt`/`samp`;
//! - `a href`, `img`, `br`, `hr`, `h1`–`h6`, `ul`/`ol`/`li`, `blockquote`;
//! - containers (`p`, `div`, `center`, `details`, `section`, …), honouring
//!   `align="center"`/`"right"`; `summary` renders as a bold line;
//! - `script`, `style`, `iframe` and `object` are dropped with their content.
//!
//! Every other tag drops its markup and keeps its text; comments are hidden. There is
//! no layout beyond alignment, and no CSS.

#[cfg(test)]
mod tests;

use super::Builder;
use super::Frame;
use crate::Alignment;
use crate::Block;
use crate::ImageRef;
use crate::Inline;
use crate::html::RAW_ELEMENTS;
use crate::html::Token;
use crate::html::Tokenizer;
use crate::html::attr;

/// Container elements: transparent, but able to align what they hold.
const CONTAINERS: [&str; 17] = [
    "p",
    "div",
    "center",
    "details",
    "section",
    "article",
    "header",
    "footer",
    "main",
    "nav",
    "aside",
    "figure",
    "figcaption",
    "picture",
    "dl",
    "dt",
    "dd",
];

/// The glyph leading a `<summary>`: the details it heads are always shown expanded.
const SUMMARY_MARKER: &str = "▾ ";

impl Builder {
    /// Map a chunk of embedded HTML: a line of an HTML block, or one whole inline tag.
    pub(super) fn html(&mut self, chunk: &str, inline: bool) {
        let mut tokens = Vec::new();
        if inline {
            // An inline tag arrives whole. Lexing it apart from the block lexer keeps a
            // stray `<` in one paragraph from bleeding into the next.
            let mut lexer = Tokenizer::default();
            lexer.feed(chunk, &mut tokens);
            lexer.flush_pending(&mut tokens);
        } else {
            self.lexer.feed(chunk, &mut tokens);
        }
        for token in tokens {
            self.html_token(token, inline);
        }
    }

    /// An HTML block ended: release what the lexer held back, and close the inline
    /// content it left open. Containers stay open — a `<div>` may span markdown blocks —
    /// except a `<p>`: markdown after it starts a paragraph of its own, which in a
    /// browser ends the open one, so an unclosed `<p align="center">` cannot centre the
    /// rest of the document.
    pub(super) fn end_html_block(&mut self) {
        let mut tokens = Vec::new();
        self.lexer.flush(&mut tokens);
        for token in tokens {
            self.html_token(token, false);
        }
        self.close_inline_run();
        if self.innermost_html_tag() == Some("p") {
            self.html_close("p");
        }
    }

    fn html_token(&mut self, token: Token, inline: bool) {
        match token {
            Token::Text(text) => self.html_text(&text),
            Token::Open {
                name,
                attrs,
                self_closing,
            } => self.html_open(&name, &attrs, self_closing, inline),
            Token::Close(name) => self.html_close(&name),
        }
    }

    /// HTML text: whitespace collapses as a browser collapses it, and whitespace alone
    /// between block elements is formatting, not content.
    fn html_text(&mut self, text: &str) {
        if self.suppress.is_some() {
            return;
        }
        let mut collapsed = collapse_whitespace(text);
        // A run of whitespace split across chunks (a line end, the next line's indent)
        // is still one run.
        if collapsed.starts_with(' ') && self.ends_in_space() {
            collapsed.remove(0);
        }
        if collapsed == " " && !self.inline_open() || collapsed.is_empty() {
            return;
        }
        self.text(&collapsed);
    }

    /// Whether the innermost inline container's content ends in a space.
    fn ends_in_space(&self) -> bool {
        let content = match self.stack.last() {
            Some(
                Frame::Paragraph { content, .. }
                | Frame::Heading { content, .. }
                | Frame::Emphasis(content)
                | Frame::Strong(content)
                | Frame::Strikethrough(content)
                | Frame::TableCell(content),
            ) => content,
            Some(
                Frame::Link { text, .. } | Frame::HtmlCode(text) | Frame::Image { alt: text, .. },
            ) => {
                return text.ends_with(' ');
            },
            _ => return false,
        };
        matches!(content.last(), Some(Inline::Text(text)) if text.ends_with(' '))
    }

    fn html_open(
        &mut self,
        name: &str,
        attrs: &[(String, String)],
        self_closing: bool,
        inline: bool,
    ) {
        if self.suppress.is_some() {
            return;
        }
        if RAW_ELEMENTS.contains(&name) {
            if !self_closing {
                self.suppress = Some((name.to_owned(), self.stack.len()));
            }
            return;
        }
        let empty = Vec::new;
        match name {
            "img" => self.html_image(attrs),
            "br" if self.inline_open() => self.text("\n"),
            "a" => {
                if let Some(href) = attr(attrs, "href").filter(|href| !href.is_empty()) {
                    let link = Frame::Link {
                        href: href.to_owned(),
                        text: String::new(),
                        image: None,
                    };
                    self.push_html(name, link);
                }
            },
            "b" | "strong" => self.push_html(name, Frame::Strong(empty())),
            "i" | "em" | "cite" | "var" => self.push_html(name, Frame::Emphasis(empty())),
            "s" | "del" | "strike" => self.push_html(name, Frame::Strikethrough(empty())),
            "code" | "kbd" | "tt" | "samp" => self.push_html(name, Frame::HtmlCode(String::new())),
            // HTML tables render as text: a cell boundary is a word boundary.
            "td" | "th" if self.inline_open() => self.text(" "),
            // Block elements count only in an HTML block; inline, only their text shows.
            _ if inline => {},
            "hr" => self.block(Block::Rule),
            _ if self_closing => {},
            "blockquote" => {
                self.close_inline_run();
                self.push_html(name, Frame::Quote(Vec::new()));
            },
            "summary" => {
                self.close_inline_run();
                let heading = Frame::Paragraph {
                    content: Vec::new(),
                    implicit: true,
                };
                self.push_html(name, heading);
                self.stack
                    .push(Frame::Strong(vec![Inline::Text(SUMMARY_MARKER.to_owned())]));
            },
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.close_inline_run();
                let level = name[1..].parse().unwrap_or(1);
                let heading = Frame::Heading {
                    level,
                    content: Vec::new(),
                };
                match html_align(name, attrs) {
                    Some(align) => {
                        self.push_marker(name, Some(align));
                        self.stack.push(heading);
                    },
                    None => self.push_html(name, heading),
                }
            },
            "ul" | "ol" => {
                self.close_inline_run();
                let start = (name == "ol").then(|| {
                    attr(attrs, "start")
                        .and_then(|start| start.trim().parse().ok())
                        .unwrap_or(1)
                });
                let list = Frame::List {
                    start,
                    items: Vec::new(),
                };
                self.push_html(name, list);
            },
            "li" => {
                self.close_inline_run();
                // `<li>` implies the end of an open sibling `<li>`.
                if self.innermost_html_tag() == Some("li") {
                    self.html_close("li");
                }
                let item = Frame::Item {
                    task: None,
                    blocks: Vec::new(),
                };
                self.push_html(name, item);
            },
            _ if CONTAINERS.contains(&name) || name == "tr" => {
                self.close_inline_run();
                // `<p>` implies the end of an open `<p>`.
                if name == "p" && self.innermost_html_tag() == Some("p") {
                    self.html_close("p");
                }
                self.push_marker(name, html_align(name, attrs));
            },
            _ => {},
        }
    }

    /// Close the frame the innermost open `<name>` opened, and everything inside it. A
    /// close tag with no open element is ignored.
    fn html_close(&mut self, name: &str) {
        if let Some((raw, _)) = &self.suppress {
            if raw == name {
                self.suppress = None;
            }
            return;
        }
        let Some(index) = self
            .html_by_name
            .get(name)
            .and_then(|open| open.last().copied())
        else {
            return;
        };
        // Exactly the frames open now: closing an inline element can open the
        // implicit paragraph that holds it, and that paragraph must outlive the close.
        for _ in index..self.stack.len() {
            self.close();
        }
    }

    fn html_image(&mut self, attrs: &[(String, String)]) {
        let alt = attr(attrs, "alt").unwrap_or_default();
        let Some(src) = attr(attrs, "src").filter(|src| !src.trim().is_empty()) else {
            if !alt.is_empty() {
                self.text(alt);
            }
            return;
        };
        self.inline(Inline::Image(ImageRef {
            alt: alt.to_owned(),
            src: src.trim().to_owned(),
            title: attr(attrs, "title")
                .filter(|title| !title.is_empty())
                .map(str::to_owned),
            width: attr(attrs, "width").and_then(css_pixels),
            height: attr(attrs, "height").and_then(css_pixels),
            link: None,
        }));
    }

    /// Push `frame`, remembering that tag `name` opened it.
    fn push_html(&mut self, name: &str, frame: Frame) {
        self.html_tags.push((self.stack.len(), name.to_owned()));
        self.html_by_name
            .entry(name.to_owned())
            .or_default()
            .push(self.stack.len());
        self.stack.push(frame);
    }

    /// Push a transparent container marker for `name`.
    fn push_marker(&mut self, name: &str, align: Option<Alignment>) {
        let (target, inherited) = self.container();
        self.markers += 1;
        let marker = Frame::HtmlBlock {
            align: align.or(inherited),
            target,
        };
        self.push_html(name, marker);
    }

    /// The name of the innermost element HTML opened that is still open.
    fn innermost_html_tag(&self) -> Option<&str> {
        self.html_tags.last().map(|(_, tag)| tag.as_str())
    }

    /// Whether the innermost frame collects inline content.
    fn inline_open(&self) -> bool {
        matches!(
            self.stack.last(),
            Some(
                Frame::Paragraph { .. }
                    | Frame::Heading { .. }
                    | Frame::Emphasis(_)
                    | Frame::Strong(_)
                    | Frame::Strikethrough(_)
                    | Frame::Link { .. }
                    | Frame::Image { .. }
                    | Frame::HtmlCode(_)
                    | Frame::TableCell(_)
            )
        )
    }

    /// Close the inline content HTML left open — and the implicit paragraph holding it —
    /// so the next block starts as a sibling rather than inside a sentence.
    fn close_inline_run(&mut self) {
        while matches!(
            self.stack.last(),
            Some(
                Frame::Paragraph { implicit: true, .. }
                    | Frame::Heading { .. }
                    | Frame::Emphasis(_)
                    | Frame::Strong(_)
                    | Frame::Strikethrough(_)
                    | Frame::Link { .. }
                    | Frame::Image { .. }
                    | Frame::HtmlCode(_)
            )
        ) {
            self.close();
        }
    }
}

/// The alignment `name` with `attrs` declares: `<center>`, or an `align` attribute of
/// `center` or `right`. Left (and justify) is the default, so declares nothing.
fn html_align(name: &str, attrs: &[(String, String)]) -> Option<Alignment> {
    if name == "center" {
        return Some(Alignment::Center);
    }
    match attr(attrs, "align")?.trim().to_ascii_lowercase().as_str() {
        "center" | "middle" => Some(Alignment::Center),
        "right" => Some(Alignment::Right),
        _ => None,
    }
}

/// A `width`/`height` attribute as CSS pixels: its leading digits (`200`, `200px`). A
/// percentage is relative to a layout the preview does not have, so it is ignored.
fn css_pixels(value: &str) -> Option<u32> {
    let value = value.trim();
    let digits = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    if value[digits..].trim_start().starts_with('%') {
        return None;
    }
    value[..digits].parse().ok().filter(|&px| px > 0)
}

/// `text` with every run of ASCII whitespace collapsed to one space. A non-breaking
/// space is content, not formatting, and survives.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if c.is_ascii_whitespace() {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}
