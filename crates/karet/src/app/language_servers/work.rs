//! The client half of what a language server says to the user: the work it
//! reports, the questions it asks, and its log.
//!
//! Work is tracked per `(server, root, token)` from its `Begin` to its `End`,
//! and also dropped when the server's connection is: an `End` never comes for
//! work a dead server was doing. The status bar shows the newest work of the
//! servers covering the active file, and the badge reads `busy` while there is
//! any.
//!
//! A question goes to the confirmation dialog, with "Dismiss" -- no action --
//! first, so every way of backing out answers the server. One that arrives
//! while another dialog is up waits for it rather than displacing it, because
//! displacing a dialog declines it, and a server must not be able to cancel
//! something the user was in the middle of deciding.

use karet_session::LanguageServerWork;
use karet_session::WorkPhase;
use karet_session::WorkToken;

use super::*;
use crate::app::util::path_under;

/// The longest the status bar's work text runs before it is cut.
const WORK_LABEL_CHARS: usize = 48;

/// One piece of work a server reported beginning and has not reported ending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkEntry {
    /// The provider doing it.
    pub(crate) server: LanguageServerId,
    /// The root its connection serves.
    pub(crate) root: PathBuf,
    /// The server's name for it.
    pub(crate) token: WorkToken,
    /// What it is, from its `Begin`.
    pub(crate) title: String,
    /// What it is doing now.
    pub(crate) message: Option<String>,
    /// How far along.
    pub(crate) percentage: Option<u32>,
    /// Whether the server allows cancelling it.
    pub(crate) cancellable: bool,
}

impl WorkEntry {
    /// The status-bar text: `rust-analyzer: Indexing 3/10 40%`, cut to fit.
    pub(crate) fn label(&self) -> String {
        let mut label = format!("{}: {}", self.server.display_name(), self.title);
        if let Some(message) = self.message.as_deref().filter(|m| !m.is_empty()) {
            label.push(' ');
            label.push_str(message);
        }
        if let Some(percentage) = self.percentage {
            label.push_str(&format!(" {percentage}%"));
        }
        match label.char_indices().nth(WORK_LABEL_CHARS) {
            Some((cut, _)) => format!("{}\u{2026}", &label[..cut]),
            None => label,
        }
    }
}

/// A server's question, waiting for the dialog to be free.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Question {
    /// The id to answer it by.
    pub(crate) request: LanguageServerMessageId,
    /// The provider asking.
    pub(crate) server: LanguageServerId,
    /// How urgent.
    pub(crate) severity: Severity,
    /// The question.
    pub(crate) message: String,
    /// The actions offered.
    pub(crate) actions: Vec<String>,
}

/// Work in flight and questions waiting, for every server.
#[derive(Default)]
pub(crate) struct WindowModel {
    /// Oldest first, so the newest is last.
    work: Vec<WorkEntry>,
    questions: VecDeque<Question>,
}

impl WindowModel {
    /// Adopt one progress report.
    pub(crate) fn apply(&mut self, report: LanguageServerWork) {
        let at = self.work.iter().position(|entry| {
            entry.server == report.server
                && entry.root == report.root
                && entry.token == report.token
        });
        match (report.phase, at) {
            (WorkPhase::End, Some(at)) => {
                self.work.remove(at);
            },
            (WorkPhase::End, None) => {},
            (WorkPhase::Begin, at) => {
                if let Some(at) = at {
                    self.work.remove(at);
                }
                self.work.push(WorkEntry {
                    server: report.server,
                    root: report.root,
                    token: report.token,
                    title: report.title.unwrap_or_default(),
                    message: report.message,
                    percentage: report.percentage,
                    cancellable: report.cancellable.unwrap_or(false),
                });
            },
            // A report for work whose `Begin` was missed is still work: it is
            // shown untitled rather than dropped.
            (WorkPhase::Report, None) => self.work.push(WorkEntry {
                server: report.server,
                root: report.root,
                token: report.token,
                title: String::new(),
                message: report.message,
                percentage: report.percentage,
                cancellable: report.cancellable.unwrap_or(false),
            }),
            (WorkPhase::Report, Some(at)) => {
                let entry = &mut self.work[at];
                if report.message.is_some() {
                    entry.message = report.message;
                }
                if report.percentage.is_some() {
                    entry.percentage = report.percentage;
                }
                if let Some(cancellable) = report.cancellable {
                    entry.cancellable = cancellable;
                }
            },
        }
    }

    /// Drop everything `server` was doing at `root`: its connection is gone.
    pub(crate) fn clear(&mut self, server: &LanguageServerId, root: &Path) {
        self.work
            .retain(|entry| !(entry.server == *server && entry.root == root));
    }

