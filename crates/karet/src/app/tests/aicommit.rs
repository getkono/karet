//! The commit box's AI affordance: what it reports, what it protects, and what
//! it refuses to act on.

use karet_session::AiCommitAgent;
use karet_session::AiCommitAgentStatus;
use karet_session::AiCommitAvailability;
use karet_session::RequestId;

use super::support::*;
use crate::app::scm::aicommit::AiCommitState;
use crate::app::*;

/// Availability as the backend reports it for a working setup.
fn ready() -> AiCommitAvailability {
    AiCommitAvailability {
        supported: true,
        enabled: true,
        options: karet_session::AiCommit::default(),
        agents: vec![AiCommitAgentStatus {
            agent: AiCommitAgent::Claude,
            available: true,
            detail: "claude 2.1".to_string(),
        }],
        effort_conflict: None,
    }
}

/// Put `app` into a generation started with `draft` already in the box.
fn generating(app: &mut App, draft: &str) -> RequestId {
    let request = RequestId(7);
    app.commit_input.text = draft.to_string();
    app.ai_commit.state = AiCommitState::Generating {
        request,
        since: Pending::start(),
        draft: draft.to_string(),
    };
    request
}

#[test]
fn an_untouched_draft_is_replaced_without_leaving_an_undo() {
    let mut app = app();
    let request = generating(&mut app, "wip");

    app.on_commit_message_generated(Some(request), "feat: real message".to_string());

    assert_eq!(app.commit_input.text, "feat: real message");
    // Nothing was lost, so there is nothing to offer back.
    assert_eq!(app.ai_commit.state, AiCommitState::Idle);
}

#[test]
fn a_draft_typed_during_the_run_stays_recoverable() {
    let mut app = app();
    let request = generating(&mut app, "wip");
    // The user keeps typing while the agent works.
    app.commit_input.text = "wip: my own words".to_string();

    app.on_commit_message_generated(Some(request), "feat: generated".to_string());

    assert_eq!(app.commit_input.text, "feat: generated", "the answer lands");
    assert_eq!(
        app.ai_commit.state,
        AiCommitState::Applied,
        "and what it replaced is held, not discarded"
    );

    assert!(app.commit_generate_undo());
    assert_eq!(app.commit_input.text, "wip: my own words");
    assert_eq!(app.ai_commit.state, AiCommitState::Idle);
    // Undo is spent: a second press has nothing to restore.
    assert!(!app.commit_generate_undo());
}

#[test]
fn asking_for_another_message_still_undoes_to_the_users_own_words() {
    let mut app = app();
    // A real backend, since the second run is started through `commit_generate`
    // and needs a request id back.
    app.backend = Some(std::sync::Arc::new(RecordingBackend::new()));
    app.on_ai_commit_availability(ready());

    // Type, generate, and take the first message over that draft.
    let first = generating(&mut app, "");
    app.commit_input.text = "fix the parser thing".to_string();
    app.on_commit_message_generated(Some(first), "fix: first attempt".to_string());
    assert_eq!(app.ai_commit.state, AiCommitState::Applied);

    // Not happy with it — ask again. The second run starts over generated text.
    app.commit_generate();
    let second = app
        .ai_commit
        .generating()
        .expect("a second run is in flight")
        .0;
    app.on_commit_message_generated(Some(second), "fix: second attempt".to_string());

    assert_eq!(app.commit_input.text, "fix: second attempt");
    // Undo must still reach the user's words, not the first generated message —
    // regenerating is not consent to lose what they wrote.
    assert!(app.commit_generate_undo());
    assert_eq!(app.commit_input.text, "fix the parser thing");
}

/// Drive `app` to the state after one generation replaced the user's words.
fn applied_over(app: &mut App, words: &str) {
    app.backend = Some(std::sync::Arc::new(RecordingBackend::new()));
    app.on_ai_commit_availability(ready());
    let first = generating(app, "");
    app.commit_input.text = words.to_string();
    app.on_commit_message_generated(Some(first), "fix: generated".to_string());
    assert_eq!(app.ai_commit.state, AiCommitState::Applied);
}

