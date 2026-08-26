//! The tab strip's one-cell save mark, which animates the shared spinner widget.

use karet_widgets::Spinner;

use super::super::SPINNER_DELAY;
use super::super::save_mark;
use crate::ui::tests::test_code_tab;

#[test]
fn the_tab_save_mark_is_one_cell_and_spins_only_for_a_slow_save() {
    use std::time::Instant;

    use unicode_width::UnicodeWidthChar;

    let nerd = karet_filetype::IconStyle::NerdFont;
    let mut tab = test_code_tab("/repo/slow.rs");
    assert_eq!(save_mark(&tab, nerd), ' ');

    tab.dirty = true;
    assert_eq!(save_mark(&tab, nerd), '\u{25cf}');

    // A save that has only just begun keeps the dirty mark: the spinner is a
    // save-specific reveal policy, not something the widget decides.
    tab.saving_since = Some(Instant::now());
    assert_eq!(save_mark(&tab, nerd), '\u{25cf}');

    // Past the delay it animates the shared widget's cycle for the active tier,
    // and every tier fills the same single cell.
    tab.saving_since = Instant::now().checked_sub(SPINNER_DELAY);
    for style in [
        nerd,
        karet_filetype::IconStyle::Unicode,
        karet_filetype::IconStyle::Ascii,
    ] {
        let mark = save_mark(&tab, style);
        assert!(
            Spinner::new(style).frames().contains(&mark),
            "{style:?} save mark {mark:?} is not a spinner frame",
        );
        // The slot is one cell wide in every branch, so the tab strip never shifts.
        assert_eq!(
            UnicodeWidthChar::width(mark),
            Some(1),
            "{style:?} save mark {mark:?} is not one cell",
        );
    }
}
