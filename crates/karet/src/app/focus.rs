//! Who holds the keyboard: the focused pane, the Source Control commit box,
//! and the open overlay, kept in one [`KeyboardOwner`] whose fields change only
//! through its transitions. A caller asks for the destination ("focus the
//! commit box", "close the overlay") rather than writing the flags that compete
//! for the keys, so a transition cannot leave a stale claim behind it, and the
//! side effects of an overlay closing run where it closes rather than being
//! polled for after every key.
//!
//! The precedence among the claims is read in one place,
//! [`App::input_context`](crate::app::App::input_context).

use crate::app::App;
use crate::keymap::Focus;
use crate::overlay::Overlay;
use crate::tab::SearchField;
use crate::view::View;

/// The keyboard's owner among the shell's pane focus, the commit box, and the
/// overlay. Fields are private: every change goes through a transition.
#[derive(Default)]
pub(crate) struct KeyboardOwner {
    /// Which pane has keyboard focus.
    focus: Focus,
    /// Whether keys are routed into the Source Control commit-message box.
    commit_box: bool,
    /// The open modal overlay (quick-open, a picker, the command palette).
    overlay: Option<Overlay>,
}

impl KeyboardOwner {
    /// The pane that has keyboard focus.
    #[must_use]
    pub(crate) fn focus(&self) -> Focus {
        self.focus
    }

    /// Whether the Source Control commit box takes the keys.
    #[must_use]
    pub(crate) fn commit_box_focused(&self) -> bool {
        self.commit_box
    }

    /// The open overlay, if any.
    #[must_use]
    pub(crate) fn overlay(&self) -> Option<&Overlay> {
        self.overlay.as_ref()
    }

    /// The open overlay, mutably, if any.
    pub(crate) fn overlay_mut(&mut self) -> Option<&mut Overlay> {
        self.overlay.as_mut()
    }

    /// Move pane focus to `focus`. The commit box keeps its claim: it is a
    /// field inside the sidebar that only its own transitions take or release.
    pub(crate) fn focus_on(&mut self, focus: Focus) {
        self.focus = focus;
    }

    /// Move pane focus to the editor.
    pub(crate) fn focus_editor(&mut self) {
        self.focus_on(Focus::Editor);
    }

    /// Move pane focus to the sidebar.
    pub(crate) fn focus_sidebar(&mut self) {
        self.focus_on(Focus::Sidebar);
    }

    /// Move pane focus to the outline panel.
    pub(crate) fn focus_outline(&mut self) {
        self.focus_on(Focus::Outline);
    }

    /// Route the keys into the commit box, which lives in the sidebar.
    pub(crate) fn focus_commit_box(&mut self) {
        self.focus = Focus::Sidebar;
        self.commit_box = true;
    }

    /// Take the keys back from the commit box, keeping pane focus where it is.
    pub(crate) fn release_commit_box(&mut self) {
        self.commit_box = false;
    }

    /// Hand the keys to a text field in the editor area (the find bar): focus
    /// the editor and release the commit box, which would otherwise outrank it.
    fn focus_editor_field(&mut self) {
        self.focus = Focus::Editor;
        self.commit_box = false;
    }

    /// Show `overlay`, returning the one it replaced.
    fn replace_overlay(&mut self, overlay: Overlay) -> Option<Overlay> {
        self.overlay.replace(overlay)
    }

    /// Close the open overlay, returning it.
    fn take_overlay(&mut self) -> Option<Overlay> {
        self.overlay.take()
    }
}

impl App {
    /// Show `overlay` over the shell. One it replaces closes as
    /// [`close_overlay`](Self::close_overlay) would close it.
    pub(crate) fn open_overlay(&mut self, overlay: Overlay) {
        if let Some(previous) = self.keyboard.replace_overlay(overlay) {
            self.overlay_closed(&previous);
        }
    }

