//! The AI commit-message options form.
//!
//! Everything a generation depends on is settled here, before one is asked for.
//! That is the point of the form: the round-trip costs seconds, so a
//! misconfiguration discovered by running one is seconds wasted and a failure
//! to explain, whereas the same fact shown here costs nothing.
//!
//! Two rules follow from it. An agent whose CLI was not found is still offered —
//! with what the probe said, so the user can see *why* rather than wondering
//! where it went — but selecting it is visibly a choice to fix something. And
//! the effort field only ever cycles through values the selected agent accepts,
//! because the adapters reject the extremes of the scale outright; an
//! unsupported pair is not a thing the user should be able to build here.

use karet_session::AiCommit;
use karet_session::AiCommitAgent;
use karet_session::AiCommitAvailability;
use karet_session::AiCommitEffort;

/// The editable AI commit-message options.
pub(crate) struct AiCommitForm {
    /// The options being edited.
    options: AiCommit,
    /// The availability the form was opened with, for per-agent annotations.
    availability: Option<AiCommitAvailability>,
    /// The efforts the selected agent accepts, plus "model default" at index 0.
    efforts: Vec<Option<AiCommitEffort>>,
    /// The selected row.
    selected: usize,
    /// Rendered rows, kept in step with the options.
    rows: Vec<String>,
}

/// The last row index — the form's fields are a fixed list.
const LAST_ROW: usize = 5;

impl AiCommitForm {
    /// Build the form over the current options and what the backend last probed.
    pub(crate) fn new(options: AiCommit, availability: Option<AiCommitAvailability>) -> Self {
        let mut form = Self {
            options,
            availability,
            efforts: Vec::new(),
            selected: 0,
            rows: Vec::new(),
        };
        form.rebuild_efforts();
        form.refresh();
        form
    }

    /// Recompute the effort cycle for the selected agent.
    ///
    /// Called whenever the agent changes, which is also when a previously valid
    /// effort can become invalid — so a pinned effort the new agent rejects is
    /// dropped here rather than surviving into a request that would fail.
    fn rebuild_efforts(&mut self) {
        self.efforts = std::iter::once(None)
            .chain(self.options.agent.supported_efforts().into_iter().map(Some))
            .collect();
        if !self.efforts.contains(&self.options.effort) {
            self.options.effort = None;
        }
    }

    /// What the probe said about `agent`, for the agent row.
    fn agent_note(&self, agent: AiCommitAgent) -> String {
        let Some(status) = self
            .availability
            .as_ref()
            .and_then(|a| a.agents.iter().find(|s| s.agent == agent))
        else {
            return "not probed".to_string();
        };
        if status.available {
            format!("✓ {}", status.detail)
        } else {
            format!("✗ {}", status.detail)
        }
    }

    fn refresh(&mut self) {
        let effort = self
            .options
            .effort
            .map_or("model default".to_string(), |e| e.as_str().to_string());
        let model = if self.options.is_auto_model() {
            "auto (by diff size)".to_string()
        } else {
            self.options.model.clone()
        };
        let binary = self
            .options
            .binary
            .clone()
            .unwrap_or_else(|| format!("{} (on PATH)", self.options.agent.default_binary()));
        self.rows = vec![
            format!("Enabled           {}", yes_no(self.options.enabled)),
            format!(
                "Agent             {}    {}",
                self.options.agent.as_str(),
                self.agent_note(self.options.agent)
            ),
            format!("Model             {model}"),
            format!("Effort            {effort}"),
            format!("Timeout           {}s", self.options.timeout_ms / 1_000),
            format!("Binary            {binary}"),
        ];
    }

    /// Advance the agent, clearing the fields that belonged to the old one.
    fn cycle_agent(&mut self) {
        let agents = AiCommitAgent::ALL;
        let next = agents
            .iter()
            .position(|a| *a == self.options.agent)
            .map_or(0, |index| (index + 1) % agents.len());
        self.options.agent = agents[next];
        // A binary path is an override for one agent's executable; carrying it
        // across would point the new agent at the old one's program.
        self.options.binary = None;
        self.rebuild_efforts();
    }

    /// Advance the effort through the values this agent accepts.
    fn cycle_effort(&mut self) {
        let next = self
            .efforts
            .iter()
            .position(|e| *e == self.options.effort)
            .map_or(0, |index| (index + 1) % self.efforts.len());
        self.options.effort = self.efforts[next];
    }

    /// Step the timeout through a set of sane waits.
    fn cycle_timeout(&mut self) {
        const CHOICES: [u64; 5] = [15_000, 30_000, 60_000, 120_000, 300_000];
        let next = CHOICES
            .iter()
            .position(|ms| *ms == self.options.timeout_ms)
            .map_or(0, |index| (index + 1) % CHOICES.len());
        self.options.timeout_ms = CHOICES[next];
    }

    pub(super) fn rows(&self) -> &[String] {
        &self.rows
    }

    pub(super) fn selected(&self) -> usize {
        self.selected
    }

    pub(super) fn select_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub(super) fn select_down(&mut self) {
        self.selected = (self.selected + 1).min(LAST_ROW);
    }

