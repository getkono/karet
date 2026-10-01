//! The cache on a Kitty terminal: images go out once, at full resolution, and each
//! cell box gets its own placement.

use std::sync::Arc;

use karet_fileview::image::Image;

use super::*;

fn ready_on_kitty(files: &[(&str, &[u8])]) -> (tempfile::TempDir, PreviewImages) {
    let dir = workspace(files);
    let images = PreviewImages::default();
    images.configure(true, (8, 16));
    for (name, _) in files {
        let _ = size(&images, dir.path(), name);
    }
    images.settle();
    (dir, images)
}

#[test]
fn a_ready_image_is_transmitted_once_and_placed_per_box() {
    let (dir, images) = ready_on_kitty(&[("a.png", &png(40, 20, [1, 2, 3]))]);
    let Lookup::Ready(Paint::Placeholder { id, placement }) =
        lookup_in(&images, dir.path(), "a.png", (5, 2))
    else {
        panic!("a Kitty terminal paints placeholders");
    };
    assert_eq!(placement, 1);
    let output = images.take_output();
    assert!(
        output.starts_with(&format!("\x1b_Ga=t,i={id},f=32,s=40,v=20,o=z,q=2,")),
        "the full 40×20 image goes out: {output:?}"
    );
    assert!(output.ends_with(&crate::preview_images::kitty::place(id, 1, 5, 2)));
    // The same box again: nothing more to say.
    let _ = lookup_in(&images, dir.path(), "a.png", (5, 2));
    assert_eq!(images.take_output(), "");
    // A new box (another pane, or a resize) is a new placement, not a re-send.
    let Lookup::Ready(Paint::Placeholder { placement, .. }) =
        lookup_in(&images, dir.path(), "a.png", (3, 1))
    else {
        panic!("still a placeholder");
    };
    assert_eq!(placement, 2);
    assert_eq!(
        images.take_output(),
        crate::preview_images::kitty::place(id, 2, 3, 1)
    );
}

#[test]
fn an_evicted_image_is_deleted_from_the_terminal_and_sent_again_on_sight() {
    let dir = workspace(&[
        ("a.png", &png(4, 4, [1, 1, 1])),
        ("b.png", &png(4, 4, [2, 2, 2])),
    ]);
    let images = PreviewImages::with_budget(100);
    images.configure(true, (8, 16));
    let _ = size(&images, dir.path(), "a.png");
    images.settle();
    let Lookup::Ready(Paint::Placeholder { id, .. }) = lookup(&images, dir.path(), "a.png") else {
        panic!("`a` is ready");
    };
    let _ = images.take_output();
    // Two frames away from `a`, then `b` lands and `a` makes room for it.
    images.end_frame();
    images.end_frame();
    let _ = size(&images, dir.path(), "b.png");
    images.settle();
    assert!(
        images
            .take_output()
            .contains(&karet_fileview::image::kitty_delete_image(id)),
        "the terminal drops `a` with our copy"
    );
    assert!(matches!(
        lookup(&images, dir.path(), "a.png"),
        Lookup::Loading(_)
    ));
    images.settle();
    let _ = lookup(&images, dir.path(), "a.png");
    assert!(
        images.take_output().contains(&format!("a=t,i={id},")),
        "and gets it back"
    );
}

#[test]
fn teardown_deletes_every_image_the_terminal_holds() {
    let (dir, images) = ready_on_kitty(&[
        ("a.png", &png(2, 2, [1, 1, 1])),
        ("b.png", &png(2, 2, [2, 2, 2])),
    ]);
    let mut ids = Vec::new();
    for name in ["a.png", "b.png"] {
        if let Lookup::Ready(Paint::Placeholder { id, .. }) = lookup(&images, dir.path(), name) {
            ids.push(id);
        }
    }
    assert_eq!(ids.len(), 2);
    let _ = images.take_output();
    let teardown = images.teardown();
    for id in ids {
        assert!(teardown.contains(&karet_fileview::image::kitty_delete_image(id)));
    }
    assert_eq!(images.teardown(), "", "and only once");
}