    /// Close the open overlay, returning it. Closing the theme picker without a
    /// pick puts back the theme from before it, here rather than on the next
    /// key, so a pointer that closed it leaves no preview behind.
    pub(crate) fn close_overlay(&mut self) -> Option<Overlay> {
        let closed = self.keyboard.take_overlay();
        if let Some(overlay) = &closed {
            self.overlay_closed(overlay);
        }
        closed
    }

    /// The side effects of `overlay` leaving the screen.
    fn overlay_closed(&mut self, overlay: &Overlay) {
        if overlay.is_theme_picker() {
            self.restore_theme_preview();
        }
    }

    /// Hand the keyboard to the active tab's find bar, on `field`.
    pub(super) fn focus_find(&mut self, field: SearchField) {
        if let Some(find) = self.active_find_mut() {
            find.field = field;
            find.focused = true;
        }
        self.keyboard.focus_editor_field();
    }

    /// Move focus between the sidebar and the editor.
    pub(super) fn toggle_focus(&mut self) {
        // A Problems filter edit belongs to one visit: leaving ends it, and
        // coming back lands on the list rather than resuming the edit.
        self.problems.filter_input = false;
        let next = match self.keyboard.focus() {
            Focus::Sidebar => Focus::Editor,
            // A view that owns the full width has no sidebar to toggle into;
            // focus would otherwise land on a panel that is not on screen.
            Focus::Editor if self.view.shows_sidebar() => Focus::Sidebar,
            Focus::Editor => Focus::Editor,
            // Toggling out of the outline returns to the editor it annotates.
            Focus::Outline => Focus::Editor,
        };
        self.keyboard.focus_on(next);
    }

    /// Show `view` in the content area, moving focus onto it.
    ///
    /// Focus follows the switch for the same reason selecting a sidebar panel
    /// focuses it: the user asked for that surface. It is also load-bearing for a
    /// view that hides the sidebar — leaving focus on a panel that is no longer
    /// drawn would strand every keystroke somewhere invisible.
    pub(super) fn select_view(&mut self, view: View) {
        self.view = view;
        self.keyboard.focus_editor();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::TextPurpose;

    #[test]
    fn a_fresh_owner_focuses_the_sidebar_with_nothing_claiming_the_keys() {
        let owner = KeyboardOwner::default();
        assert_eq!(owner.focus(), Focus::Sidebar);
        assert!(!owner.commit_box_focused());
        assert!(owner.overlay().is_none());
    }

    #[test]
    fn focusing_the_commit_box_moves_pane_focus_to_the_sidebar() {
        let mut owner = KeyboardOwner::default();
        owner.focus_editor();
        owner.focus_commit_box();
        assert_eq!(owner.focus(), Focus::Sidebar);
        assert!(owner.commit_box_focused());
    }

    #[test]
    fn a_pane_focus_move_leaves_the_commit_box_claim_alone() {
        let mut owner = KeyboardOwner::default();
        owner.focus_commit_box();
        owner.focus_outline();
        assert_eq!(owner.focus(), Focus::Outline);
        assert!(owner.commit_box_focused());
        owner.release_commit_box();
        assert!(!owner.commit_box_focused());
        assert_eq!(owner.focus(), Focus::Outline);
    }

    #[test]
    fn an_editor_field_takes_the_keys_from_the_commit_box() {
        let mut owner = KeyboardOwner::default();
        owner.focus_commit_box();
        owner.focus_editor_field();
        assert_eq!(owner.focus(), Focus::Editor);
        assert!(!owner.commit_box_focused());
    }

    #[test]
    fn replacing_and_taking_the_overlay_hand_back_the_one_that_left() {
        let mut owner = KeyboardOwner::default();
        let prompt = || Overlay::text("Evaluate", TextPurpose::DebugEvaluate);
        assert!(owner.replace_overlay(prompt()).is_none());
        assert!(owner.replace_overlay(prompt()).is_some());
        assert!(owner.take_overlay().is_some());
        assert!(owner.overlay().is_none());
        assert!(owner.take_overlay().is_none());
    }
}