    /// The newest work of a server covering `path`, opened as `language`.
    pub(crate) fn for_file(
        &self,
        servers: &[LanguageServerStatus],
        path: &Path,
        language: &str,
    ) -> Option<&WorkEntry> {
        self.work
            .iter()
            .rev()
            .find(|entry| covers(servers, &entry.server, language) && path_under(&entry.root, path))
    }

    /// Hold `question` until the dialog is free.
    pub(crate) fn wait(&mut self, question: Question) {
        self.questions.push_back(question);
    }

    /// The next question waiting, if any.
    pub(crate) fn next_question(&mut self) -> Option<Question> {
        self.questions.pop_front()
    }
}

/// Whether `server` is one of the providers for `language`.
fn covers(servers: &[LanguageServerStatus], server: &LanguageServerId, language: &str) -> bool {
    servers.iter().any(|status| {
        status.server == *server
            && status
                .languages
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(language))
    })
}

impl App {
    /// Adopt one progress report from the session.
    pub(in crate::app) fn on_language_server_work(&mut self, report: LanguageServerWork) {
        self.lsp_runtime.window.apply(report);
    }

    /// The work the status bar shows: the newest of the active file's servers.
    pub(crate) fn active_language_server_work(&self) -> Option<&WorkEntry> {
        let TabKind::Code { path, language, .. } = &self.tabs.get(self.active)?.kind else {
            return None;
        };
        self.lsp_runtime
            .window
            .for_file(&self.lsp_runtime.servers, path, language)
    }

    /// Ask the active file's server to cancel the work the status bar shows.
    pub(in crate::app) fn cancel_language_server_work(&mut self) {
        let Some(work) = self.active_language_server_work().cloned() else {
            self.notify(
                Report::Refusal,
                NotificationKind::Lsp,
                "no language-server work is running for this file",
            );
            return;
        };
        if !work.cancellable {
            self.notify(
                Report::Refusal,
                NotificationKind::Lsp,
                format!(
                    "{} does not allow cancelling this",
                    work.server.display_name()
                ),
            );
            return;
        }
        self.send_command(LanguageServerCommand::CancelWork {
            server: work.server,
            root: work.root,
            token: work.token,
        });
    }

    /// Put a server's question to the user, or hold it while another dialog
    /// is up.
    pub(in crate::app) fn on_language_server_question(&mut self, question: Question) {
        if self.confirm.is_some() {
            self.lsp_runtime.window.wait(question);
        } else {
            self.ask_language_server_question(question);
        }
    }

    /// Put the next held question, once the dialog is free.
    pub(in crate::app) fn ask_next_language_server_question(&mut self) {
        if self.confirm.is_none()
            && let Some(question) = self.lsp_runtime.window.next_question()
        {
            self.ask_language_server_question(question);
        }
    }

    fn ask_language_server_question(&mut self, question: Question) {
        let request = question.request;
        let mut choices = vec![ConfirmChoice::custom(
            "Dismiss",
            ConfirmAction::AnswerLanguageServerMessage {
                request,
                action: None,
            },
        )];
        choices.extend(
            question
                .actions
                .into_iter()
                .enumerate()
                .map(|(index, title)| {
                    ConfirmChoice::custom(
                        title,
                        ConfirmAction::AnswerLanguageServerMessage {
                            request,
                            action: Some(index),
                        },
                    )
                }),
        );
        let urgency = match question.severity {
            Severity::Error => " (error)",
            Severity::Warning => " (warning)",
            _ => "",
        };
        self.confirm = Some(ConfirmDialog::new(
            format!("{} asks{urgency}", question.server.display_name()),
            question.message,
            choices,
        ));
        self.context_menu = None;
    }

    /// Open the log of the active file's server: reserved at once, filled when
    /// the session answers.
    pub(in crate::app) fn show_language_server_log(&mut self) {
        let server = self
            .active_language_server_work()
            .map(|work| work.server.clone())
            .or_else(|| {
                let TabKind::Code { language, .. } = &self.tabs.get(self.active)?.kind else {
                    return None;
                };
                self.lsp_runtime
                    .servers
                    .iter()
                    .find(|status| {
                        status.enabled
                            && status
                                .languages
                                .iter()
                                .any(|candidate| candidate.eq_ignore_ascii_case(language))
                    })
                    .map(|status| status.server.clone())
            });
        let Some(server) = server else {
            self.notify(
                Report::Refusal,
                NotificationKind::Lsp,
                "open a file a language server serves to see its log",
            );
            return;
        };
        let existing = self.tabs.iter().position(
            |tab| matches!(&tab.kind, TabKind::ServerLog { server: shown, .. } if *shown == server),
        );
        match existing {
            Some(index) => {
                if let TabKind::ServerLog { loading_since, .. } = &mut self.tabs[index].kind {
                    *loading_since = Some(Pending::start());
                }
                self.select_tab(index);
            },
            None => self.push_tab(Tab::server_log(server.clone())),
        }
        self.send_command(LanguageServerCommand::Log { server });
    }

