//! The commit box's AI chip: one right-aligned slot in the border that says
//! what generation will do, is doing, or just did.
//!
//! The slot is painted for every state, including the states with nothing to
//! say, so the border never reflows as a generation starts, animates, and ends —
//! an affordance used many times a day must not make the box twitch.
//!
//! What it shows is the resolved configuration, pushed from the backend before
//! anything runs, so the model that would be used and the reason nothing would
//! happen are both visible without pressing anything.

use std::time::Instant;

use karet_core::ThemeRole;
use karet_filetype::IconStyle;
use karet_theme::Theme;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::app::App;
use crate::app::scm::aicommit::AiCommitState;
use crate::app::scm::aicommit::generating_label;

/// The chip's text and the role it is painted in.
pub(super) struct Chip {
    /// What the chip reads.
    pub(super) label: String,
    /// The colour role carrying its meaning.
    pub(super) role: ThemeRole,
}

/// The mark that introduces the chip.
///
/// Nerd Font and Unicode tiers get a sparkle; the ASCII tier gets `AI`, since
/// every single-cell ASCII alternative reads as punctuation rather than a mark.
fn mark(style: IconStyle) -> &'static str {
    match style {
        IconStyle::NerdFont | IconStyle::Unicode => "\u{2728}",
        IconStyle::Ascii => "AI",
    }
}

/// What the chip should say, or `None` while the backend has not answered yet.
///
/// `None` is deliberately not "ready": claiming an agent before one was probed
/// is the mistake the whole availability push exists to prevent.
pub(super) fn chip(app: &App, now: Instant, style: IconStyle) -> Option<Chip> {
    let mark = mark(style);
    match &app.ai_commit.state {
        AiCommitState::Generating { since, .. } => {
            // Below the shared reveal delay the chip keeps saying whatever it
            // said before, so a fast generation never flashes a spinner.
            if !since.visible() {
                return idle_chip(app, mark);
            }
            Some(Chip {
                label: format!("{} · Esc", generating_label(*since, now, style)),
                role: ThemeRole::LineNumberActive,
            })
        },
        AiCommitState::Applied { .. } => Some(Chip {
            label: format!("{mark} applied · Ctrl+Z undo"),
            role: ThemeRole::LineNumberActive,
        }),
        AiCommitState::Failed { reason } => Some(Chip {
            label: format!("{mark} {reason}"),
            role: ThemeRole::DiagnosticError,
        }),
        AiCommitState::Idle => idle_chip(app, mark),
    }
}

/// The resting chip: the model that would run, or why nothing would.
fn idle_chip(app: &App, mark: &str) -> Option<Chip> {
    let availability = app.ai_commit.availability.as_ref()?;
    match availability.blocker() {
        Some(blocker) => Some(Chip {
            label: format!("{mark} {blocker}"),
            role: ThemeRole::LineNumber,
        }),
        None => {
            let model = app.ai_commit.model_label().unwrap_or("auto");
            Some(Chip {
                label: format!("{mark} {model} · Ctrl+G"),
                role: ThemeRole::LineNumber,
            })
        },
    }
}

/// Where the chip sits: right-aligned in `area`'s top border, clipped to what
/// fits beside the block's own title.
///
/// Returns `None` when the border is too narrow to carry it, which is what keeps
/// the chip from colliding with the title on a squeezed sidebar.
pub(super) fn chip_rect(area: Rect, title_width: u16, label_width: u16) -> Option<Rect> {
    // One cell of padding each side of the label, plus the block's own corners.
    let needed = label_width.saturating_add(2);
    let available = area.width.saturating_sub(title_width).saturating_sub(2);
    if label_width == 0 || needed > available {
        return None;
    }
    Some(Rect {
        x: area.right().saturating_sub(needed).saturating_sub(1),
        y: area.y,
        width: needed,
        height: 1,
    })
}