#[test]
fn off_kitty_the_resample_is_made_once_per_box() {
    let dir = workspace(&[("a.png", &png(64, 64, [5, 6, 7]))]);
    let images = PreviewImages::default();
    let _ = size(&images, dir.path(), "a.png");
    images.settle();
    let pixels = |cells| match lookup_in(&images, dir.path(), "a.png", cells) {
        Lookup::Ready(Paint::Pixels(image)) => Some(image),
        _ => None,
    };
    let (first, again) = (pixels((8, 4)), pixels((8, 4)));
    assert!(
        first
            .as_ref()
            .zip(again.as_ref())
            .is_some_and(|(a, b)| Arc::ptr_eq(a, b))
    );
    assert!(first.is_some_and(|image| (image.width(), image.height()) == (8, 8)));
    assert!(pixels((4, 2)).is_some_and(|image| (image.width(), image.height()) == (4, 4)));
    assert_eq!(
        images.take_output(),
        "",
        "nothing is written to a non-Kitty terminal"
    );
}

#[test]
fn a_new_cell_size_resizes_every_image() {
    let dir = workspace(&[("a.png", &png(80, 80, [1, 1, 1]))]);
    let images = PreviewImages::default();
    let source = dir.path().join("README.md");
    let before = images.generation();
    images.configure(false, karet_markdown::DEFAULT_CELL_PIXELS);
    assert_eq!(
        images.generation(),
        before,
        "an unchanged cell size re-wraps nothing"
    );
    images.configure(false, (10, 20));
    assert_ne!(images.generation(), before);
    assert_eq!(images.sizer(&source, dir.path()).cell_pixels(), (10, 20));
    let rows = karet_markdown::parse("![a](a.png)")
        .wrap_with(40, &images.sizer(&source, dir.path()))
        .lines
        .len();
    assert_eq!(rows, 4, "80 px on 20-pixel rows");
}

#[test]
fn a_changed_file_drops_its_old_image_from_the_terminal() {
    let (dir, images) = ready_on_kitty(&[("a.png", &png(2, 2, [1, 1, 1]))]);
    let Lookup::Ready(Paint::Placeholder { id, .. }) = lookup(&images, dir.path(), "a.png") else {
        panic!("`a` is ready");
    };
    let _ = images.take_output();
    let _ = std::fs::write(dir.path().join("a.png"), png(3, 3, [2, 2, 2]));
    images.backdate_checks();
    assert_eq!(size(&images, dir.path(), "a.png"), Some((3, 3)));
    assert_eq!(
        images.take_output(),
        karet_fileview::image::kitty_delete_image(id),
        "the stale pixels leave the terminal"
    );
    images.settle();
    let Lookup::Ready(Paint::Placeholder { id: new, .. }) = lookup(&images, dir.path(), "a.png")
    else {
        panic!("the new file is ready");
    };
    assert_ne!(new, id, "under a fresh id");
}

#[test]
fn a_refused_image_is_dropped_from_the_terminal() {
    let (dir, images) = ready_on_kitty(&[("a.png", &png(2, 2, [1, 1, 1]))]);
    let Lookup::Ready(Paint::Placeholder { id, .. }) = lookup(&images, dir.path(), "a.png") else {
        panic!("`a` is ready");
    };
    let _ = images.take_output();
    let _ = std::fs::remove_file(dir.path().join("a.png"));
    images.backdate_checks();
    assert_eq!(size(&images, dir.path(), "a.png"), None);
    assert_eq!(
        images.take_output(),
        karet_fileview::image::kitty_delete_image(id)
    );
}

#[test]
fn two_boxes_of_one_image_keep_their_resamples() {
    let dir = workspace(&[("a.png", &png(64, 64, [5, 6, 7]))]);
    let images = PreviewImages::default();
    let _ = size(&images, dir.path(), "a.png");
    images.settle();
    let pixels = |cells| match lookup_in(&images, dir.path(), "a.png", cells) {
        Lookup::Ready(Paint::Pixels(image)) => Some(image),
        _ => None,
    };
    let (wide, narrow) = (pixels((8, 4)), pixels((4, 2)));
    // Alternating between two panes' boxes resamples neither again.
    let same = |a: &Option<Arc<Image>>, b: Option<Arc<Image>>| {
        a.as_ref()
            .zip(b.as_ref())
            .is_some_and(|(a, b)| Arc::ptr_eq(a, b))
    };
    assert!(same(&wide, pixels((8, 4))));
    assert!(same(&narrow, pixels((4, 2))));
}