    /// Fill every open log of `server` with the session's answer.
    pub(in crate::app) fn on_language_server_log(
        &mut self,
        server: &LanguageServerId,
        log: Vec<LanguageServerLogEntry>,
    ) {
        for tab in self.all_tabs_mut() {
            if let TabKind::ServerLog {
                server: shown,
                entries,
                loading_since,
                ..
            } = &mut tab.kind
                && shown == server
            {
                entries.clone_from(&log);
                *loading_since = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(phase: WorkPhase, token: &str) -> LanguageServerWork {
        LanguageServerWork {
            server: LanguageServerId::RustAnalyzer,
            root: PathBuf::from("/w"),
            token: WorkToken::Text(token.to_owned()),
            phase,
            title: (phase == WorkPhase::Begin).then(|| "Indexing".to_owned()),
            message: None,
            percentage: None,
            cancellable: None,
        }
    }

    fn rust_status() -> LanguageServerStatus {
        LanguageServerStatus {
            server: LanguageServerId::RustAnalyzer,
            languages: vec!["rust".to_owned()],
            enabled: true,
            managed: true,
            manual_install_reason: None,
            installed: None,
            ever_installed: false,
            declined: false,
            cleanup_pending: false,
            instances: Vec::new(),
        }
    }

    #[test]
    fn work_runs_from_begin_through_reports_to_end() {
        let mut model = WindowModel::default();
        let servers = [rust_status()];
        let file = Path::new("/w/src/lib.rs");
        model.apply(report(WorkPhase::Begin, "a"));
        let mut progress = report(WorkPhase::Report, "a");
        progress.message = Some("3/10".to_owned());
        progress.percentage = Some(30);
        progress.cancellable = Some(true);
        model.apply(progress);
        let shown = model.for_file(&servers, file, "rust").cloned();
        assert_eq!(
            shown.as_ref().map(WorkEntry::label).as_deref(),
            Some("rust-analyzer: Indexing 3/10 30%")
        );
        assert_eq!(shown.map(|work| work.cancellable), Some(true));
        assert!(model.for_file(&servers, file, "python").is_none());
        assert!(
            model
                .for_file(&servers, Path::new("/elsewhere/a.rs"), "rust")
                .is_none()
        );
        model.apply(report(WorkPhase::End, "a"));
        assert!(model.for_file(&servers, file, "rust").is_none());
    }

    #[test]
    fn the_newest_work_shows_and_a_lost_connection_ends_all_of_it() {
        let mut model = WindowModel::default();
        let servers = [rust_status()];
        let file = Path::new("/w/src/lib.rs");
        model.apply(report(WorkPhase::Begin, "a"));
        let mut second = report(WorkPhase::Begin, "b");
        second.title = Some("Building".to_owned());
        model.apply(second);
        assert_eq!(
            model
                .for_file(&servers, file, "rust")
                .map(|work| work.title.as_str()),
            Some("Building")
        );
        // A report whose begin was missed is shown, not dropped.
        model.apply(report(WorkPhase::Report, "c"));
        assert!(
            model
                .for_file(&servers, file, "rust")
                .is_some_and(|work| work.title.is_empty())
        );
        model.clear(&LanguageServerId::RustAnalyzer, Path::new("/w"));
        assert!(model.for_file(&servers, file, "rust").is_none());
    }

    #[test]
    fn a_long_label_is_cut() {
        let entry = WorkEntry {
            server: LanguageServerId::RustAnalyzer,
            root: PathBuf::from("/w"),
            token: WorkToken::Number(1),
            title: "x".repeat(100),
            message: None,
            percentage: None,
            cancellable: false,
        };
        assert_eq!(entry.label().chars().count(), WORK_LABEL_CHARS + 1);
        assert!(entry.label().ends_with('\u{2026}'));
    }

    #[test]
    fn questions_wait_in_arrival_order() {
        let mut model = WindowModel::default();
        for id in [1, 2] {
            model.wait(Question {
                request: LanguageServerMessageId(id),
                server: LanguageServerId::RustAnalyzer,
                severity: Severity::Information,
                message: String::new(),
                actions: Vec::new(),
            });
        }
        assert_eq!(
            model.next_question().map(|q| q.request),
            Some(LanguageServerMessageId(1))
        );
        assert_eq!(
            model.next_question().map(|q| q.request),
            Some(LanguageServerMessageId(2))
        );
        assert!(model.next_question().is_none());
    }
}
