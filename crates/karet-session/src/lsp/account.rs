//! Signing in to the account a language server proposes for: GitHub, for
//! `copilot-language-server`, whose inline completions need it.
//!
//! Copilot's flow is its own, not LSP's: a `signIn` request answers a device
//! code and the page to enter it at, and the `command` it returns -- run
//! through `workspace/executeCommand` -- waits for the user to confirm it
//! there. karet opens no browser: the server's `window/showDocument` is
//! refused (karet-lsp declares no support for it), so the user is handed the
//! code and the page and opens it themselves. The wait outlasts any request
//! timeout, so the command is run on a task of its own and the user is handed
//! the code at once; what the command answers is reported as a second
//! outcome of the same sign-in -- signed in, or failed and why. A server too
//! old to return a command is sent `signInConfirm` with the code instead,
//! which is what it waits on.

use std::sync::Arc;

use karet_lsp::LspClient;
use serde_json::Value;
use serde_json::json;

use crate::api::AccountOutcome;

/// Run the server's sign-in (`sign_in`) or sign-out flow, handing `report`
/// what the user is to do or what came of it. A device code's completing
/// request then runs on a task of its own, and `report` is handed what came
/// of that too, after the code.
pub(super) async fn account(
    client: &Arc<LspClient>,
    sign_in: bool,
    report: impl Fn(AccountOutcome) + Send + 'static,
) {
    let method = if sign_in { "signIn" } else { "signOut" };
    let reply = client.custom_request::<_, Value>(method, json!({})).await;
    let (outcome, follow_up) = match reply {
        Err(error) => {
            let message = error.to_string();
            (AccountOutcome::Failed { message }, None)
        },
        Ok(_) if !sign_in => (AccountOutcome::SignedOut, None),
        Ok(reply) => read_sign_in(&reply),
    };
    report(outcome);
    if let Some((method, params)) = follow_up {
        let client = Arc::clone(client);
        tokio::spawn(async move {
            let reply = client.custom_request::<_, Value>(method, params).await;
            report(read_completion(reply.map_err(|error| error.to_string())));
        });
    }
}

/// What came of a device code's completing request: a reply naming a status
/// is read as a `signIn` status is, and any other reply means the request
/// finished, so the account is signed in.
fn read_completion(reply: Result<Value, String>) -> AccountOutcome {
    match reply {
        Err(message) => AccountOutcome::Failed { message },
        Ok(reply) if reply.get("status").is_some() => read_sign_in(&reply).0,
        Ok(reply) => AccountOutcome::SignedIn {
            user: reply.get("user").and_then(Value::as_str).map(str::to_owned),
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

    /// Today's Copilot: a code to confirm, and the command that waits for it,
    /// run with its own arguments.
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

    /// The completing request's answer is the sign-in's last word: a status
    /// is read as `signIn`'s is, a bare reply means it finished, and an error
    /// is the failure the user is told.
    #[test]
    fn the_completing_request_says_whether_the_sign_in_landed() {
        assert_eq!(
            read_completion(Ok(json!({"status": "OK", "user": "octocat"}))),
            AccountOutcome::SignedIn {
                user: Some("octocat".to_owned()),
            }
        );
        assert_eq!(
            read_completion(Ok(Value::Null)),
            AccountOutcome::SignedIn { user: None }
        );
        assert!(matches!(
            read_completion(Ok(json!({"status": "NotAuthorized"}))),
            AccountOutcome::Failed { message } if message.contains("NotAuthorized")
        ));
        assert_eq!(
            read_completion(Err("device code expired".to_owned())),
            AccountOutcome::Failed {
                message: "device code expired".to_owned(),
            }
        );
    }
}
