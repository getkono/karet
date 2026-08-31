//! The Source-Control commit box's AI state: what the backend says is possible,
//! what is in flight, and what the last run left behind.
//!
//! Three things make this more than a boolean.
//!
//! **Availability arrives before the request.** The backend probes the agent and
//! pushes an [`AiCommitAvailability`]; the widget renders that, so the resolved
//! model — or the reason nothing will happen — is visible before the user
//! presses anything, rather than seconds afterwards.
//!
//! **The run is long and interruptible.** A generation is a multi-second round
//! trip to a process, so it anchors a [`Pending`] like every other slow load in
//! the app: nothing renders for the first 200 ms, and past that a spinner
//! animates against the shared reveal policy. It stays cancellable throughout,
//! and a superseded run's answer is dropped rather than applied.
//!
//! **The answer must not destroy a draft.** The draft is snapshotted when the
//! run starts. If it is untouched on arrival the message simply replaces it; if
//! the user typed while waiting, the message still lands but the previous draft
//! is kept so a single undo brings it back. That undo target lives on
//! [`AiCommitUi`] rather than inside a state variant, so cancelling, failing, or
//! regenerating cannot quietly throw away the user's own words.

use std::time::Duration;
use std::time::Instant;

use karet_core::NotificationKind;
use karet_session::AiCommitAvailability;
use karet_session::Command as SessionCommand;
use karet_session::RequestId;
use karet_widgets::Spinner;

use crate::app::App;
use crate::app::Pending;
use crate::app::Report;

/// How long a generation runs before the elapsed seconds join the spinner.
///
/// Below this the spinner alone reads as "working"; past it the user is waiting
/// long enough to want to know *how* long, and a number that appears too eagerly
/// just adds noise to the common fast path.
const ELAPSED_REVEAL: Duration = Duration::from_secs(2);

/// What the commit box's AI affordance is currently doing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum AiCommitState {
    /// Nothing in flight, nothing to report.
    #[default]
    Idle,
    /// A generation is running and may be cancelled.
    Generating {
        /// The request to correlate the answer with, and to cancel.
        request: RequestId,
        /// When it started — drives the delayed reveal and the spinner phase.
        since: Pending,
        /// The draft as it was when the run began, so the answer can tell
        /// whether the user has typed since.
        draft: String,
    },
    /// A generated message was applied over a draft that is still recoverable.
    Applied,
    /// The last run produced no message.
    Failed {
        /// Why, phrased for display.
        reason: String,
    },
}

/// The commit box's AI affordance: backend-reported capability plus local state.
#[derive(Clone, Debug, Default)]
pub(crate) struct AiCommitUi {
    /// The last availability the backend pushed. `None` until it answers.
    pub(crate) availability: Option<AiCommitAvailability>,
    /// What the affordance is doing right now.
    pub(crate) state: AiCommitState,
    /// Text the user wrote that a generated message replaced, still restorable.
    ///
    /// Deliberately *outside* [`AiCommitState`]: the undo target belongs to the
    /// box, not to whatever the affordance happens to be doing. Hanging it off
    /// one variant meant every transition away from that variant — cancelling a
    /// second run, a second run failing, a refusal before one even started —
    /// silently discarded the user's own words. It survives until it is used or
    /// the commit lands.
    pub(crate) undo: Option<String>,
}

impl AiCommitUi {
    /// The in-flight request, if a generation is running.
    pub(crate) fn generating(&self) -> Option<(RequestId, Pending)> {
        match self.state {
            AiCommitState::Generating { request, since, .. } => Some((request, since)),
            _ => None,
        }
    }

    /// Why generation is unavailable, or `None` when it would run.
    ///
    /// This is the single question the affordance asks. Before the backend has
    /// answered, `availability` is `None` and there is no blocker to report *and*
    /// no readiness to claim — which is why callers test the availability itself
    /// for presence rather than treating "no blocker" as "ready".
    pub(crate) fn blocker(&self) -> Option<String> {
        self.availability
            .as_ref()
            .and_then(AiCommitAvailability::blocker)
    }

