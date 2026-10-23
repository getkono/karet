//! Parse a DOCX byte slice into the neutral [`Document`] model.
//!
//! A `.docx` is a ZIP container of XML parts. This module unzips it (deflate only),
//! then streams `word/document.xml` with `quick-xml` — no DOM is built — reacting to
//! the handful of WordprocessingML elements karet needs and **skipping everything
//! else** so an unrecognized tag never fails a parse. Two side-car parts are read
//! best-effort: `word/_rels/document.xml.rels` (to resolve hyperlink `r:id`s to
//! URLs) and `word/numbering.xml` (to tell ordered lists from bulleted ones).
//!
//! ## Deliberate simplifications
//! - **Elements are matched by *local* name** (`p`, `r`, `t`, …), so the `w:`/`r:`
//!   namespace prefixes a producer chooses do not matter.
//! - **Numbering** is resolved only as far as `numId → abstractNumId → per-level
//!   `w:numFmt``; a level whose format is `bullet`/`none` (or that cannot be
//!   resolved at all) renders as a bullet, everything else as an ordered item.
//! - **Hyperlinks** resolve through the relationships part when the target is
//!   present; an unresolvable link degrades to its plain text.
//! - **Images** (`w:drawing`/`w:pict`/`w:object`) are flattened to a `[image]`
//!   placeholder span; their contents are skipped.

use std::collections::HashMap;
use std::io::Cursor;
use std::io::Read;

use quick_xml::Reader;
use quick_xml::events::BytesStart;
use quick_xml::events::Event;
use zip::ZipArchive;
use zip::result::ZipError;

use crate::error::DocxError;
use crate::model::Block;
use crate::model::Document;
use crate::model::ParaStyle;
use crate::model::Span;

/// Parse the bytes of a `.docx` file into a [`Document`].
///
/// # Errors
/// Returns [`DocxError`] if `bytes` is not a ZIP ([`DocxError::NotAZip`]), has no
/// `word/document.xml` ([`DocxError::MissingDocument`]), that part is not UTF-8
/// ([`DocxError::NotUtf8`]), or its XML is malformed ([`DocxError::Xml`]).
pub fn parse(bytes: &[u8]) -> Result<Document, DocxError> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).map_err(|_| DocxError::NotAZip)?;

    let document = read_entry(&mut zip, "word/document.xml")?.ok_or(DocxError::MissingDocument)?;
    let document = String::from_utf8(document).map_err(|_| DocxError::NotUtf8)?;

    // Side-car parts are optional; a failure to read or decode one degrades to "no
    // relationships" / "no numbering" rather than failing the whole document.
    let rels = read_entry(&mut zip, "word/_rels/document.xml.rels")
        .ok()
        .flatten()
        .and_then(|b| String::from_utf8(b).ok())
        .map(|s| parse_relationships(&s))
        .unwrap_or_default();
    let numbering = read_entry(&mut zip, "word/numbering.xml")
        .ok()
        .flatten()
        .and_then(|b| String::from_utf8(b).ok())
        .map(|s| Numbering::parse(&s))
        .unwrap_or_default();

    let blocks = parse_body(&document, &rels, &numbering)?;
    Ok(Document { blocks })
}

/// A map from a relationship id (`rId7`) to its target (a URL, for hyperlinks).
type Rels = HashMap<String, String>;

/// Read a ZIP entry by name: `Ok(Some(bytes))` if present, `Ok(None)` if absent,
/// `Err` if the archive is unreadable.
fn read_entry(
    zip: &mut ZipArchive<Cursor<&[u8]>>,
    name: &str,
) -> Result<Option<Vec<u8>>, DocxError> {
    match zip.by_name(name) {
        Ok(mut file) => {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).map_err(|_| DocxError::NotAZip)?;
            Ok(Some(buf))
        },
        Err(ZipError::FileNotFound) => Ok(None),
        Err(_) => Err(DocxError::NotAZip),
    }
}

