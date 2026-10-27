//! Signing in to the account a language server proposes for: GitHub, for
//! `copilot-language-server`, whose inline completions need it.
//!
//! Copilot's flow is its own, not LSP's: a `signIn` request answers a device
//! code and the page to enter it at, and the `command` it returns -- run
//! through `workspace/executeCommand` -- waits for the user to confirm it
//! there. karet opens no browser: the server's `window/showDocument` is
//! refused (karet-lsp declares no support for it), so the user is handed the
//! code and the page and opens it themselves. The wait outlasts the standard
//! request timeout, so the command is bounded by how long the device code
//! lasts instead, and is run on a task of its own so the user is handed the
//! code at once; what the command answers is reported as a second
//! outcome of the same sign-in -- signed in, or failed and why. A server too
//! old to return a command is sent `signInConfirm` with the code instead,
//! which is what it waits on.
//!
//! The completing request can wait for minutes, so the user may retry the
//! sign-in, or sign out, while it does. Each server task holds its sign-in's
//! running completing request in a [`FollowUpTask`], and a new sign-in or
//! sign-out aborts it first: the abandoned flow never reports, so its late
//! "signed in" or "sign-in failed" cannot land over the flow the user is in.

use std::sync::Arc;
use std::time::Duration;

use karet_lsp::LspClient;
use serde_json::Value;
use serde_json::json;
use tokio::task::AbortHandle;

use crate::api::AccountOutcome;

/// How long a device code lasts when the `signIn` reply does not say:
/// GitHub's device flow issues codes that expire after fifteen minutes.
const DEVICE_CODE_LIFETIME: Duration = Duration::from_secs(15 * 60);

/// The longest a completing request is waited on, whatever the reply claims,
/// so a server naming an absurd lifetime cannot hold the task forever.
const MAX_DEVICE_CODE_LIFETIME: Duration = Duration::from_secs(60 * 60);

/// Waited past the code's lifetime, so the server's own "expired" answer
/// arrives before karet's timeout and the user is told why.
const EXPIRY_GRACE: Duration = Duration::from_secs(30);