    /// Type into the selected row, or cycle it when the key is a space.
    pub(super) fn push_char(&mut self, c: char) {
        match self.selected {
            0 if c == ' ' => self.options.enabled = !self.options.enabled,
            1 if c == ' ' => self.cycle_agent(),
            2 => self.options.model.push(c),
            3 if c == ' ' => self.cycle_effort(),
            4 if c == ' ' => self.cycle_timeout(),
            5 => self.options.binary.get_or_insert_with(String::new).push(c),
            _ => {},
        }
        self.refresh();
    }

    pub(super) fn pop_char(&mut self) {
        match self.selected {
            2 => {
                self.options.model.pop();
            },
            5 => {
                if let Some(binary) = self.options.binary.as_mut() {
                    binary.pop();
                    if binary.is_empty() {
                        self.options.binary = None;
                    }
                }
            },
            _ => {},
        }
        self.refresh();
    }

    /// The options to persist.
    ///
    /// An emptied model box means "let the diff decide" rather than "run a model
    /// with no name", which is the only reading that produces a working request.
    pub(super) fn options(&self) -> AiCommit {
        let mut options = self.options.clone();
        if options.model.trim().is_empty() {
            options.model = karet_session::AI_COMMIT_AUTO_MODEL.to_string();
        }
        options
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

#[cfg(test)]
mod tests {
    use karet_session::AiCommitAgentStatus;

    use super::*;

    fn availability() -> AiCommitAvailability {
        AiCommitAvailability {
            supported: true,
            enabled: true,
            options: AiCommit::default(),
            agents: vec![
                AiCommitAgentStatus {
                    agent: AiCommitAgent::Claude,
                    available: true,
                    detail: "claude 2.1".to_string(),
                },
                AiCommitAgentStatus {
                    agent: AiCommitAgent::Codex,
                    available: false,
                    detail: "`codex` was not found on PATH".to_string(),
                },
            ],
            effort_conflict: None,
        }
    }

    #[test]
    fn the_agent_row_shows_what_the_probe_found() {
        let form = AiCommitForm::new(AiCommit::default(), Some(availability()));
        assert!(form.rows()[1].contains("claude"), "{:?}", form.rows());
        assert!(form.rows()[1].contains("✓"), "installed is marked");

        // An unavailable agent is still selectable, but says why it will not run —
        // which is the whole reason to show it rather than hide it.
        let mut form = form;
        form.selected = 1;
        form.push_char(' ');
        assert!(form.rows()[1].contains("codex"));
        assert!(
            form.rows()[1].contains("not found on PATH"),
            "{:?}",
            form.rows()
        );
    }

    #[test]
    fn effort_only_ever_cycles_through_what_the_agent_accepts() {
        let mut form = AiCommitForm::new(AiCommit::default(), None);
        form.selected = 3;
        // Claude rejects `minimal`, so it must never appear in the cycle.
        let mut seen = Vec::new();
        for _ in 0..form.efforts.len() {
            form.push_char(' ');
            seen.push(form.options.effort);
        }
        assert!(!seen.contains(&Some(AiCommitEffort::Minimal)));
        assert!(seen.contains(&Some(AiCommitEffort::Max)));
        assert!(seen.contains(&None), "the model default stays reachable");
    }

    #[test]
    fn switching_agent_drops_an_effort_the_new_agent_rejects() {
        let options = AiCommit {
            agent: AiCommitAgent::Claude,
            effort: Some(AiCommitEffort::Max),
            binary: Some("/opt/bin/claude".to_string()),
            ..AiCommit::default()
        };
        let mut form = AiCommitForm::new(options, None);
        assert_eq!(form.options.effort, Some(AiCommitEffort::Max));

        form.selected = 1;
        form.push_char(' ');
        assert_eq!(form.options.agent, AiCommitAgent::Codex);
        // Codex rejects `max`: keeping it would build a request that cannot run.
        assert_eq!(form.options.effort, None);
        // And the binary belonged to claude, so it must not point codex at it.
        assert_eq!(form.options.binary, None);
    }

    #[test]
    fn an_emptied_model_means_auto_rather_than_nameless() {
        let mut form = AiCommitForm::new(AiCommit::default(), None);
        form.selected = 2;
        for _ in 0..16 {
            form.pop_char();
        }
        assert!(form.options.model.is_empty(), "the box really is empty");
        assert!(form.options().is_auto_model(), "but the request is not");
    }

    #[test]
    fn typing_edits_only_the_free_text_rows() {
        let mut form = AiCommitForm::new(AiCommit::default(), None);
        let before = form.options.clone();
        // Row 0 is a toggle: an ordinary character must not corrupt it.
        form.selected = 0;
        form.push_char('x');
        assert_eq!(form.options.enabled, before.enabled);
        // Space toggles it.
        form.push_char(' ');
        assert_ne!(form.options.enabled, before.enabled);

        form.selected = 5;
        form.push_char('/');
        form.push_char('x');
        assert_eq!(form.options.binary.as_deref(), Some("/x"));
        form.pop_char();
        form.pop_char();
        assert_eq!(form.options.binary, None, "emptied is unset, not blank");
    }

    #[test]
    fn selection_stays_within_the_rows() {
        let mut form = AiCommitForm::new(AiCommit::default(), None);
        for _ in 0..20 {
            form.select_down();
        }
        assert_eq!(form.selected(), LAST_ROW);
        assert_eq!(form.rows().len(), LAST_ROW + 1, "every row is reachable");
        for _ in 0..20 {
            form.select_up();
        }
        assert_eq!(form.selected(), 0);
    }
}
