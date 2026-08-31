//! Asking about a language server, and reporting what the answer set going.
//!
//! Three questions live here, not one: a provider that is missing, a provider
//! that is missing *and* switched off, and an update plan whose exact versions
//! the user has to approve. They differ in what saying yes costs, so they differ
//! in what they say — and in all three the first answer, the one every way of
//! backing out takes, is the one that spends no bandwidth.

use super::*;

impl App {
    /// Offer to install a missing provider.
    ///
    /// Installing spends the user's bandwidth on a download they did not ask for,
    /// so the first row — the one Enter takes and the one every way of backing out
    /// runs — declines. `Never ask` sits alongside it so a user who does not want
    /// this provider can say so once, instead of dismissing the same prompt on
    /// every launch.
    ///
    /// A provider *disabled* for the language is a different question: it has to
    /// be turned on as well as fetched, so saying yes changes more than the disk.
    pub(in crate::app) fn prompt_language_server_install(
        &mut self,
        server: LanguageServerId,
        language: &str,
        enabled: bool,
    ) {
        let name = server.display_name().to_string();
        let (title, body, proceed) = if enabled {
            (
                format!("Install {name}?"),
                format!(
                    "Karet has no language server for {language}. Installing \
                     downloads {name} from its upstream release and activates it \
                     for this and future sessions."
                ),
                format!("Install {name}"),
            )
        } else {
            (
                format!("Enable and install {name}?"),
                format!(
                    "{name} is turned off for {language} in your settings, and is \
                     not installed. Continuing downloads it and enables it for \
                     {language}."
                ),
                format!("Enable and install {name}"),
            )
        };
        self.confirm(ConfirmDialog::new(
            title,
            body,
            vec![
                ConfirmChoice::custom("Not now", ConfirmAction::Cancel),
                ConfirmChoice::custom(
                    proceed,
                    ConfirmAction::InstallLanguageServer(server.clone()),
                ),
                ConfirmChoice::custom(
                    format!("Never ask about {name}"),
                    ConfirmAction::DeclineLanguageServer(server),
                ),
            ],
        ));
    }

    /// Say why a missing provider is the user's to install.
    ///
    /// A notification, not a dialog, because there is nothing to decide: karet
    /// cannot fetch these, so a prompt would only offer an action that fails.
    /// It is tagged per provider so opening ten Go files states the condition
    /// once, and it names the executable karet looked for, since "install
    /// gopls" is not actionable until you know what has to end up on `PATH`.
    pub(in crate::app) fn report_manual_language_server(
        &mut self,
        server: &LanguageServerId,
        command: &str,
        reason: &str,
    ) {
        let name = server.display_name();
        self.notify_tagged(
            Report::Alert,
            NotificationKind::Lsp,
            format!(
                "No language server for {name}: it {reason}. Install it yourself so \
                 '{command}' is on PATH, or point lsp.servers.{} at it.",
                server.key()
            ),
            Some(format!("lsp-manual-install:{}", server.key())),
        );
    }

    /// Forget the selected provider's recorded refusal, so it is offered again.
    ///
    /// The counterpart to *Never ask*: a refusal the user cannot take back is a
    /// setting they cannot find, and this one lives outside settings by design.
    pub(in crate::app) fn undecline_selected_language_server(&mut self) {
        let Some(status) = self.selected_language_server() else {
            return;
        };
        let name = status.server.display_name().to_string();
        if !status.declined {
            self.notify(
                Report::Refusal,
                NotificationKind::Lsp,
                format!("{name} was not declined"),
            );
            return;
        }
        self.send_command(SessionCommand::UndeclineLanguageServer {
            server: status.server,
        });
        self.notify(
            Report::Outcome,
            NotificationKind::Lsp,
            format!("{name} will be offered again when a file needs it"),
        );
        self.refresh_language_servers();
    }

    /// Record that the user does not want this provider offered again.
    pub(in crate::app) fn decline_language_server(&mut self, server: LanguageServerId) {
        let name = server.display_name().to_string();
        self.send_command(SessionCommand::DeclineLanguageServer { server });
        self.notify(
            Report::Outcome,
            NotificationKind::Lsp,
            format!("{name} will not be offered again · press o in Language Servers to undo"),
        );
    }
}
