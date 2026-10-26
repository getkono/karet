//! Signing in to the account a language server proposes for: GitHub, for
//! `copilot-language-server`, whose inline completions need it.
//!
//! Copilot's flow is its own, not LSP's: a `signIn` request answers a device
//! code for the user to confirm in a browser, and the `command` it returns --
//! run through `workspace/executeCommand` -- opens that browser and waits for
//! the confirmation. The wait outlasts any request timeout, and the server
//! reports the outcome through its own status notifications regardless, so
//! the command is run on a task of its own and its answer only logged: the
//! user is handed the code at once. A server too old to return a command is
//! sent `signInConfirm` with the code instead, which is what it waits on.

use std::sync::Arc;

use karet_lsp::LspClient;
use serde_json::Value;
use serde_json::json;

use crate::api::AccountOutcome;

/// Run the server's sign-in (`sign_in`) or sign-out flow, answering what the
/// user is to do or what came of it.
pub(super) async fn account(client: &Arc<LspClient>, sign_in: bool) -> AccountOutcome {
    let method = if sign_in { "signIn" } else { "signOut" };
    let reply = client.custom_request::<_, Value>(method, json!({})).await;
    match reply {
        Err(error) => AccountOutcome::Failed {
            message: error.to_string(),
        },
        Ok(_) if !sign_in => AccountOutcome::SignedOut,
        Ok(reply) => {
            let (outcome, follow_up) = read_sign_in(&reply);
            if let Some((method, params)) = follow_up {
                let client = Arc::clone(client);
                tokio::spawn(async move {
                    if let Err(error) = client.custom_request::<_, Value>(method, params).await {
                        tracing::debug!(%error, method, "sign-in follow-up unanswered");
                    }
                });
            }
            outcome
        },
    }
}

/// What a `signIn` reply asks of the user, and the request that completes it
/// once they have: the `command` it returned, or `signInConfirm` for a server
/// that returned none.
fn read_sign_in(reply: &Value) -> (AccountOutcome, Option<(&'static str, Value)>) {
    let text = |key: &str| reply.get(key).and_then(Value::as_str).map(str::to_owned);
    if let Some(user_code) = text("userCode") {
        let follow_up = match reply.get("command") {
            Some(command) if command.get("command").is_some() => (
                "workspace/executeCommand",
                json!({
                    "command": command["command"],
                    "arguments": command.get("arguments").cloned().unwrap_or(json!([])),
                }),
            ),
            _ => ("signInConfirm", json!({ "userCode": user_code })),
        };
        let outcome = AccountOutcome::DeviceCode {
            user_code,
            verification_uri: text("verificationUri"),
        };
        return (outcome, Some(follow_up));
    }
    match text("status").as_deref() {
        Some("AlreadySignedIn" | "OK" | "MaybeOk") => {
            (AccountOutcome::SignedIn { user: text("user") }, None)
        },
        status => (
            AccountOutcome::Failed {
                message: format!(
                    "sign-in was not offered (status {})",
                    status.unwrap_or("missing")
                ),
            },
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Today's Copilot: a code to confirm, and the command that opens the
    /// browser and waits for it, run with its own arguments.
    #[test]
    fn a_device_code_comes_with_the_command_that_completes_it() {
        let (outcome, follow_up) = read_sign_in(&json!({
            "userCode": "ABCD-EFGH",
            "verificationUri": "https://github.com/login/device",
            "command": {"command": "github.copilot.finishDeviceFlow", "arguments": [], "title": "Sign in"}
        }));
        assert_eq!(
            outcome,
            AccountOutcome::DeviceCode {
                user_code: "ABCD-EFGH".to_owned(),
                verification_uri: Some("https://github.com/login/device".to_owned()),
            }
        );
        assert_eq!(
            follow_up,
            Some((
                "workspace/executeCommand",
                json!({"command": "github.copilot.finishDeviceFlow", "arguments": []})
            ))
        );
    }

    /// An older server returns no command: it waits on `signInConfirm`.
    #[test]
    fn without_a_command_the_code_is_confirmed_by_request() {
        let (_, follow_up) = read_sign_in(&json!({"userCode": "WXYZ-1234"}));
        assert_eq!(
            follow_up,
            Some(("signInConfirm", json!({"userCode": "WXYZ-1234"})))
        );
    }

    #[test]
    fn an_account_already_signed_in_says_whose_and_anything_else_fails() {
        let (signed_in, follow_up) =
            read_sign_in(&json!({"status": "AlreadySignedIn", "user": "octocat"}));
        assert_eq!(
            signed_in,
            AccountOutcome::SignedIn {
                user: Some("octocat".to_owned()),
            }
        );
        assert_eq!(follow_up, None);
        let (refused, _) = read_sign_in(&json!({"status": "NotAuthorized"}));
        assert!(matches!(
            refused,
            AccountOutcome::Failed { message } if message.contains("NotAuthorized")
        ));
    }
}