/// Map any XML/decoding error to [`DocxError::Xml`].
fn xml_err<E: std::fmt::Display>(e: E) -> DocxError {
    DocxError::Xml(e.to_string())
}

/// Character formatting accumulated from a run's `w:rPr`.
#[derive(Clone, Copy, Default)]
struct RunFmt {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

impl RunFmt {
    /// Build a [`Span`] carrying `text`, this run's formatting, and `link`.
    fn span(self, text: String, link: &Option<String>) -> Span {
        Span {
            text,
            bold: self.bold,
            italic: self.italic,
            underline: self.underline,
            strike: self.strike,
            link: link.clone(),
        }
    }
}

/// An unformatted span (a line break, tab, or image placeholder), carrying `link`.
fn plain_span(text: &str, link: &Option<String>) -> Span {
    Span {
        text: text.to_string(),
        link: link.clone(),
        ..Span::default()
    }
}

/// Parse `word/document.xml`'s body into block-level elements.
fn parse_body(xml: &str, rels: &Rels, numbering: &Numbering) -> Result<Vec<Block>, DocxError> {
    let mut reader = Reader::from_reader(xml.as_bytes());
    let mut blocks = Vec::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"p" => blocks.push(parse_paragraph(&mut reader, rels, numbering)?),
                b"tbl" => blocks.push(parse_table(&mut reader, rels, numbering)?),
                // Any other container (`w:body`, `w:sectPr`, …) is descended into
                // transparently — its `w:p`/`w:tbl` children are handled here too.
                _ => {},
            },
            // A self-closed empty paragraph is a blank line.
            Event::Empty(e) if e.local_name().as_ref() == b"p" => {
                blocks.push(Block::Paragraph {
                    style: ParaStyle::Normal,
                    spans: Vec::new(),
                });
            },
            Event::Eof => break,
            _ => {},
        }
    }
    Ok(blocks)
}

/// Parse a `w:p` (the opening tag already consumed) into a paragraph block.
fn parse_paragraph(
    reader: &mut Reader<&[u8]>,
    rels: &Rels,
    numbering: &Numbering,
) -> Result<Block, DocxError> {
    let mut style = ParaStyle::Normal;
    let mut spans: Vec<Span> = Vec::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"pPr" => style = parse_paragraph_props(reader, numbering)?,
                b"r" => parse_run(reader, None, &mut spans)?,
                b"hyperlink" => {
                    let url = relationship_target(&e, rels);
                    parse_hyperlink(reader, url, &mut spans)?;
                },
                b"drawing" | b"pict" | b"object" => {
                    skip_subtree(reader)?;
                    spans.push(plain_span("[image]", &None));
                },
                _ => {},
            },
            Event::End(e) if e.local_name().as_ref() == b"p" => break,
            Event::Eof => break,
            _ => {},
        }
    }
    Ok(Block::Paragraph { style, spans })
}

/// Parse a `w:pPr` (opening tag consumed) into the resolved [`ParaStyle`].
fn parse_paragraph_props(
    reader: &mut Reader<&[u8]>,
    numbering: &Numbering,
) -> Result<ParaStyle, DocxError> {
    let mut p_style: Option<String> = None;
    let mut ilvl: Option<u8> = None;
    let mut num_id: Option<String> = None;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                b"pStyle" => p_style = attr(&e, b"val"),
                b"ilvl" => ilvl = attr(&e, b"val").and_then(|v| v.parse().ok()),
                b"numId" => num_id = attr(&e, b"val"),
                _ => {},
            },
            Event::End(e) if e.local_name().as_ref() == b"pPr" => break,
            Event::Eof => break,
            _ => {},
        }
    }
    Ok(resolve_style(
        p_style.as_deref(),
        ilvl,
        num_id.as_deref(),
        numbering,
    ))
}