#[test]
fn cancelling_a_second_run_keeps_the_undo() {
    let mut app = app();
    applied_over(&mut app, "my words");
    app.commit_generate();
    assert!(app.commit_generate_cancel());
    assert!(
        app.commit_generate_undo(),
        "cancelling a re-run must not spend the undo"
    );
    assert_eq!(app.commit_input.text, "my words");
}

#[test]
fn a_failed_second_run_keeps_the_undo() {
    let mut app = app();
    applied_over(&mut app, "my words");
    app.commit_generate();
    let second = app.ai_commit.generating().expect("in flight").0;
    app.on_commit_message_failed(Some(second), "the agent timed out".to_string());
    assert!(
        app.commit_generate_undo(),
        "a failed re-run must not spend the undo"
    );
    assert_eq!(app.commit_input.text, "my words");
}

#[test]
fn a_refused_second_run_keeps_the_undo() {
    let mut app = app();
    applied_over(&mut app, "my words");
    // The agent disappears between the two presses.
    let mut gone = ready();
    gone.agents = vec![AiCommitAgentStatus {
        agent: AiCommitAgent::Claude,
        available: false,
        detail: "`claude` was not found on PATH".to_string(),
    }];
    app.on_ai_commit_availability(gone);
    app.commit_generate();
    assert!(
        app.commit_generate_undo(),
        "a refusal must not spend the undo"
    );
    assert_eq!(app.commit_input.text, "my words");
}

#[test]
fn committing_disarms_the_undo() {
    let mut app = app();
    applied_over(&mut app, "my words");
    app.on_committed("0123456789abcdef");
    assert_eq!(app.commit_input.text, "", "the box is cleared to commit");
    assert!(
        !app.commit_generate_undo(),
        "the draft belonged to a commit that has already landed"
    );
    assert_eq!(app.commit_input.text, "");
}

#[test]
fn a_failure_reaches_the_notification_stack_as_well_as_the_chip() {
    let mut app = app();
    let request = generating(&mut app, "wip");
    // git reports failures as multi-line stderr; the chip is one row in a
    // border, so the detail has to survive somewhere it can actually be read.
    app.on_commit_message_failed(
        Some(request),
        "fatal: bad object HEAD\n\nfix it".to_string(),
    );

    let (severity, status) = last_report(&app).expect("a failure reaches the notification stack");
    assert_eq!(severity, Severity::Error, "{status}");
    assert!(status.contains("bad object HEAD"), "{status}");
    assert!(!status.contains('\n'), "collapsed to one line: {status:?}");
    let AiCommitState::Failed { reason } = &app.ai_commit.state else {
        panic!("expected a failed state");
    };
    assert!(!reason.contains('\n'), "collapsed to one line: {reason:?}");
}

#[test]
fn a_superseded_answer_cannot_overwrite_the_box() {
    let mut app = app();
    let current = generating(&mut app, "wip");
    let stale = RequestId(current.0 + 1);

    app.on_commit_message_generated(Some(stale), "from an abandoned run".to_string());
    assert_eq!(app.commit_input.text, "wip", "the stale answer is dropped");

    // An unattributed answer is equally untrustworthy while a run is in flight.
    app.on_commit_message_generated(None, "unattributed".to_string());
    assert_eq!(app.commit_input.text, "wip");

    // The answer to the run actually in flight still applies.
    app.on_commit_message_generated(Some(current), "the real one".to_string());
    assert_eq!(app.commit_input.text, "the real one");
}

#[test]
fn an_answer_arriving_after_a_cancel_is_ignored() {
    let mut app = app();
    let request = generating(&mut app, "wip");
    assert!(app.commit_generate_cancel());
    assert_eq!(app.ai_commit.state, AiCommitState::Idle);

    // The process may still have been mid-flight when it was killed.
    app.on_commit_message_generated(Some(request), "too late".to_string());
    assert_eq!(app.commit_input.text, "wip", "the draft is untouched");
}