    /// The model a generation would run, for display beside the affordance.
    ///
    /// `"auto"` stays `"auto"`: the concrete model is chosen from the diff's
    /// size, which is not known until the diff is read, and naming one here
    /// would be a guess the user could catch us getting wrong.
    pub(crate) fn model_label(&self) -> Option<&str> {
        self.availability
            .as_ref()
            .map(|status| status.options.model.as_str())
    }
}

impl App {
    /// Ask the backend to draft a commit message from the staged diff.
    ///
    /// A run already in flight is cancelled rather than raced. When the backend
    /// has already reported that nothing can run, say so immediately instead of
    /// sending a request whose only possible answer is the same refusal.
    pub(crate) fn commit_generate(&mut self) {
        if let Some(reason) = self.ai_commit.blocker() {
            self.ai_commit.state = AiCommitState::Failed { reason };
            return;
        }
        if let Some((previous, _)) = self.ai_commit.generating() {
            self.send_command(SessionCommand::Cancel { request: previous });
        }
        let draft = self.commit_input.text.clone();
        match self.send(SessionCommand::GenerateCommitMessage) {
            Some(request) => {
                self.ai_commit.state = AiCommitState::Generating {
                    request,
                    since: Pending::start(),
                    draft,
                };
            },
            None => self.ai_commit.state = AiCommitState::Idle,
        }
    }

    /// Stop an in-flight generation, leaving the draft exactly as it is.
    ///
    /// Returns whether there was one — the commit box's `Esc` cancels the run
    /// first and only blurs the field once nothing is running.
    pub(crate) fn commit_generate_cancel(&mut self) -> bool {
        let Some((request, _)) = self.ai_commit.generating() else {
            return false;
        };
        self.send_command(SessionCommand::Cancel { request });
        self.ai_commit.state = AiCommitState::Idle;
        self.notify(
            Report::Outcome,
            NotificationKind::Vcs,
            "commit message generation cancelled",
        );
        true
    }

    /// Restore the draft a generated message replaced.
    ///
    /// Available whatever the affordance is doing, because the undo target
    /// outlives the run that created it.
    pub(crate) fn commit_generate_undo(&mut self) -> bool {
        let Some(restored) = self.ai_commit.undo.take() else {
            return false;
        };
        self.set_commit_text(restored);
        if matches!(self.ai_commit.state, AiCommitState::Applied) {
            self.ai_commit.state = AiCommitState::Idle;
        }
        self.notify(
            Report::Outcome,
            NotificationKind::Vcs,
            "restored the previous commit message",
        );
        true
    }

    /// Adopt the availability the backend pushed.
    ///
    /// A run in flight is left alone: it was started under the configuration
    /// that was current when it began, and its answer is still wanted.
    pub(crate) fn on_ai_commit_availability(&mut self, status: AiCommitAvailability) {
        self.ai_commit.availability = Some(status);
    }

    /// Fill the commit editor with a generated message.
    ///
    /// Ignores anything but the answer to the run currently in flight, so a
    /// superseded or already-cancelled request cannot overwrite the box.
    pub(crate) fn on_commit_message_generated(
        &mut self,
        request: Option<RequestId>,
        message: String,
    ) {
        let AiCommitState::Generating {
            request: expected,
            draft,
            ..
        } = &self.ai_commit.state
        else {
            return;
        };
        if request != Some(*expected) {
            return;
        }
        // The message lands either way — having asked for it and then having to
        // ask again is the worse outcome. What the user typed decides only what
        // stays recoverable: their newest words if they typed while waiting,
        // otherwise whatever an earlier generation already displaced.
        let touched = self.commit_input.text != *draft;
        let previous = std::mem::take(&mut self.commit_input.text);
        if touched {
            self.ai_commit.undo = Some(previous);
        }
        self.ai_commit.state = if self.ai_commit.undo.is_some() {
            AiCommitState::Applied
        } else {
            AiCommitState::Idle
        };
        self.set_commit_text(message);
        self.notify(
            Report::Outcome,
            NotificationKind::Vcs,
            "commit message generated",
        );
    }