/// Decide a paragraph's style from its `w:pStyle` and numbering properties.
///
/// A heading style wins over a numbering reference (a numbered heading renders as a
/// heading); otherwise a `w:numId` makes it a list item, defaulting to a bullet.
fn resolve_style(
    p_style: Option<&str>,
    ilvl: Option<u8>,
    num_id: Option<&str>,
    numbering: &Numbering,
) -> ParaStyle {
    if let Some(style) = p_style
        && let Some(level) = heading_level(style)
    {
        return ParaStyle::Heading(level);
    }
    if let Some(num_id) = num_id {
        let level = ilvl.unwrap_or(0);
        return ParaStyle::ListItem {
            ordered: numbering.ordered(num_id, level),
            level,
        };
    }
    ParaStyle::Normal
}

/// The heading level a `w:pStyle` value denotes: `Title` → 1, `Heading{n}` /
/// `Heading {n}` → `n` (clamped to `1..=6`), or `None` for a non-heading style.
fn heading_level(style: &str) -> Option<u8> {
    let style = style.trim();
    if style.eq_ignore_ascii_case("Title") {
        return Some(1);
    }
    let rest = style.to_ascii_lowercase();
    let rest = rest.strip_prefix("heading")?;
    let n: u8 = rest.trim().parse().ok()?;
    (n >= 1).then_some(n.min(6))
}

/// Parse a `w:r` run (opening tag consumed), appending its spans to `spans`. `link`
/// is `Some` when the run sits inside a `w:hyperlink`.
fn parse_run(
    reader: &mut Reader<&[u8]>,
    link: Option<String>,
    spans: &mut Vec<Span>,
) -> Result<(), DocxError> {
    let mut fmt = RunFmt::default();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"rPr" => fmt = parse_run_props(reader)?,
                b"t" => {
                    let text = read_text(reader, b"t")?;
                    if !text.is_empty() {
                        spans.push(fmt.span(text, &link));
                    }
                },
                b"br" => {
                    skip_subtree(reader)?;
                    spans.push(plain_span("\n", &link));
                },
                b"tab" => {
                    skip_subtree(reader)?;
                    spans.push(plain_span("\t", &link));
                },
                b"drawing" | b"pict" | b"object" => {
                    skip_subtree(reader)?;
                    spans.push(plain_span("[image]", &link));
                },
                _ => {},
            },
            Event::Empty(e) => match e.local_name().as_ref() {
                b"br" | b"cr" => spans.push(plain_span("\n", &link)),
                b"tab" => spans.push(plain_span("\t", &link)),
                b"drawing" | b"pict" | b"object" => spans.push(plain_span("[image]", &link)),
                _ => {},
            },
            Event::End(e) if e.local_name().as_ref() == b"r" => break,
            Event::Eof => break,
            _ => {},
        }
    }
    Ok(())
}

/// Parse a `w:rPr` (opening tag consumed) into accumulated [`RunFmt`].
fn parse_run_props(reader: &mut Reader<&[u8]>) -> Result<RunFmt, DocxError> {
    let mut fmt = RunFmt::default();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                b"b" => fmt.bold = toggle(&e),
                b"i" => fmt.italic = toggle(&e),
                b"strike" => fmt.strike = toggle(&e),
                b"u" => fmt.underline = underline_on(&e),
                _ => {},
            },
            Event::End(e) if e.local_name().as_ref() == b"rPr" => break,
            Event::Eof => break,
            _ => {},
        }
    }
    Ok(fmt)
}

/// Parse a `w:hyperlink` (opening tag consumed), appending its runs' spans — each
/// carrying `url` as its link — to `spans`.
fn parse_hyperlink(
    reader: &mut Reader<&[u8]>,
    url: Option<String>,
    spans: &mut Vec<Span>,
) -> Result<(), DocxError> {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"r" => parse_run(reader, url.clone(), spans)?,
                b"drawing" | b"pict" | b"object" => {
                    skip_subtree(reader)?;
                    spans.push(plain_span("[image]", &url));
                },
                _ => {},
            },
            Event::End(e) if e.local_name().as_ref() == b"hyperlink" => break,
            Event::Eof => break,
            _ => {},
        }
    }
    Ok(())
}

