//! Integration tests for the karet-docx public API, exercising the full
//! bytes → [`Document`] → markdown pipeline across the crate boundary.
//!
//! Every DOCX fixture is built in-test by zipping hand-written XML with the same
//! `zip` crate the reader uses — no binary fixtures are checked in.

use std::io::Write;

use karet_docx::Block;
use karet_docx::DocxError;
use karet_docx::ParaStyle;
use karet_docx::parse;
use karet_docx::to_markdown;

/// Wrap `body_inner` (a sequence of `w:p`/`w:tbl`) in the document skeleton.
fn document_xml(body_inner: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<w:body>{body_inner}</w:body></w:document>"#
    )
}

/// Build a `.docx` (ZIP) whose only entry is `word/document.xml`.
fn docx(body_inner: &str) -> Vec<u8> {
    docx_parts(&[("word/document.xml", document_xml(body_inner).into_bytes())])
}

/// Build a `.docx` from an explicit set of `(entry name, bytes)` parts.
fn docx_parts(parts: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        // In-memory ZIP writes do not realistically fail; ignore the results rather
        // than `unwrap`/`expect` (denied by the workspace lints, tests included).
        for (name, content) in parts {
            let _ = writer.start_file(*name, options);
            let _ = writer.write_all(content);
        }
        let _ = writer.finish();
    }
    buf
}

/// Parse fixture bytes and render to markdown in one step. A parse failure yields
/// an empty string, so the caller's `assert_eq!` reports the mismatch.
fn markdown_of(bytes: &[u8]) -> String {
    parse(bytes).map(|d| to_markdown(&d)).unwrap_or_default()
}

/// A `w:r` run wrapping a single `w:t` with the given text.
fn run(text: &str) -> String {
    format!("<w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r>")
}

#[test]
fn plain_paragraphs() {
    let body = format!(
        "<w:p>{}</w:p><w:p>{}</w:p>",
        run("Hello world"),
        run("Second")
    );
    assert_eq!(markdown_of(&docx(&body)), "Hello world\n\nSecond");
}

#[test]
fn headings_from_pstyle() {
    let body = format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>{}</w:p>
<w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr>{}</w:p>
<w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr>{}</w:p>"#,
        run("Big"),
        run("Sub"),
        run("The Title"),
    );
    assert_eq!(markdown_of(&docx(&body)), "# Big\n\n## Sub\n\n# The Title");
}

#[test]
fn bold_italic_strike_underline_runs() {
    let body = r#"<w:p>
<w:r><w:rPr><w:b/></w:rPr><w:t>bold</w:t></w:r>
<w:r><w:rPr><w:i/></w:rPr><w:t>italic</w:t></w:r>
<w:r><w:rPr><w:strike/></w:rPr><w:t>gone</w:t></w:r>
<w:r><w:rPr><w:u w:val="single"/></w:rPr><w:t>under</w:t></w:r>
</w:p>"#;
    // Underline has no markdown form, so `under` is plain text.
    assert_eq!(markdown_of(&docx(body)), "**bold***italic*~~gone~~under");
}

#[test]
fn explicitly_disabled_bold_is_not_applied() {
    let body = r#"<w:p><w:r><w:rPr><w:b w:val="false"/></w:rPr><w:t>plain</w:t></w:r></w:p>"#;
    assert_eq!(markdown_of(&docx(body)), "plain");
}

#[test]
fn nested_ordered_and_bulleted_lists() {
    // numbering.xml: numId 1 -> abstract 0 (decimal, ordered); numId 2 -> abstract 1
    // (bullet). Level 1 of abstract 0 stays decimal.
    let numbering = r#"<?xml version="1.0"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:abstractNum w:abstractNumId="0">
  <w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl>
  <w:lvl w:ilvl="1"><w:numFmt w:val="decimal"/></w:lvl>
</w:abstractNum>
<w:abstractNum w:abstractNumId="1">
  <w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl>
</w:abstractNum>
<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
<w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>
</w:numbering>"#;
    let list_item = |num: &str, ilvl: &str, text: &str| {
        format!(
            r#"<w:p><w:pPr><w:pStyle w:val="ListParagraph"/><w:numPr><w:ilvl w:val="{ilvl}"/><w:numId w:val="{num}"/></w:numPr></w:pPr>{}</w:p>"#,
            run(text)
        )
    };
    let body = format!(
        "{}{}{}",
        list_item("1", "0", "first"),
        list_item("1", "1", "nested"),
        list_item("2", "0", "bullet"),
    );
    let bytes = docx_parts(&[
        ("word/document.xml", document_xml(&body).into_bytes()),
        ("word/numbering.xml", numbering.as_bytes().to_vec()),
    ]);
    assert_eq!(markdown_of(&bytes), "1. first\n\n  1. nested\n\n- bullet");
}