    /// Report that a generation produced no message.
    ///
    /// The reason goes to two places on purpose. The chip is the surface the
    /// user is already looking at, but it lives in a border a few cells wide and
    /// has to shorten to fit; a git error carrying multiple lines of stderr
    /// would be reduced to the word "failed" there. The notification takes the
    /// full text, so the detail is never only in the place that cannot show it.
    pub(crate) fn on_commit_message_failed(&mut self, request: Option<RequestId>, reason: String) {
        // A failure with no request is an unsolicited one (a backend that could
        // not attribute it); show it rather than dropping it on the floor.
        if let Some((expected, _)) = self.ai_commit.generating()
            && request.is_some_and(|id| id != expected)
        {
            return;
        }
        self.notify(
            Report::Failure,
            NotificationKind::Vcs,
            format!("commit message: {}", one_line(&reason)),
        );
        self.ai_commit.state = AiCommitState::Failed {
            reason: one_line(&reason),
        };
    }

    /// Replace the commit draft, putting the caret at the end.
    fn set_commit_text(&mut self, text: String) {
        self.commit_input.text = text;
        let end = self.commit_input.text.len();
        self.commit_input
            .edit
            .set_cursor(&self.commit_input.text, end, false);
        self.commit_input.edit.scroll = 0;
    }

    /// When the AI affordance next needs repainting.
    ///
    /// Before the reveal threshold that is the threshold itself, so the spinner
    /// appears without waiting for a keystroke; after it, one frame interval, so
    /// the animation keeps running while the user does nothing.
    pub(crate) fn ai_commit_next_wake(&self, now: Instant) -> Option<Duration> {
        let (_, since) = self.ai_commit.generating()?;
        Some(since.wake(now).unwrap_or(Spinner::FRAME_INTERVAL))
    }
}

/// Collapse `text` onto one line, for anywhere a width is measured.
///
/// `git` reports failures as multi-line stderr, and both the chip and the card
/// line are single-row surfaces — a newline there measures as width but paints
/// as nothing, so a two-line reason silently overflows its slot and disappears.
pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The label for a running generation: a spinner, and the elapsed seconds once
/// the wait is long enough to be worth counting.
pub(crate) fn generating_label(
    since: Pending,
    now: Instant,
    icon_style: karet_filetype::IconStyle,
) -> String {
    let elapsed = since.elapsed_since(now);
    let frame = Spinner::new(icon_style).frame(elapsed);
    if elapsed >= ELAPSED_REVEAL {
        format!("{frame} generating… {}s", elapsed.as_secs())
    } else {
        format!("{frame} generating…")
    }
}

impl App {
    /// Open the AI commit-message options, re-probing so the agents' state is
    /// current rather than whatever was true at startup.
    pub(crate) fn open_ai_commit_form(&mut self) {
        let options = self
            .ai_commit
            .availability
            .as_ref()
            .map(|status| status.options.clone())
            .unwrap_or_else(|| self.settings.git.ai_commit.clone());
        self.send_command(SessionCommand::ProbeAiCommit);
        self.overlay = Some(crate::overlay::Overlay::AiCommit(
            crate::overlay::AiCommitForm::new(options, self.ai_commit.availability.clone()),
        ));
    }

    /// Persist the options the form assembled.
    ///
    /// The backend answers with a fresh availability, so the chip reflects the
    /// new configuration without the client having to guess at it.
    pub(crate) fn save_ai_commit_options(&mut self, options: karet_session::AiCommit) {
        // A configuration change can only invalidate a run already in flight;
        // let it finish under what it started with rather than half-applying.
        self.send_command(SessionCommand::SetAiCommitOptions {
            options: Box::new(options),
        });
    }
}