/// Parse a `w:tbl` (opening tag consumed) into a table block.
fn parse_table(
    reader: &mut Reader<&[u8]>,
    rels: &Rels,
    numbering: &Numbering,
) -> Result<Block, DocxError> {
    let mut rows: Vec<Vec<Vec<Span>>> = Vec::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) if e.local_name().as_ref() == b"tr" => {
                rows.push(parse_row(reader, rels, numbering)?);
            },
            Event::End(e) if e.local_name().as_ref() == b"tbl" => break,
            Event::Eof => break,
            _ => {},
        }
    }
    Ok(Block::Table { rows })
}

/// Parse a `w:tr` row (opening tag consumed) into its cells.
fn parse_row(
    reader: &mut Reader<&[u8]>,
    rels: &Rels,
    numbering: &Numbering,
) -> Result<Vec<Vec<Span>>, DocxError> {
    let mut cells: Vec<Vec<Span>> = Vec::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) if e.local_name().as_ref() == b"tc" => {
                cells.push(parse_cell(reader, rels, numbering)?);
            },
            Event::End(e) if e.local_name().as_ref() == b"tr" => break,
            Event::Eof => break,
            _ => {},
        }
    }
    Ok(cells)
}

/// Parse a `w:tc` cell (opening tag consumed) into a flat run of spans, joining its
/// paragraphs with a single space and skipping any nested table.
fn parse_cell(
    reader: &mut Reader<&[u8]>,
    rels: &Rels,
    numbering: &Numbering,
) -> Result<Vec<Span>, DocxError> {
    let mut spans: Vec<Span> = Vec::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(e) => match e.local_name().as_ref() {
                b"p" => {
                    if let Block::Paragraph { spans: para, .. } =
                        parse_paragraph(reader, rels, numbering)?
                    {
                        if !spans.is_empty() && !para.is_empty() {
                            spans.push(Span::text(" "));
                        }
                        spans.extend(para);
                    }
                },
                b"tbl" => skip_subtree(reader)?,
                _ => {},
            },
            Event::End(e) if e.local_name().as_ref() == b"tc" => break,
            Event::Eof => break,
            _ => {},
        }
    }
    Ok(spans)
}

/// Read text content up to the `end` local-name closing tag, returning the
/// concatenated, entity-unescaped string.
fn read_text(reader: &mut Reader<&[u8]>, end: &[u8]) -> Result<String, DocxError> {
    let mut out = String::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Text(t) => out.push_str(&t.xml_content().map_err(xml_err)?),
            Event::End(e) if e.local_name().as_ref() == end => break,
            Event::Eof => break,
            _ => {},
        }
    }
    Ok(out)
}

/// Consume events until the element opened just before the call is closed,
/// balancing nested start/end tags. Tolerates a truncated (EOF-terminated) subtree.
fn skip_subtree(reader: &mut Reader<&[u8]>) -> Result<(), DocxError> {
    let mut depth = 1u32;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf).map_err(xml_err)? {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            },
            Event::Eof => break,
            _ => {},
        }
    }
    Ok(())
}

/// The value of the attribute with local name `name`, entity-unescaped.
fn attr(e: &BytesStart, name: &[u8]) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        (a.key.local_name().as_ref() == name)
            .then(|| a.unescape_value().ok().map(|v| v.into_owned()))
            .flatten()
    })
}

/// A `w:b`/`w:i`/`w:strike` toggle: on unless `w:val` explicitly says off.
fn toggle(e: &BytesStart) -> bool {
    !matches!(attr(e, b"val").as_deref(), Some("false" | "0" | "off"))
}

