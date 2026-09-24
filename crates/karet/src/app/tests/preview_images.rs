//! The markdown preview painting local images end to end: layout reserves the rows,
//! the pixels arrive off-thread, and an image clicks through to its own tab.

use super::support::*;
use crate::app::*;
use crate::preview_images::tests::png;

/// An app rooted at a scratch workspace holding `readme` and `files`, with the
/// README open and its preview beside it, drawn once.
fn previewing(readme: &str, files: &[(&str, &[u8])]) -> (App, std::path::PathBuf) {
    let root = test_dir("preview-images");
    write_file(&root, "README.md", readme.as_bytes());
    for (rel, bytes) in files {
        write_file(&root, rel, bytes);
    }
    let mut app = App::new(root.clone(), Vec::new(), Vec::new(), false);
    app.open_path(&root.join("README.md"));
    app.main_rect = Rect::new(0, 0, 100, 30);
    app.dispatch(Command::MarkdownPreviewSide);
    let _ = screen(&mut app, 100, 30);
    (app, root)
}

/// The wrapped lines of the active tab's in-editor preview.
fn preview_lines(app: &App) -> Vec<karet_markdown::WrappedLine> {
    app.tabs[app.active]
        .markdown_preview
        .as_ref()
        .map(|preview| preview.wrapped.lines.clone())
        .unwrap_or_default()
}

fn image_rows(app: &App) -> usize {
    preview_lines(app)
        .iter()
        .filter(|line| line.image.is_some())
        .count()
}

/// `text` without its OSC 8 hyperlink escapes, which wrap every linked cell.
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("\u{1b}]") {
        out.push_str(&rest[..start]);
        rest = rest[start..]
            .find("\u{1b}\\")
            .map_or("", |end| &rest[start + end + 2..]);
    }
    out.push_str(rest);
    out
}

/// How many halfblock cells the screen shows inside the preview. (A cell may carry
/// its glyph inside an OSC 8 hyperlink, so the glyph is matched within the symbol.)
fn halfblocks(app: &mut App) -> usize {
    let rect = app.markdown_preview_rect;
    let buffer = frame(app, 100, 30);
    let mut count = 0;
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            if buffer[(x, y)].symbol().contains('▀') {
                count += 1;
            }
        }
    }
    count
}

#[test]
fn a_local_image_reserves_rows_at_once_and_paints_once_decoded() {
    let (mut app, root) = previewing(
        "# Logo\n\n<p align=\"center\"><img src=\"logo.png\" alt=\"Logo\"></p>\n",
        &[("logo.png", &png(16, 64, [200, 40, 40]))],
    );
    // 16×64 px is 2 columns by 4 rows: reserved from the first frame, before decoding.
    assert_eq!(image_rows(&app), 4);
    assert_eq!(halfblocks(&mut app), 0, "no pixels until the decode lands");
    let screen_text = plain(&screen(&mut app, 100, 30).join("\n"));
    assert!(
        !screen_text.contains("🖼 Logo"),
        "no placeholder flashes before the reveal delay:\n{screen_text}"
    );

    app.preview_images.backdate_pending();
    let screen_text = plain(&screen(&mut app, 100, 30).join("\n"));
    assert!(
        screen_text.contains("🖼 Logo"),
        "a slow decode shows a muted placeholder:\n{screen_text}"
    );

    app.preview_images.settle();
    assert_eq!(
        halfblocks(&mut app),
        2 * 4,
        "every reserved cell is painted"
    );
    assert_eq!(image_rows(&app), 4, "the layout did not move");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_centered_image_paints_in_the_middle_of_the_preview() {
    let (mut app, root) = previewing(
        "<p align=\"center\"><img src=\"logo.png\"></p>\n",
        &[("logo.png", &png(16, 16, [0, 200, 0]))],
    );
    app.preview_images.settle();
    let rect = app.markdown_preview_rect;
    let buffer = frame(&mut app, 100, 30);
    let columns: Vec<u16> = (rect.x..rect.right())
        .filter(|&x| (rect.y..rect.bottom()).any(|y| buffer[(x, y)].symbol().contains('▀')))
        .collect();
    let (Some(&first), Some(&last)) = (columns.first(), columns.last()) else {
        panic!("the image was not painted");
    };
    let left = first - rect.x;
    let right = rect.right() - 1 - last;
    assert!(left.abs_diff(right) <= 2, "left {left}, right {right}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_remote_image_is_a_chip_and_nothing_is_fetched() {
    let (app, root) = previewing("![build](https://ci.example.com/badge.svg)\n", &[]);
    assert_eq!(image_rows(&app), 0);
    assert!(
        app.preview_images.pendings().is_empty(),
        "no load was started"
    );
    let text: String = preview_lines(&app)
        .iter()
        .map(karet_markdown::WrappedLine::text)
        .collect();
    assert!(text.contains("🖼 build"), "{text}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_image_that_fails_to_decode_turns_back_into_a_chip() {
    let mut cut = png(16, 16, [0, 0, 0]);
    cut.truncate(40);
    let (mut app, root) = previewing("![cut](cut.png)\n", &[("cut.png", &cut)]);
    assert_eq!(image_rows(&app), 1, "rows reserved from the header");
    app.preview_images.settle();
    let _ = screen(&mut app, 100, 30);
    assert_eq!(image_rows(&app), 0, "re-wrapped once the decode failed");
    let text: String = preview_lines(&app)
        .iter()
        .map(karet_markdown::WrappedLine::text)
        .collect();
    assert!(text.contains("🖼 cut"), "{text}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn ctrl_clicking_an_image_opens_it_in_its_own_tab() {
    let (mut app, root) = previewing(
        "![logo](logo.png)\n",
        &[("logo.png", &png(16, 16, [0, 0, 200]))],
    );
    app.preview_images.settle();
    let _ = screen(&mut app, 100, 30);
    let Some(hit) = app
        .markdown_link_hits
        .iter()
        .find(|hit| hit.target == "logo.png")
        .cloned()
    else {
        panic!("the image left no click target");
    };
    assert!(app.handle_markdown_link_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: hit.rect.x,
        row: hit.rect.y,
        modifiers: KeyModifiers::CONTROL,
    }));
    assert_eq!(
        app.tabs[app.active]
            .path()
            .and_then(|path| path.file_name()),
        Some(std::ffi::OsStr::new("logo.png"))
    );
    assert!(matches!(app.tabs[app.active].kind, TabKind::Image { .. }));
    let _ = std::fs::remove_dir_all(root);
}
