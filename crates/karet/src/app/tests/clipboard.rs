//! Clipboard rows of `docs/editing-behavior.md`: copy and cut take every
//! caret's selection, or every caret's whole line when nothing is selected, and
//! paste puts a line copy above the line and gives each caret its own piece when
//! the counts match.

use super::harness::*;
use crate::app::*;

/// A Ctrl+`c` press.
fn ctrl(c: char) -> Step {
    Step::Key(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// Run `steps` from `start` and return the rig's marked text and clipboard.
fn run(start: &str, steps: &[Step]) -> (String, String) {
    let mut rig = Rig::new(start);
    rig.run(steps);
    let clipboard = rig.app.clipboard.get().unwrap_or_default();
    (rig.marked(), clipboard)
}

/// Copy takes every selection, or every caret's line; the text never changes.
#[test]
fn copy_takes_every_caret() {
    let rows = [
        ("one selection", "a[bc|]d", "a[bc|]d", "bc"),
        (
            "several selections, joined in document order",
            "[ab#] x\n[cd|]",
            "[ab#] x\n[cd|]",
            "ab\ncd",
        ),
        (
            "empty carets beside a selection add nothing",
            "a|b\nc[de#]",
            "a|b\nc[de#]",
            "de",
        ),
        ("no selection copies the line", "a|b\ncd", "a|b\ncd", "ab\n"),
        (
            "no selection copies every caret's line once",
            "a|b#\ncd\ne|f",
            "a|b#\ncd\ne|f",
            "ab\nef\n",
        ),
    ];
    for (name, start, expect, clipboard) in rows {
        let (got, copied) = run(start, &[ctrl('c')]);
        assert_eq!(got, expect, "{name}");
        assert_eq!(copied, clipboard, "{name}");
    }
}

/// Cut deletes exactly what its copy took, and never an unselected character.
#[test]
fn cut_deletes_what_it_copied() {
    let rows = [
        (
            "an empty caret beside a selection keeps its text (#332)",
            "foo |bar\nfoo [bar#]",
            "foo |bar\nfoo #",
            "bar",
        ),
        ("every selection is cut", "[ab|] x [cd#]", "| x #", "ab\ncd"),
        (
            "no selection cuts the line, the caret keeps its column",
            "ab\nc|d\nef",
            "ab\ne|f",
            "cd\n",
        ),
        (
            "the last line takes the break before it",
            "ab\nc|d",
            "a|b",
            "cd\n",
        ),
        ("the only line empties the buffer", "a|b", "|", "ab\n"),
        (
            "carets on adjacent lines cut both lines",
            "a|\nb#\nc",
            "c|",
            "a\nb\n",
        ),
        (
            "carets on separate lines each cut their own",
            "a|\nb\nc#\nd",
            "b|\nd#",
            "a\nc\n",
        ),
    ];
    for (name, start, expect, clipboard) in rows {
        let (got, copied) = run(start, &[ctrl('x')]);
        assert_eq!(got, expect, "{name}");
        assert_eq!(copied, clipboard, "{name}");
    }
}

/// Paste inserts at every caret, a line copy above the line, and one piece per
/// caret when the counts match.
#[test]
fn paste_places_what_was_copied() {
    use KeyCode::*;
    check(vec![
        case(
            "a line copy pastes above the caret's line",
            "a|b\ncd",
            vec![ctrl('c'), key(Down), ctrl('v')],
            "ab\nab\nc|d",
        ),
        case(
            "a line cut pastes back above the line it moved up",
            "ab\nc|d\nef",
            vec![ctrl('x'), ctrl('v')],
            "ab\ncd\ne|f",
        ),
        case(
            "a line copy replaces a selection like any text",
            "a|b\ncd",
            vec![
                ctrl('c'),
                key(Down),
                Step::Key(Home, KeyModifiers::SHIFT),
                ctrl('v'),
            ],
            "ab\nab\n|d",
        ),
        case(
            "each caret pastes the piece it copied",
            "[x|]\n[y#]",
            vec![ctrl('c'), key(End), ctrl('v')],
            "xx|\nyy#",
        ),
        case(
            "a multi-line piece still goes to the caret that copied it",
            "[a\nb|]\n[c#]",
            vec![ctrl('c'), key(End), ctrl('v')],
            "a\nba\nb|\ncc#",
        ),
        case(
            "each caret pastes the line it copied above its own line",
            "a|\nb#",
            vec![ctrl('c'), ctrl('v')],
            "a\na|\nb\nb#",
        ),
    ]);
}

/// Text from outside karet: one line per caret is distributed, anything else
/// goes in whole at every caret.
#[test]
fn paste_of_outside_text() -> Result<(), crate::clipboard::ClipboardError> {
    let rows = [
        ("one line per caret", "a|\nb#", "1\n2", "a1|\nb2#"),
        (
            "a trailing newline still counts lines",
            "a|\nb#",
            "1\n2\n",
            "a1|\nb2#",
        ),
        ("counts that differ paste whole", "a|\nb#", "z", "az|\nbz#"),
        ("one caret pastes whole", "a|", "1\n2", "a1\n2|"),
    ];
    for (name, start, clipboard, expect) in rows {
        let mut rig = Rig::new(start);
        rig.app.clipboard.set(clipboard)?;
        rig.run(&[ctrl('v')]);
        assert_eq!(rig.marked(), expect, "{name}");
    }
    Ok(())
}

/// The line-copy marker belongs to the text karet wrote: the same text from
/// elsewhere pastes at the caret.
#[test]
fn outside_text_equal_to_a_line_has_no_marker() -> Result<(), crate::clipboard::ClipboardError> {
    let mut rig = Rig::new("a|b");
    rig.run(&[ctrl('c')]);
    rig.app.clipboard.set("ab\n")?;
    rig.run(&[ctrl('v')]);
    assert_eq!(rig.marked(), "aab\n|b");
    Ok(())
}

/// Carets sharing a line keep every line piece dealt to them: each goes in
/// above that line, in document order. A line copy every caret takes whole
/// goes in once.
#[test]
fn line_pieces_for_carets_on_one_line() -> Result<(), crate::clipboard::ClipboardError> {
    let rows: [(&str, &[&str], &str); 2] = [
        ("one piece per caret", &["x\n", "y\n"], "x\ny\na|b#"),
        ("one piece for every caret", &["x\n"], "x\na|b#"),
    ];
    for (name, pieces, expect) in rows {
        let mut rig = Rig::new("a|b#");
        rig.app.clipboard.set_copied(crate::clipboard::Copied {
            pieces: pieces.iter().map(|p| (*p).to_string()).collect(),
            whole_lines: true,
            indents: Vec::new(),
        })?;
        rig.run(&[ctrl('v')]);
        assert_eq!(rig.marked(), expect, "{name}");
    }
    Ok(())
}

/// A paste of several lines that starts a line of its own lands at the level
/// Rust's indent query gives that line, keeping the block's inner structure;
/// one line, or a paste after other text on the line, goes in as it is.
#[test]
fn multi_line_paste_reindents_from_the_indent_query() -> Result<(), crate::clipboard::ClipboardError>
{
    let block = "if a {\n    b();\n}";
    let deep = "        if a {\n            b();\n        }";
    let rows = [
        (
            "a block from column 0, one level into the function",
            "fn f() {\n|\n}",
            block,
            "fn f() {\n    if a {\n        b();\n    }|\n}",
        ),
        (
            "a deeper block comes out to the caret's level",
            "fn f() {\n|\n}",
            deep,
            "fn f() {\n    if a {\n        b();\n    }|\n}",
        ),
        (
            "the indentation before the caret is replaced by the level",
            "fn f() {\n  |\n}",
            deep,
            "fn f() {\n    if a {\n        b();\n    }|\n}",
        ),
        (
            "the file's own step decides the level",
            "fn g() {\n\tx();\n}\nfn f() {\n|\n}",
            block,
            "fn g() {\n\tx();\n}\nfn f() {\n\tif a {\n\t    b();\n\t}|\n}",
        ),
        (
            "a line shallower than the first keeps its shortfall",
            "fn f() {\n|\n}",
            "    a();\n}\nb();",
            "fn f() {\n    a();\n}\nb();|\n}",
        ),
        (
            "blank lines inside the block stay as they are",
            "fn f() {\n|\n}",
            "a();\n\nb();",
            "fn f() {\n    a();\n\n    b();|\n}",
        ),
        (
            "a blank first line leaves the first solid line to set the base",
            "fn f() {\n|\n}",
            "\n    if a {\n        b();\n    }",
            "fn f() {\n\n    if a {\n        b();\n    }|\n}",
        ),
        (
            "a selection on its own line is replaced at the level",
            "fn f() {\n    [x|]\n}",
            deep,
            "fn f() {\n    if a {\n        b();\n    }|\n}",
        ),
        (
            "one line goes in as it is",
            "fn f() {\n|\n}",
            "        x();",
            "fn f() {\n        x();|\n}",
        ),
        (
            "after other text on the line, as it is",
            "fn f() { |}",
            block,
            "fn f() { if a {\n    b();\n}|}",
        ),
        (
            "carets on lines of their own each re-indent",
            "fn f() {\n|\n#\n}",
            "a\n  b\nc",
            "fn f() {\n    a\n      b\n    c|\n    a\n      b\n    c#\n}",
        ),
        (
            "carets sharing a line's indentation each paste as they are",
            "fn f() {\n  |  #\n}",
            "a\n b\nc",
            "fn f() {\n  a\n b\nc|  a\n b\nc#\n}",
        ),
        (
            "on the first line, with no line above to indent from, as it is",
            "|",
            deep,
            "        if a {\n            b();\n        }|",
        ),
        (
            "outside any construct, as it is",
            "fn f() {}\n|",
            deep,
            "fn f() {}\n        if a {\n            b();\n        }|",
        ),
    ];
    for (name, start, clipboard, expect) in rows {
        let mut rig = Rig::new(start);
        rig.app.clipboard.set(clipboard)?;
        rig.run(&[ctrl('v')]);
        assert_eq!(rig.marked(), expect, "{name}");
    }
    Ok(())
}

/// karet's own copy remembers the level its first line had, so a selection
/// begun after the indentation re-indents as the block it was, and a line copy
/// of several lines goes in above the caret's line at that line's level.
#[test]
fn a_karet_copy_reindents_from_its_first_lines_level()
-> Result<(), crate::clipboard::ClipboardError> {
    let rows: [(&str, crate::clipboard::Copied, &str); 3] = [
        (
            "a selection taken after the indentation",
            crate::clipboard::Copied {
                pieces: vec!["if a {\n            b();\n        }".into()],
                whole_lines: false,
                indents: vec!["        ".into()],
            },
            "fn f() {\n    if a {\n        b();\n    }|\n}",
        ),
        (
            "a line copy of several lines, above the caret's line",
            crate::clipboard::Copied {
                pieces: vec!["        a();\n".into(), "            b();\n".into()],
                whole_lines: true,
                indents: vec!["        ".into(), "            ".into()],
            },
            "fn f() {\n    a();\n        b();\n|\n}",
        ),
        (
            "a selection begun on an unindented blank line",
            crate::clipboard::Copied {
                pieces: vec!["\n    fn x() {\n        y();\n    }".into()],
                whole_lines: false,
                indents: vec![String::new()],
            },
            "fn f() {\n\n    fn x() {\n        y();\n    }|\n}",
        ),
    ];
    for (name, copied, expect) in rows {
        let mut rig = Rig::new("fn f() {\n|\n}");
        rig.app.clipboard.set_copied(copied)?;
        rig.run(&[ctrl('v')]);
        assert_eq!(rig.marked(), expect, "{name}");
    }
    Ok(())
}

/// Copying records the level each piece's first line had where it was taken.
#[test]
fn copy_records_each_pieces_first_line_level() {
    let rows: [(&str, &str, &[&str]); 2] = [
        (
            "selections, from the line each starts on",
            "    i[f a|] {\n\t[b#]",
            &["    ", "\t"],
        ),
        ("line copies, from each line", "  a|\nb#", &["  ", ""]),
    ];
    for (name, start, indents) in rows {
        let mut rig = Rig::new(start);
        rig.run(&[ctrl('c')]);
        let text = rig.app.clipboard.get().unwrap_or_default();
        let recorded = rig
            .app
            .clipboard
            .copied(&text)
            .map(|c| c.indents.clone())
            .unwrap_or_default();
        assert_eq!(recorded, indents, "{name}");
    }
}

/// A language with no indent query pastes every block as it is: the text a
/// Rust file re-indents goes in verbatim once the tab is TOML.
#[test]
fn a_language_without_an_indent_query_pastes_verbatim()
-> Result<(), crate::clipboard::ClipboardError> {
    let mut rig = Rig::new("fn f() {\n|\n}");
    let idx = rig.app.active;
    if let TabKind::Code { path, .. } = &mut rig.app.tabs[idx].kind {
        *path = "t.toml".into();
    }
    rig.app.clipboard.set("        b = 1\n          c = 2")?;
    rig.run(&[ctrl('v')]);
    assert_eq!(rig.marked(), "fn f() {\n        b = 1\n          c = 2|\n}");
    Ok(())
}

/// A line copy that comes back with CRLF endings (as a Windows or terminal
/// clipboard hands it over) is still recognised as karet's own copy, so it
/// pastes above the caret's line rather than at the caret.
#[test]
fn a_crlf_line_copy_still_pastes_as_a_line() {
    let mut rig = Rig::new("a|b\ncd");
    rig.step(Step::Key(KeyCode::Char('c'), KeyModifiers::CONTROL));
    rig.step(key(KeyCode::Down));
    rig.app.handle_paste("ab\r\n".to_owned());
    assert_eq!(rig.marked(), "ab\nab\nc|d");
}

/// Every CRLF and lone CR becomes LF before the text lands; text that
/// normalises to nothing changes nothing.
#[test]
fn pasted_line_endings_become_lf() {
    let rows = [
        ("a lone CR", "a\rb", "a\nb|"),
        ("a CRLF", "a\r\nb", "a\nb|"),
        ("mixed endings", "a\r\nb\rc\nd", "a\nb\nc\nd|"),
        ("CR CR stays two breaks", "a\r\rb", "a\n\nb|"),
        ("nothing", "", "|"),
    ];
    for (name, pasted, expect) in rows {
        let mut rig = Rig::new("|");
        rig.app.handle_paste(pasted.to_owned());
        assert_eq!(rig.marked(), expect, "{name}");
    }
}