#[test]
fn list_without_numbering_defaults_to_bullet() {
    // No numbering.xml at all: an unresolved numId falls back to a bullet.
    let body = format!(
        r#"<w:p><w:pPr><w:numPr><w:numId w:val="5"/></w:numPr></w:pPr>{}</w:p>"#,
        run("item")
    );
    assert_eq!(markdown_of(&docx(&body)), "- item");
}

#[test]
fn table_becomes_pipe_table_with_escaping() {
    let cell = |text: &str| format!("<w:tc><w:p>{}</w:p></w:tc>", run(text));
    let body = format!(
        "<w:tbl><w:tr>{}{}</w:tr><w:tr>{}{}</w:tr></w:tbl>",
        cell("Name"),
        cell("Value"),
        cell("a|b"),
        cell("c"),
    );
    assert_eq!(
        markdown_of(&docx(&body)),
        "| Name | Value |\n| --- | --- |\n| a\\|b | c |"
    );
}

#[test]
fn hyperlink_resolves_to_markdown_link() {
    let rels = r#"<?xml version="1.0"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/" TargetMode="External"/>
</Relationships>"#;
    let body = format!(
        r#"<w:p><w:hyperlink r:id="rId1">{}</w:hyperlink></w:p>"#,
        run("click here")
    );
    let bytes = docx_parts(&[
        ("word/document.xml", document_xml(&body).into_bytes()),
        ("word/_rels/document.xml.rels", rels.as_bytes().to_vec()),
    ]);
    assert_eq!(markdown_of(&bytes), "[click here](https://example.com/)");
}

#[test]
fn unresolved_hyperlink_degrades_to_plain_text() {
    // No rels part, so `r:id` cannot resolve; the text is kept without a link.
    let body = format!(
        r#"<w:p><w:hyperlink r:id="rId9">{}</w:hyperlink></w:p>"#,
        run("bare")
    );
    assert_eq!(markdown_of(&docx(&body)), "bare");
}

#[test]
fn image_becomes_placeholder() {
    let body = format!(
        "<w:p>{}<w:r><w:drawing><wp:inline><a:blip/></wp:inline></w:drawing></w:r></w:p>",
        run("before ")
    );
    assert_eq!(markdown_of(&docx(&body)), "before [image]");
}

#[test]
fn breaks_and_tabs_become_newline_and_tab() {
    let body =
        "<w:p><w:r><w:t>a</w:t><w:br/><w:t>b</w:t><w:tab/><w:t>c</w:t></w:r></w:p>".to_string();
    assert_eq!(markdown_of(&docx(&body)), "a\nb\tc");
}

#[test]
fn unknown_elements_are_skipped() {
    // Proofing marks, bookmarks, and an unknown leaf must not disturb the text.
    let body = format!(
        r#"<w:p><w:bookmarkStart w:id="0" w:name="x"/><w:proofErr w:type="spellStart"/>{}<w:proofErr w:type="spellEnd"/><w:mysteryTag w:val="?"/><w:bookmarkEnd w:id="0"/></w:p>"#,
        run("clean text")
    );
    assert_eq!(markdown_of(&docx(&body)), "clean text");
}

#[test]
fn error_not_a_zip() {
    assert!(matches!(
        parse(b"this is definitely not a zip archive"),
        Err(DocxError::NotAZip)
    ));
}

#[test]
fn error_missing_document() {
    // A valid ZIP that lacks word/document.xml.
    let bytes = docx_parts(&[("word/other.xml", b"<x/>".to_vec())]);
    assert!(matches!(parse(&bytes), Err(DocxError::MissingDocument)));
}

#[test]
fn error_not_utf8() {
    // word/document.xml carrying an invalid UTF-8 byte (0xFF).
    let bytes = docx_parts(&[("word/document.xml", vec![b'<', 0xFF, b'>'])]);
    assert!(matches!(parse(&bytes), Err(DocxError::NotUtf8)));
}

#[test]
fn error_malformed_xml() {
    // An attribute value with no closing quote is a hard XML syntax error.
    let bytes = docx_parts(&[(
        "word/document.xml",
        br#"<w:document><w:body><w:p w:x="oops></w:p></w:body></w:document>"#.to_vec(),
    )]);
    assert!(matches!(parse(&bytes), Err(DocxError::Xml(_))));
}

#[test]
fn parses_into_expected_model_structure() {
    let body = format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>{}</w:p><w:p>{}</w:p>"#,
        run("Title"),
        run("body"),
    );
    let doc = parse(&docx(&body)).unwrap_or_default();
    assert_eq!(doc.blocks.len(), 2);
    assert!(matches!(
        doc.blocks[0],
        Block::Paragraph {
            style: ParaStyle::Heading(1),
            ..
        }
    ));
    assert!(matches!(
        doc.blocks[1],
        Block::Paragraph {
            style: ParaStyle::Normal,
            ..
        }
    ));
}