/// A `w:u` underline: on unless its `w:val` is `none` (or an explicit off).
fn underline_on(e: &BytesStart) -> bool {
    !matches!(attr(e, b"val").as_deref(), Some("none" | "false" | "0"))
}

/// Resolve a `w:hyperlink`'s `r:id` attribute against the relationships map.
fn relationship_target(e: &BytesStart, rels: &Rels) -> Option<String> {
    attr(e, b"id").and_then(|id| rels.get(&id).cloned())
}

/// Parse `word/_rels/document.xml.rels` into an id → target map.
fn parse_relationships(xml: &str) -> Rels {
    let mut reader = Reader::from_reader(xml.as_bytes());
    let mut map = Rels::new();
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if e.local_name().as_ref() == b"Relationship"
                    && let (Some(id), Some(target)) = (attr(&e, b"Id"), attr(&e, b"Target"))
                {
                    map.insert(id, target);
                }
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {},
        }
    }
    map
}

/// Best-effort ordered-vs-bulleted resolution from `word/numbering.xml`.
///
/// Maps each `w:num`'s `numId` to its `abstractNumId`, and each abstract
/// definition's per-`ilvl` `w:numFmt` to whether it is ordered (any format other
/// than `bullet`/`none`). Anything that cannot be resolved defaults to bulleted.
#[derive(Default)]
struct Numbering {
    num_to_abstract: HashMap<String, String>,
    abstract_levels: HashMap<String, HashMap<u8, bool>>,
}