/// The style for a chip's role.
pub(super) fn chip_style(theme: &Theme, role: ThemeRole) -> Style {
    Style::default().fg(theme.role(role).to_ratatui())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chip_is_right_aligned_and_yields_when_the_border_is_full() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 5,
        };
        let rect = chip_rect(area, 16, 10).expect("a 40-wide border fits a 10-cell chip");
        assert_eq!(rect.y, area.y, "the chip rides the top border");
        assert_eq!(rect.height, 1);
        assert!(
            rect.right() < area.right(),
            "it stops short of the corner: {rect:?}"
        );
        assert!(
            rect.x > 16,
            "and starts after the title rather than over it: {rect:?}"
        );

        // Too narrow to hold both: the chip yields rather than overprinting.
        assert_eq!(chip_rect(area, 30, 10), None);
        let narrow = Rect { width: 8, ..area };
        assert_eq!(chip_rect(narrow, 6, 10), None);
        // An empty label claims no space at all.
        assert_eq!(chip_rect(area, 16, 0), None);
    }

    /// An app with availability reported and the chip in `state`.
    fn app_with(
        state: AiCommitState,
        availability: Option<karet_session::AiCommitAvailability>,
    ) -> App {
        let mut app = crate::app::tests::support::app();
        app.ai_commit.availability = availability;
        app.ai_commit.state = state;
        app
    }

    fn ready() -> karet_session::AiCommitAvailability {
        karet_session::AiCommitAvailability {
            supported: true,
            enabled: true,
            options: karet_session::AiCommit::default(),
            agents: vec![karet_session::AiCommitAgentStatus {
                agent: karet_session::AiCommitAgent::Claude,
                available: true,
                detail: "claude 2.1".to_string(),
            }],
            effort_conflict: None,
        }
    }

    #[test]
    fn the_resting_chip_names_the_model_and_the_key() {
        let app = app_with(AiCommitState::Idle, Some(ready()));
        let chip = chip(&app, Instant::now(), IconStyle::Unicode).expect("a ready chip");
        assert!(chip.label.contains("auto"), "{}", chip.label);
        assert!(chip.label.contains("Ctrl+G"), "{}", chip.label);
        assert_eq!(chip.role, ThemeRole::LineNumber, "resting is muted");
    }

    #[test]
    fn nothing_is_claimed_before_the_backend_answers() {
        let app = app_with(AiCommitState::Idle, None);
        assert!(
            chip(&app, Instant::now(), IconStyle::Unicode).is_none(),
            "an unprobed setup must not advertise a key that would fail"
        );
    }

    #[test]
    fn a_fast_generation_never_flashes_a_spinner() {
        let now = Instant::now();
        let app = app_with(
            AiCommitState::Generating {
                request: karet_session::RequestId(1),
                since: crate::app::Pending::at(now),
                draft: String::new(),
            },
            Some(ready()),
        );
        let chip = chip(&app, now, IconStyle::Unicode).expect("the resting chip persists");
        // Below the reveal delay the chip still reads as it did at rest.
        assert!(chip.label.contains("Ctrl+G"), "{}", chip.label);
        assert!(!chip.label.contains("generating"), "{}", chip.label);
    }

    #[test]
    fn a_slow_generation_shows_progress_and_the_way_out() {
        let now = Instant::now();
        let since = now - crate::app::LOADING_REVEAL_DELAY - std::time::Duration::from_millis(1);
        let app = app_with(
            AiCommitState::Generating {
                request: karet_session::RequestId(1),
                since: crate::app::Pending::at(since),
                draft: String::new(),
            },
            Some(ready()),
        );
        let chip = chip(&app, now, IconStyle::Unicode).expect("a revealed chip");
        assert!(chip.label.contains("generating"), "{}", chip.label);
        assert!(
            chip.label.contains("Esc"),
            "cancelling is offered: {}",
            chip.label
        );
    }

    #[test]
    fn a_failure_is_coloured_as_one_and_says_why() {
        let app = app_with(
            AiCommitState::Failed {
                reason: "`claude` was not found on PATH".to_string(),
            },
            Some(ready()),
        );
        let chip = chip(&app, Instant::now(), IconStyle::Unicode).expect("a failed chip");
        assert!(chip.label.contains("not found on PATH"), "{}", chip.label);
        assert_eq!(chip.role, ThemeRole::DiagnosticError);
    }

    #[test]
    fn an_applied_message_advertises_its_undo() {
        let app = app_with(
            AiCommitState::Applied {
                undo: "my draft".to_string(),
            },
            Some(ready()),
        );
        let chip = chip(&app, Instant::now(), IconStyle::Unicode).expect("an applied chip");
        assert!(chip.label.contains("Ctrl+Z"), "{}", chip.label);
    }

    #[test]
    fn the_ascii_tier_gets_a_readable_mark() {
        // The sparkle is two columns of nothing on a terminal without it.
        assert_eq!(mark(IconStyle::Ascii), "AI");
        assert_eq!(mark(IconStyle::NerdFont), mark(IconStyle::Unicode));
    }
}