#[test]
fn escape_stops_a_generation_before_it_blurs_the_field() {
    let mut app = app();
    app.commit_input.focused = true;
    generating(&mut app, "wip");

    app.commit_cancel();
    assert!(
        app.commit_input.focused,
        "the first Esc reaches the run, not the focus"
    );
    assert_eq!(app.ai_commit.state, AiCommitState::Idle);

    // With nothing running, Esc goes back to meaning "leave the field".
    app.commit_cancel();
    assert!(!app.commit_input.focused);
    assert_eq!(app.commit_input.text, "wip", "the draft is kept either way");
}

#[test]
fn a_known_blocker_is_reported_without_asking_the_backend() {
    let mut app = app();
    let mut status = ready();
    status.agents = vec![AiCommitAgentStatus {
        agent: AiCommitAgent::Claude,
        available: false,
        detail: "`claude` was not found on PATH".to_string(),
    }];
    app.on_ai_commit_availability(status);

    app.commit_generate();
    assert_eq!(
        app.ai_commit.state,
        AiCommitState::Failed {
            reason: "`claude` was not found on PATH".to_string()
        },
        "the refusal is immediate, not a round-trip away"
    );
    assert!(app.ai_commit.generating().is_none());
}

#[test]
fn availability_drives_what_the_affordance_claims() {
    let mut app = app();
    // Before the backend answers, nothing is claimed either way: no blocker to
    // report, and no model to promise.
    assert!(app.ai_commit.availability.is_none());
    assert_eq!(app.ai_commit.blocker(), None);
    assert_eq!(app.ai_commit.model_label(), None);

    app.on_ai_commit_availability(ready());
    assert_eq!(
        app.ai_commit.blocker(),
        None,
        "a probed, enabled agent runs"
    );
    assert_eq!(app.ai_commit.model_label(), Some("auto"));

    let mut disabled = ready();
    disabled.enabled = false;
    app.on_ai_commit_availability(disabled);
    assert!(
        app.ai_commit
            .blocker()
            .is_some_and(|b| b.contains("disabled"))
    );
}

#[test]
fn a_failure_is_attributed_to_the_run_that_is_waiting() {
    let mut app = app();
    let request = generating(&mut app, "wip");

    // A failure from an abandoned run must not disturb the one still going.
    app.on_commit_message_failed(Some(RequestId(request.0 + 1)), "stale".to_string());
    assert!(app.ai_commit.generating().is_some());

    app.on_commit_message_failed(Some(request), "the agent timed out".to_string());
    assert_eq!(
        app.ai_commit.state,
        AiCommitState::Failed {
            reason: "the agent timed out".to_string()
        }
    );
    assert_eq!(app.commit_input.text, "wip", "a failure costs no draft");
}

#[test]
fn a_running_generation_schedules_its_own_repaints() {
    let mut app = app();
    let now = Instant::now();
    assert_eq!(app.ai_commit_next_wake(now), None, "idle needs no wake");

    // Just started: wake at the reveal threshold, so the spinner appears without
    // waiting for a keystroke.
    app.ai_commit.state = AiCommitState::Generating {
        request: RequestId(1),
        since: Pending::at(now),
        draft: String::new(),
    };
    let wake = app.ai_commit_next_wake(now).expect("a pending run wakes");
    assert!(wake <= LOADING_REVEAL_DELAY && !wake.is_zero(), "{wake:?}");

    // Past the threshold: keep waking a frame at a time so it animates.
    let revealed = now - LOADING_REVEAL_DELAY - Duration::from_millis(1);
    app.ai_commit.state = AiCommitState::Generating {
        request: RequestId(1),
        since: Pending::at(revealed),
        draft: String::new(),
    };
    assert_eq!(
        app.ai_commit_next_wake(now),
        Some(karet_widgets::Spinner::FRAME_INTERVAL)
    );
}