impl Numbering {
    /// Parse the numbering part; a malformed part yields an empty (all-bullets) map.
    fn parse(xml: &str) -> Self {
        let mut reader = Reader::from_reader(xml.as_bytes());
        let mut this = Numbering::default();
        let mut current_abstract: Option<String> = None;
        let mut current_level: Option<u8> = None;
        let mut current_num: Option<String> = None;
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(e)) | Ok(Event::Empty(e)) => match e.local_name().as_ref() {
                    b"abstractNum" => current_abstract = attr(&e, b"abstractNumId"),
                    b"lvl" => current_level = attr(&e, b"ilvl").and_then(|v| v.parse().ok()),
                    b"numFmt" => {
                        if let (Some(abs), Some(level), Some(fmt)) =
                            (&current_abstract, current_level, attr(&e, b"val"))
                        {
                            let ordered = fmt != "bullet" && fmt != "none";
                            this.abstract_levels
                                .entry(abs.clone())
                                .or_default()
                                .insert(level, ordered);
                        }
                    },
                    b"num" => current_num = attr(&e, b"numId"),
                    // The `<w:abstractNumId>` *element* only appears inside `<w:num>`.
                    b"abstractNumId" => {
                        if let (Some(num), Some(abs)) = (&current_num, attr(&e, b"val")) {
                            this.num_to_abstract.insert(num.clone(), abs);
                        }
                    },
                    _ => {},
                },
                Ok(Event::End(e)) => match e.local_name().as_ref() {
                    b"abstractNum" => {
                        current_abstract = None;
                        current_level = None;
                    },
                    b"lvl" => current_level = None,
                    b"num" => current_num = None,
                    _ => {},
                },
                Ok(Event::Eof) | Err(_) => break,
                _ => {},
            }
        }
        this
    }

    /// Whether the list identified by `num_id` at nesting `level` is ordered. Falls
    /// back to level 0's format, then to bulleted (`false`).
    fn ordered(&self, num_id: &str, level: u8) -> bool {
        self.num_to_abstract
            .get(num_id)
            .and_then(|abs| self.abstract_levels.get(abs))
            .and_then(|levels| levels.get(&level).or_else(|| levels.get(&0)))
            .copied()
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_level_maps_styles() {
        assert_eq!(heading_level("Heading1"), Some(1));
        assert_eq!(heading_level("Heading 3"), Some(3));
        assert_eq!(heading_level("Heading9"), Some(6)); // clamped to 6
        assert_eq!(heading_level("Title"), Some(1));
        assert_eq!(heading_level("title"), Some(1));
        assert_eq!(heading_level("Normal"), None);
        assert_eq!(heading_level("Heading0"), None);
    }

    #[test]
    fn toggle_defaults_on_and_respects_off() {
        assert!(toggle(&BytesStart::new("w:b")));
        // `<w:b w:val="false"/>` turns bold off.
        let mut e = BytesStart::new("w:b");
        e.push_attribute(("w:val", "false"));
        assert!(!toggle(&e));
    }

    #[test]
    fn underline_none_is_off() {
        let mut e = BytesStart::new("w:u");
        e.push_attribute(("w:val", "none"));
        assert!(!underline_on(&e));
        let mut single = BytesStart::new("w:u");
        single.push_attribute(("w:val", "single"));
        assert!(underline_on(&single));
    }

    #[test]
    fn numbering_defaults_to_bullets_when_unresolved() {
        let n = Numbering::default();
        assert!(!n.ordered("1", 0));
    }

    /// Parse `inner` as the contents of a `w:body`, with no rels and no numbering.
    fn body(inner: &str) -> Result<Vec<Block>, DocxError> {
        body_with(inner, &Numbering::default())
    }

    /// Parse `inner` as the contents of a `w:body` against `numbering`.
    fn body_with(inner: &str, numbering: &Numbering) -> Result<Vec<Block>, DocxError> {
        let xml = format!("<w:document><w:body>{inner}</w:body></w:document>");
        parse_body(&xml, &Rels::new(), numbering)
    }

    /// A normal paragraph holding `spans`.
    fn normal(spans: Vec<Span>) -> Block {
        Block::Paragraph {
            style: ParaStyle::Normal,
            spans,
        }
    }

    /// A numbering part whose `numId` 1 → abstract 0 has level 0 `decimal` and
    /// level 1 `bullet`; `numId` 2 → abstract 9 is undefined.
    const NUMBERING: &str = r#"<w:numbering>
        <w:abstractNum w:abstractNumId="0">
            <w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl>
            <w:lvl w:ilvl="1"><w:numFmt w:val="bullet"/></w:lvl>
        </w:abstractNum>
        <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
        <w:num w:numId="2"><w:abstractNumId w:val="9"/></w:num>
    </w:numbering>"#;

    #[test]
    fn numbering_resolves_levels_and_falls_back_to_level_zero() {
        let n = Numbering::parse(NUMBERING);
        assert!(n.ordered("1", 0));
        assert!(!n.ordered("1", 1));
        // Level 5 is not defined: level 0's `decimal` decides it.
        assert!(n.ordered("1", 5));
        // An abstract definition that does not exist, or an unknown numId, is a bullet.
        assert!(!n.ordered("2", 0));
        assert!(!n.ordered("3", 0));
    }

    #[test]
    fn numbering_parse_tolerates_malformed_xml() {
        // Everything before the parse error is kept; the error just stops the scan.
        let n = Numbering::parse(
            r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></oops>"#,
        );
        assert!(n.ordered("1", 0));
        assert!(!Numbering::parse("<<<not xml").ordered("1", 0));
    }

    #[test]
    fn heading_style_wins_over_num_id() -> Result<(), DocxError> {
        let numbering = Numbering::parse(NUMBERING);
        let blocks = body_with(
            r#"<w:p><w:pPr><w:pStyle w:val="Heading2"/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>H</w:t></w:r></w:p>"#,
            &numbering,
        )?;
        assert_eq!(
            blocks,
            vec![Block::Paragraph {
                style: ParaStyle::Heading(2),
                spans: vec![Span::text("H")],
            }]
        );
        Ok(())
    }

    #[test]
    fn non_heading_style_with_num_id_is_a_list_item() -> Result<(), DocxError> {
        let numbering = Numbering::parse(NUMBERING);
        let blocks = body_with(
            r#"<w:p><w:pPr><w:pStyle w:val="ListParagraph"/><w:numPr><w:ilvl w:val="3"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>a</w:t></w:r></w:p><w:p><w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr></w:p>"#,
            &numbering,
        )?;
        assert_eq!(
            blocks,
            vec![
                // Level 3 is undefined, so level 0's `decimal` makes it ordered.
                Block::Paragraph {
                    style: ParaStyle::ListItem {
                        ordered: true,
                        level: 3,
                    },
                    spans: vec![Span::text("a")],
                },
                // No `w:ilvl` defaults the level to 0.
                Block::Paragraph {
                    style: ParaStyle::ListItem {
                        ordered: true,
                        level: 0,
                    },
                    spans: Vec::new(),
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn self_closed_paragraph_is_an_empty_normal_paragraph() -> Result<(), DocxError> {
        let blocks = body("<w:p/><w:p><w:r><w:t>x</w:t></w:r></w:p><w:p/>")?;
        assert_eq!(
            blocks,
            vec![
                normal(Vec::new()),
                normal(vec![Span::text("x")]),
                normal(Vec::new())
            ]
        );
        Ok(())
    }

    #[test]
    fn breaks_tabs_and_carriage_returns_become_plain_spans() -> Result<(), DocxError> {
        let blocks = body(
            "<w:p><w:r><w:rPr><w:b/></w:rPr><w:t>a</w:t><w:cr/><w:t>b</w:t><w:br/><w:tab/><w:br></w:br></w:r></w:p>",
        )?;
        let bold = |t: &str| Span {
            text: t.into(),
            bold: true,
            ..Span::default()
        };
        // The break/tab spans carry no formatting even inside a bold run.
        assert_eq!(
            blocks,
            vec![normal(vec![
                bold("a"),
                Span::text("\n"),
                bold("b"),
                Span::text("\n"),
                Span::text("\t"),
                Span::text("\n"),
            ])]
        );
        Ok(())
    }

    #[test]
    fn run_toggles_respect_explicit_off_values() -> Result<(), DocxError> {
        let blocks = body(
            r#"<w:p>
                <w:r><w:rPr><w:b w:val="0"/><w:i w:val="off"/><w:strike w:val="false"/><w:u w:val="0"/></w:rPr><w:t>plain</w:t></w:r>
                <w:r><w:rPr><w:b w:val="1"/><w:i w:val="on"/><w:strike w:val="true"/><w:u w:val="single"/></w:rPr><w:t>all</w:t></w:r>
                <w:r><w:rPr><w:b/><w:i/><w:strike/><w:u/></w:rPr><w:t>bare</w:t></w:r>
                <w:r><w:rPr><w:u w:val="false"/></w:rPr><w:t>u-false</w:t></w:r>
            </w:p>"#,
        )?;
        let all = |t: &str| Span {
            text: t.into(),
            bold: true,
            italic: true,
            underline: true,
            strike: true,
            link: None,
        };
        assert_eq!(
            blocks,
            vec![normal(vec![
                Span::text("plain"),
                all("all"),
                all("bare"),
                Span::text("u-false"),
            ])]
        );
        Ok(())
    }

    #[test]
    fn underline_off_spellings() {
        for (val, on) in [
            ("none", false),
            ("false", false),
            ("0", false),
            // `off` is a toggle spelling, not an underline value: it stays on.
            ("off", true),
            ("double", true),
        ] {
            let mut e = BytesStart::new("w:u");
            e.push_attribute(("w:val", val));
            assert_eq!(underline_on(&e), on, "w:u w:val={val}");
        }
    }

    /// One table row of single-text cells.
    fn row(cells: &[&str]) -> Vec<Vec<Span>> {
        cells.iter().map(|c| vec![Span::text(*c)]).collect()
    }

    #[test]
    fn cell_paragraphs_join_with_a_single_space() -> Result<(), DocxError> {
        let blocks = body(
            "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>one</w:t></w:r></w:p><w:p></w:p><w:p/><w:p><w:r><w:t>two</w:t></w:r></w:p></w:tc></w:tr></w:tbl>",
        )?;
        // The empty paragraphs between add no separator of their own.
        assert_eq!(
            blocks,
            vec![Block::Table {
                rows: vec![vec![vec![
                    Span::text("one"),
                    Span::text(" "),
                    Span::text("two"),
                ]]],
            }]
        );
        Ok(())
    }

    #[test]
    fn nested_table_inside_a_cell_is_skipped() -> Result<(), DocxError> {
        let blocks = body(
            "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>outer</w:t></w:r></w:p><w:tbl><w:tr><w:tc><w:p><w:r><w:t>inner</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:tc><w:tc><w:p><w:r><w:t>next</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p><w:r><w:t>after</w:t></w:r></w:p>",
        )?;
        assert_eq!(
            blocks,
            vec![
                Block::Table {
                    rows: vec![row(&["outer", "next"])],
                },
                normal(vec![Span::text("after")]),
            ]
        );
        Ok(())
    }

    #[test]
    fn ragged_and_empty_tables_keep_their_shape() -> Result<(), DocxError> {
        let blocks = body(
            "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc></w:tr><w:tr><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc></w:tr><w:tr></w:tr></w:tbl><w:tbl></w:tbl><w:tbl><w:tr><w:tc></w:tc></w:tr></w:tbl>",
        )?;
        assert_eq!(
            blocks,
            vec![
                Block::Table {
                    rows: vec![row(&["a"]), row(&["b", "c"]), Vec::new()],
                },
                Block::Table { rows: Vec::new() },
                Block::Table {
                    rows: vec![vec![Vec::new()]],
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn truncated_body_degrades_without_error() -> Result<(), DocxError> {
        // EOF inside a paragraph/run/text keeps what was read so far.
        assert_eq!(
            parse_body(
                "<w:document><w:body><w:p><w:r><w:t>cut off",
                &Rels::new(),
                &Numbering::default()
            )?,
            vec![normal(vec![Span::text("cut off")])]
        );
        // EOF inside a table row, a skipped drawing, and run properties.
        assert_eq!(
            parse_body(
                "<w:body><w:tbl><w:tr><w:tc><w:p><w:r><w:t>x</w:t></w:r>",
                &Rels::new(),
                &Numbering::default()
            )?,
            vec![Block::Table {
                rows: vec![row(&["x"])],
            }]
        );
        assert_eq!(
            parse_body(
                "<w:body><w:p><w:r><w:drawing><w:inline>",
                &Rels::new(),
                &Numbering::default()
            )?,
            vec![normal(vec![Span::text("[image]")])]
        );
        assert_eq!(
            parse_body(
                "<w:body><w:p><w:r><w:rPr><w:b/>",
                &Rels::new(),
                &Numbering::default()
            )?,
            vec![normal(Vec::new())]
        );
        assert_eq!(
            parse_body("", &Rels::new(), &Numbering::default())?,
            Vec::new()
        );
        Ok(())
    }

    #[test]
    fn malformed_body_xml_is_an_xml_error() {
        for xml in [
            // A mismatched close tag.
            "<w:body><w:p><w:r><w:t>x</w:r></w:p></w:body>",
            // An unterminated attribute value inside a paragraph.
            r#"<w:body><w:p><w:pPr><w:pStyle w:val="Heading1></w:pPr></w:p></w:body>"#,
            // A close tag with no matching open.
            "<w:body></w:p></w:body>",
        ] {
            let result = parse_body(xml, &Rels::new(), &Numbering::default());
            assert!(
                matches!(result, Err(DocxError::Xml(_))),
                "{xml}: {result:?}"
            );
        }
    }
}