/// A device code's completing request: the method, its params, and how long
/// to wait for the user to confirm the code.
type FollowUp = (&'static str, Value, Duration);

/// The completing request of a server's last sign-in, while it runs: one per
/// server task, superseded -- aborted, so it reports nothing -- when the next
/// sign-in or sign-out starts, or when the server task ends.
#[derive(Debug, Default)]
pub(super) struct FollowUpTask(Option<AbortHandle>);

impl FollowUpTask {
    /// Abort the running completing request, if any. A request whose reply
    /// was already being reported when this runs still reports it -- abort
    /// takes effect at the task's next await -- but that outcome was settled
    /// before the new flow began.
    fn supersede(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

impl Drop for FollowUpTask {
    fn drop(&mut self) {
        self.supersede();
    }
}

/// Run the server's sign-in (`sign_in`) or sign-out flow, handing `report`
/// what the user is to do or what came of it. The completing request of an
/// earlier sign-in still held in `follow_up_task` is aborted first, so it
/// reports nothing. A device code's completing request then runs on a task
/// of its own, held there in turn, and `report` is handed what came of that too,
/// after the code.
pub(super) async fn account(
    client: &Arc<LspClient>,
    sign_in: bool,
    follow_up_task: &mut FollowUpTask,
    report: impl Fn(AccountOutcome) + Send + 'static,
) {
    follow_up_task.supersede();
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
    if let Some((method, params, timeout)) = follow_up {
        let client = Arc::clone(client);
        let task = tokio::spawn(async move {
            let reply = client
                .custom_request_with::<_, Value>(method, params, timeout)
                .await;
            report(read_completion(reply.map_err(|error| error.to_string())));
        });
        follow_up_task.0 = Some(task.abort_handle());
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
/// that returned none, waited on for as long as the code lasts.
fn read_sign_in(reply: &Value) -> (AccountOutcome, Option<FollowUp>) {
    let text = |key: &str| reply.get(key).and_then(Value::as_str).map(str::to_owned);
    if let Some(user_code) = text("userCode") {
        let (method, params) = match reply.get("command") {
            Some(command) if command.get("command").is_some() => (
                "workspace/executeCommand",
                json!({
                    "command": command["command"],
                    "arguments": command.get("arguments").cloned().unwrap_or(json!([])),
                }),
            ),
            _ => ("signInConfirm", json!({ "userCode": user_code })),
        };
        let follow_up = (method, params, code_lifetime(reply) + EXPIRY_GRACE);
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

/// How long the `signIn` reply's code lasts: its `expiresIn` seconds, capped
/// at [`MAX_DEVICE_CODE_LIFETIME`], or [`DEVICE_CODE_LIFETIME`] without one.
fn code_lifetime(reply: &Value) -> Duration {
    reply
        .get("expiresIn")
        .and_then(Value::as_u64)
        .map_or(DEVICE_CODE_LIFETIME, Duration::from_secs)
        .min(MAX_DEVICE_CODE_LIFETIME)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use karet_lsp::codec;

    use super::*;

    type Reader = tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>;
    type Writer = tokio::io::WriteHalf<tokio::io::DuplexStream>;
    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A connected client whose server answers the handshake, then runs
    /// `script` over the rest of the connection.
    async fn scripted_client<F, Fut>(
        script: F,
    ) -> Result<Arc<LspClient>, Box<dyn std::error::Error>>
    where
        F: FnOnce(Reader, Writer) -> Fut + Send + 'static,
        Fut: Future<Output = Option<Value>> + Send,
    {
        let (client_end, server_end) = tokio::io::duplex(1 << 16);
        let (client_read, client_write) = tokio::io::split(client_end);
        tokio::spawn(async move {
            let (server_read, mut writer) = tokio::io::split(server_end);
            let mut reader = tokio::io::BufReader::new(server_read);
            let init = read(&mut reader).await?;
            answer(&mut writer, &init, json!({"capabilities": {}})).await?;
            read(&mut reader).await?; // initialized
            script(reader, writer).await
        });
        Ok(Arc::new(
            LspClient::connect(client_read, client_write, Path::new("/")).await?,
        ))
    }

    /// A `signIn` reply handing the user `user_code`, with the command that
    /// waits for them to confirm it.
    fn device_code(user_code: &str) -> Value {
        json!({
            "userCode": user_code,
            "verificationUri": "https://github.com/login/device",
            "expiresIn": 900,
            "command": {"command": "github.copilot.finishDeviceFlow", "arguments": []}
        })
    }

    /// A connected client whose server answers `signIn` with a device code
    /// at once, then answers the completing command only after `delay` --
    /// the time the user takes to confirm in the browser.
    async fn slow_confirming_client(
        delay: Duration,
    ) -> Result<Arc<LspClient>, Box<dyn std::error::Error>> {
        scripted_client(move |mut reader, mut writer| async move {
            let sign_in = read(&mut reader).await?;
            answer(&mut writer, &sign_in, device_code("ABCD-EFGH")).await?;
            let finish = read(&mut reader).await?;
            tokio::time::sleep(delay).await;
            let landed = json!({"status": "OK", "user": "octocat"});
            answer(&mut writer, &finish, landed).await?;
            // Hold the connection open until the client goes.
            read(&mut reader).await
        })
        .await
    }

    /// Every outcome reported to the returned receiver.
    fn reporter() -> (
        impl Fn(AccountOutcome) + Clone + Send + 'static,
        tokio::sync::mpsc::UnboundedReceiver<AccountOutcome>,
    ) {
        let (sender, outcomes) = tokio::sync::mpsc::unbounded_channel();
        let report = move |outcome| {
            let _ = sender.send(outcome);
        };
        (report, outcomes)
    }

    /// What `recv` answers next -- an outcome, or `None` once every reporter
    /// is gone -- or `None` itself if it answers nothing for longer than any
    /// code lasts, so a lingering reporter fails the test instead of hanging
    /// it.
    async fn nothing_more(
        outcomes: &mut tokio::sync::mpsc::UnboundedReceiver<AccountOutcome>,
    ) -> Option<Option<AccountOutcome>> {
        let wait = MAX_DEVICE_CODE_LIFETIME * 2;
        tokio::time::timeout(wait, outcomes.recv()).await.ok()
    }

    async fn read<R>(reader: &mut R) -> Option<Value>
    where
        R: tokio::io::AsyncBufRead + Unpin,
    {
        let bytes = codec::read_frame(reader).await.ok()??;
        serde_json::from_slice(&bytes).ok()
    }

    async fn answer<W>(writer: &mut W, request: &Value, result: Value) -> Option<()>
    where
        W: tokio::io::AsyncWrite + Unpin,
    {
        let reply = json!({"jsonrpc": "2.0", "id": request["id"], "result": result});
        codec::write_frame(writer, reply.to_string().as_bytes())
            .await
            .ok()
    }

    /// Regression: the completing command waits for the user to confirm the
    /// code in their browser, which takes longer than the standard request
    /// timeout. A confirmation a minute in still reports the sign-in landed,
    /// not a false "timed out".
    #[tokio::test(start_paused = true)]
    async fn a_confirmation_slower_than_the_request_timeout_still_lands() -> TestResult {
        let client = slow_confirming_client(Duration::from_secs(60)).await?;
        let (report, mut outcomes) = reporter();
        let mut follow_up = FollowUpTask::default();
        account(&client, true, &mut follow_up, report).await;
        assert!(matches!(
            outcomes.recv().await,
            Some(AccountOutcome::DeviceCode { .. })
        ));
        assert_eq!(
            outcomes.recv().await,
            Some(AccountOutcome::SignedIn {
                user: Some("octocat".to_owned()),
            })
        );
        Ok(())
    }

    /// Regression (#759): a retried sign-in supersedes the first one's
    /// completing request. The server answers the abandoned request with a
    /// refusal before it answers the live one, and that refusal is never
    /// reported -- only the retry's sign-in is.
    #[tokio::test(start_paused = true)]
    async fn a_retried_sign_in_drops_the_first_ones_late_answer() -> TestResult {
        let (first_sent, first_seen) = tokio::sync::oneshot::channel();
        let client = scripted_client(move |mut reader, mut writer| async move {
            let sign_in = read(&mut reader).await?;
            answer(&mut writer, &sign_in, device_code("FIRST-CODE")).await?;
            let first_finish = read(&mut reader).await?;
            first_sent.send(()).ok()?;
            let retry = read(&mut reader).await?;
            answer(&mut writer, &retry, device_code("RETRY-CODE")).await?;
            let retry_finish = read(&mut reader).await?;
            let refused = json!({"status": "NotAuthorized"});
            answer(&mut writer, &first_finish, refused).await?;
            let landed = json!({"status": "OK", "user": "octocat"});
            answer(&mut writer, &retry_finish, landed).await?;
            read(&mut reader).await
        })
        .await?;
        let (report, mut outcomes) = reporter();
        let mut follow_up = FollowUpTask::default();
        account(&client, true, &mut follow_up, report.clone()).await;
        first_seen.await?;
        account(&client, true, &mut follow_up, report).await;
        let codes = [outcomes.recv().await, outcomes.recv().await];
        assert!(
            matches!(
                &codes,
                [
                    Some(AccountOutcome::DeviceCode { user_code: first, .. }),
                    Some(AccountOutcome::DeviceCode { user_code: retry, .. }),
                ] if first == "FIRST-CODE" && retry == "RETRY-CODE"
            ),
            "got {codes:?}"
        );
        assert_eq!(
            outcomes.recv().await,
            Some(AccountOutcome::SignedIn {
                user: Some("octocat".to_owned()),
            })
        );
        // Every reporter is gone -- the abandoned one with its aborted task --
        // and nothing else was reported.
        assert_eq!(nothing_more(&mut outcomes).await, Some(None));
        Ok(())
    }

    /// Regression (#759): signing out supersedes a sign-in still waiting on
    /// its code, so the server's late answer to it is not reported as a
    /// sign-in after the sign-out.
    #[tokio::test(start_paused = true)]
    async fn signing_out_drops_a_pending_sign_ins_late_answer() -> TestResult {
        let (finish_sent, finish_seen) = tokio::sync::oneshot::channel();
        let client = scripted_client(move |mut reader, mut writer| async move {
            let sign_in = read(&mut reader).await?;
            answer(&mut writer, &sign_in, device_code("ABCD-EFGH")).await?;
            let finish = read(&mut reader).await?;
            finish_sent.send(()).ok()?;
            let sign_out = read(&mut reader).await?;
            answer(&mut writer, &sign_out, Value::Null).await?;
            let landed = json!({"status": "OK", "user": "octocat"});
            answer(&mut writer, &finish, landed).await?;
            read(&mut reader).await
        })
        .await?;
        let (report, mut outcomes) = reporter();
        let mut follow_up = FollowUpTask::default();
        account(&client, true, &mut follow_up, report.clone()).await;
        finish_seen.await?;
        account(&client, false, &mut follow_up, report).await;
        assert!(matches!(
            outcomes.recv().await,
            Some(AccountOutcome::DeviceCode { .. })
        ));
        assert_eq!(outcomes.recv().await, Some(AccountOutcome::SignedOut));
        assert_eq!(nothing_more(&mut outcomes).await, Some(None));
        Ok(())
    }

    /// The wait is the code's own lifetime plus a grace, so the server's
    /// "expired" answer arrives first; a missing lifetime is GitHub's fifteen
    /// minutes, and an absurd one is capped.
    #[test]
    fn the_completing_request_waits_as_long_as_the_code_lasts() {
        let wait = |reply: Value| read_sign_in(&reply).1.map(|(_, _, timeout)| timeout);
        assert_eq!(
            wait(json!({"userCode": "A", "expiresIn": 600})),
            Some(Duration::from_secs(600) + EXPIRY_GRACE)
        );
        assert_eq!(
            wait(json!({"userCode": "A"})),
            Some(DEVICE_CODE_LIFETIME + EXPIRY_GRACE)
        );
        assert_eq!(
            wait(json!({"userCode": "A", "expiresIn": 999_999_999})),
            Some(MAX_DEVICE_CODE_LIFETIME + EXPIRY_GRACE)
        );
    }

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
            follow_up.map(|(method, params, _)| (method, params)),
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
            follow_up.map(|(method, params, _)| (method, params)),
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
