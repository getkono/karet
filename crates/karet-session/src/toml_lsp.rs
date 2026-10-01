//! The built-in TOML language server (feature `toml-lsp`): taplo's server, run
//! in-process on a thread of its own and spoken to over an in-memory pipe.
//!
//! taplo's native release assets carry no publisher-authenticated digest, so
//! the managed registry will not fetch them, and before this TOML was the one
//! language whose default server could only ever be installed by hand.
//! Compiling the server in makes the crates.io checksum its integrity check and
//! leaves nothing to install.
//!
//! It is the *last* resolution step, never the first: a taplo the user
//! configured, the project ships, or `PATH` provides still wins, so a pinned
//! version keeps working exactly as before.
//!
//! The server is taplo's own handler table driven by a loop of ours, because
//! the transports taplo ships are stdio and TCP, and neither is an in-memory
//! pipe. Its handlers are `!Send`, which is why it gets a current-thread
//! runtime and a [`tokio::task::LocalSet`] rather than a task on the session's
//! runtime.

use std::cell::RefCell;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::Once;

use futures::SinkExt;
use futures::StreamExt;
use futures::channel::mpsc;
use futures::sink::SinkMapErr;
use karet_lsp::LaunchFailure;
use karet_lsp::LspClient;
use karet_lsp::LspError;
use karet_lsp::LspSpec;
use karet_lsp::codec;
use lsp_async_stub::rpc::Message;
use serde_json::Value;
use taplo_common::environment::native::NativeEnvironment;
use tokio::io::AsyncBufRead;
use tokio::io::AsyncWrite;
use tokio::io::BufReader;
use tokio::io::DuplexStream;
use tokio::sync::Notify;

use crate::api::LanguageServerId;

/// The provider this module stands in for.
const PROVIDER: &str = "taplo";

/// The command a built-in launch carries.
///
/// Never executed: the connector recognises it and connects in-process instead
/// of spawning. It is what the language-server panel shows as the command, so
/// it says what runs rather than naming a binary that is not there.
pub(crate) const COMMAND: &str = "karet-builtin:taplo";

/// Bytes either side of the pipe may buffer before the writer waits.
const PIPE_CAPACITY: usize = 1 << 20;

/// Whether `server` is built into this karet, so it never needs installing.
pub(crate) fn bundles(server: &LanguageServerId) -> bool {
    server.key() == PROVIDER
}

/// The launch that runs the built-in server for `language`, when `server` is
/// the one this build carries.
pub(crate) fn spec(server: &LanguageServerId, language: &str) -> Option<LspSpec> {
    bundles(server).then(|| LspSpec::new(COMMAND, Vec::new(), vec![language.to_owned()]))
}

/// Whether `spec` asks for the built-in server rather than a process.
pub(crate) fn is_builtin(spec: &LspSpec) -> bool {
    spec.command == COMMAND
}

/// Start the built-in server and complete the handshake with it.
///
/// # Errors
/// A [`LaunchCause::Host`](karet_lsp::LaunchCause::Host) launch failure when
/// the server's thread cannot be started, else whatever the handshake fails
/// with.
pub(crate) async fn connect(
    root: &Path,
    initialization_options: Option<Value>,
) -> Result<LspClient, LspError> {
    let (client, server) = tokio::io::duplex(PIPE_CAPACITY);
    std::thread::Builder::new()
        .name("karet-taplo-lsp".into())
        .spawn(move || serve(server))
        .map_err(|error| {
            LspError::Launch(Box::new(LaunchFailure::host(
                COMMAND,
                Vec::new(),
                format!("the built-in taplo server's thread could not start: {error}"),
            )))
        })?;
    let (read, write) = tokio::io::split(client);
    LspClient::connect_with(read, write, root, initialization_options).await
}

thread_local! {
    /// Set only on a built-in server's own thread: what a panic there signals.
    static ON_PANIC: RefCell<Option<Arc<Notify>>> = const { RefCell::new(None) };
}

/// The writer a handler answers through.
type Writer = SinkMapErr<mpsc::UnboundedSender<Message>, fn(mpsc::SendError) -> io::Error>;

/// A failed send can only mean the writer task stopped: the client hung up.
fn hung_up(_closed: mpsc::SendError) -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "the client has gone")
}

/// Route panics on a built-in server's thread away from the process-wide hook.
///
/// A panic in a taplo task is caught by its runtime and would otherwise leave
/// the pipe open with the request unanswered until the client times out. It
/// would also run the host's hook, and a TUI host's hook restores the terminal
/// under an editor that is still running. Installed once, wrapping whatever
/// hook is current, so every other thread's panic reaches it unchanged.
fn contain_panics() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if signal_panic() {
                tracing::error!(%info, "the built-in taplo server panicked and is restarting");
            } else {
                previous(info);
            }
        }));
    });
}

/// Tell this thread's server it panicked, when this is a server's thread.
fn signal_panic() -> bool {
    ON_PANIC
        .try_with(|slot| {
            slot.try_borrow()
                .ok()
                .and_then(|slot| slot.as_ref().map(|panicked| panicked.notify_one()))
                .is_some()
        })
        .unwrap_or(false)
}

/// Run the server over `stream` until the client sends `exit` or hangs up, or
/// the server panics.
///
/// Dropping `stream` on return is what tells the client the server is gone,
/// so a server that stops for any reason surfaces as a closed connection and
/// takes the ordinary restart path.
fn serve(stream: DuplexStream) {
    host(stream, |stream, panicked| async move {
        let server = taplo_lsp::create_server();
        let world = taplo_lsp::create_world(NativeEnvironment::new());
        drive(stream, panicked, move |message, writer| {
            server.handle_message(world.clone(), message, writer)
        })
        .await;
    });
}

/// Run `session` on this thread's own runtime, ending it on any panic here.
///
/// Every task taplo spawns, its own included, runs on this thread, so the
/// panic hook [`contain_panics`] installs sees each of their panics.
fn host<S, F>(stream: DuplexStream, session: S)
where
    S: FnOnce(DuplexStream, Arc<Notify>) -> F,
    F: Future<Output = ()>,
{
    contain_panics();
    let panicked = Arc::new(Notify::new());
    ON_PANIC.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&panicked)));
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::warn!(%error, "the built-in taplo server could not build its runtime");
            return;
        },
    };
    // The `LocalSet` is dropped with every task on it, so the writer's half of
    // the pipe goes with it.
    tokio::task::LocalSet::new().block_on(&runtime, session(stream, panicked));
}

/// Dispatch every message to `handle` until `exit`, a hang-up, or a panic.
async fn drive<H, F>(stream: DuplexStream, panicked: Arc<Notify>, mut handle: H)
where
    H: FnMut(Message, Writer) -> F,
    F: Future<Output = io::Result<()>> + 'static,
{
    let (read, mut write) = tokio::io::split(stream);
    let (outgoing, mut queue) = mpsc::unbounded::<Message>();
    tokio::task::spawn_local(async move {
        while let Some(message) = queue.next().await {
            if let Err(error) = write_message(&mut write, &message).await {
                tracing::debug!(%error, "the built-in taplo server lost its client");
                break;
            }
        }
    });
    let mut reader = BufReader::new(read);
    loop {
        // A panic leaves a request unanswered and the server's state suspect,
        // so it ends the session rather than letting the client wait it out.
        let message = tokio::select! {
            message = read_message(&mut reader) => message,
            () = panicked.notified() => break,
        };
        let Some(message) = message else { break };
        if message.method.as_deref() == Some("exit") {
            break;
        }
        let writer: Writer = outgoing.clone().sink_map_err(hung_up);
        let handled = handle(message, writer);
        tokio::task::spawn_local(async move {
            if let Err(error) = handled.await {
                tracing::debug!(%error, "a built-in taplo handler failed");
            }
        });
    }
}

/// Read one `Content-Length`-framed message, through the codec every karet
/// language-server connection uses.
///
/// [`None`] ends the session: end of stream, a frame the codec rejects (no
/// usable length, one over its cap, a truncated body), or a body that is not a
/// JSON-RPC message. The client is karet's own, so a malformed frame means the
/// stream is no longer trustworthy, and resyncing from the middle of a body
/// would be guessing.
async fn read_message<R: AsyncBufRead + Unpin>(reader: &mut R) -> Option<Message> {
    let body = codec::read_frame(reader).await.ok()??;
    serde_json::from_slice(&body).ok()
}

async fn write_message<W: AsyncWrite + Unpin>(out: &mut W, message: &Message) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    codec::write_frame(out, &body).await
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;
    use tokio::io::AsyncWriteExt;
    use tokio::io::ReadHalf;
    use tokio::io::WriteHalf;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Long enough for a loaded CI machine, short enough that a server which
    /// never answers fails the test instead of hanging the suite.
    const PATIENCE: Duration = Duration::from_secs(20);

    fn frame(body: &str) -> Vec<u8> {
        format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
    }

    async fn send(out: &mut WriteHalf<DuplexStream>, message: &Value) -> TestResult {
        out.write_all(&frame(&message.to_string())).await?;
        out.flush().await?;
        Ok(())
    }

    /// The next message from the server that `wanted` accepts, answering any
    /// `workspace/configuration` request on the way with schemas switched off.
    ///
    /// Switched off because taplo's default configuration fetches the
    /// SchemaStore catalogue, and the merge gate makes no network request.
    async fn next_matching(
        reader: &mut BufReader<ReadHalf<DuplexStream>>,
        out: &mut WriteHalf<DuplexStream>,
        wanted: impl Fn(&Message) -> bool,
    ) -> Result<Message, Box<dyn std::error::Error>> {
        let found = tokio::time::timeout(PATIENCE, async {
            while let Some(message) = read_message(reader).await {
                if message.method.as_deref() == Some("workspace/configuration") {
                    let reply = json!({
                        "jsonrpc": "2.0",
                        "id": message.id,
                        "result": [{ "schema": { "enabled": false } }],
                    });
                    send(out, &reply).await.ok()?;
                    continue;
                }
                if wanted(&message) {
                    return Some(message);
                }
            }
            None
        })
        .await?;
        found.ok_or_else(|| "the server closed before sending the expected message".into())
    }

    fn is_response(id: i64) -> impl Fn(&Message) -> bool {
        move |message| {
            message.method.is_none() && serde_json::to_value(&message.id).ok() == Some(json!(id))
        }
    }

    #[test]
    fn only_taplo_is_built_in() {
        assert!(bundles(&LanguageServerId::new("taplo")));
        assert!(!bundles(&LanguageServerId::new("gopls")));
        assert!(spec(&LanguageServerId::new("gopls"), "go").is_none());
    }

    #[test]
    fn the_built_in_launch_is_recognised_and_a_path_launch_is_not() {
        let built_in = spec(&LanguageServerId::new("taplo"), "toml");
        assert!(built_in.as_ref().is_some_and(is_builtin));
        assert_eq!(
            built_in.map(|spec| spec.languages),
            Some(vec!["toml".to_owned()])
        );
        // A taplo on PATH is launched as a process, never routed in-process.
        let on_path = LspSpec::new("taplo", vec!["lsp".into(), "stdio".into()], Vec::new());
        assert!(!is_builtin(&on_path));
    }

    #[tokio::test]
    async fn a_frame_is_read_whatever_the_header_case() -> TestResult {
        let body = r#"{"jsonrpc":"2.0","method":"exit"}"#;
        let bytes = format!("content-length: {}\r\n\r\n{body}", body.len());
        let message = read_message(&mut bytes.as_bytes()).await;
        assert_eq!(
            message.and_then(|message| message.method),
            Some("exit".to_owned())
        );
        Ok(())
    }

    /// Every way a frame can be unusable ends the session rather than leaving
    /// the reader stranded mid-stream.
    #[tokio::test]
    async fn an_unusable_frame_ends_the_session() {
        let oversized = format!("Content-Length: {}\r\n\r\n", codec::MAX_MESSAGE_BYTES + 1);
        let cases: [&[u8]; 5] = [
            b"",
            b"Content-Length: 4\r\n\r\nnull",
            b"Content-Length: banana\r\n\r\n{}",
            b"Content-Length: 100\r\n\r\n{}",
            oversized.as_bytes(),
        ];
        for case in cases {
            let mut reader = case;
            assert!(
                read_message(&mut reader).await.is_none(),
                "{:?} should end the session",
                String::from_utf8_lossy(case)
            );
        }
    }

    /// Send one request to a server whose handler panics, and require the
    /// server to close its end of the pipe rather than leave it waiting.
    async fn a_panic_closes_the_pipe<H, F>(handle: H) -> TestResult
    where
        H: FnMut(Message, Writer) -> F + Send + 'static,
        F: Future<Output = io::Result<()>> + 'static,
    {
        let (client, server) = tokio::io::duplex(PIPE_CAPACITY);
        let thread = std::thread::spawn(move || {
            host(server, |stream, panicked| drive(stream, panicked, handle));
        });
        let (read, mut out) = tokio::io::split(client);
        let mut reader = BufReader::new(read);
        let hover = json!({ "jsonrpc": "2.0", "id": 1, "method": "textDocument/hover" });
        send(&mut out, &hover).await?;
        let closed = tokio::time::timeout(PATIENCE, async {
            while read_message(&mut reader).await.is_some() {}
        })
        .await;
        assert!(closed.is_ok(), "the server kept its end open after a panic");
        assert!(
            thread.join().is_ok(),
            "the panic escaped the server's runtime"
        );
        Ok(())
    }

    // A deliberate panic: it is the behaviour under test.
    #[allow(clippy::panic)]
    #[tokio::test]
    async fn a_panicking_handler_ends_the_session() -> TestResult {
        a_panic_closes_the_pipe(|_message, _writer| async { panic!("handler panicked") }).await
    }

    /// taplo publishes diagnostics from tasks of its own, not from the handler
    /// the loop spawned, so a panic there must end the session too.
    // A deliberate panic: it is the behaviour under test.
    #[allow(clippy::panic)]
    #[tokio::test]
    async fn a_panic_in_a_task_the_handler_spawned_ends_the_session() -> TestResult {
        a_panic_closes_the_pipe(|_message, _writer| async {
            tokio::task::spawn_local(async { panic!("spawned task panicked") });
            Ok(())
        })
        .await
    }

    /// Only a server's own thread is diverted from the host's panic hook.
    #[test]
    fn only_a_server_thread_diverts_its_panics() {
        assert!(
            !signal_panic(),
            "a thread with no server reaches the host hook"
        );
        let diverted = std::thread::spawn(|| {
            let panicked = Arc::new(Notify::new());
            ON_PANIC.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&panicked)));
            signal_panic()
        })
        .join();
        assert!(matches!(diverted, Ok(true)));
    }

    /// The whole built-in path: the handshake advertises what the panel shows,
    /// a broken document is diagnosed, a messy one is formatted, and the
    /// session ends cleanly on `shutdown` + `exit`.
    #[tokio::test]
    async fn the_built_in_server_validates_formats_and_shuts_down() -> TestResult {
        let (client, server) = tokio::io::duplex(PIPE_CAPACITY);
        let thread = std::thread::spawn(move || serve(server));
        let (read, mut out) = tokio::io::split(client);
        let mut reader = BufReader::new(read);

        let initialize = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "processId": null, "rootUri": null, "capabilities": {} },
        });
        send(&mut out, &initialize).await?;
        let answer = next_matching(&mut reader, &mut out, is_response(1)).await?;
        let capabilities = answer
            .result
            .as_ref()
            .and_then(|result| result.get("capabilities"))
            .cloned()
            .unwrap_or_default();
        assert_eq!(capabilities["documentFormattingProvider"], json!(true));
        assert_eq!(capabilities["hoverProvider"], json!(true));
        assert!(capabilities.get("completionProvider").is_some());

        send(
            &mut out,
            &json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        )
        .await?;
        let uri = "file:///workspace/Cargo.toml";
        send(
            &mut out,
            &json!({
                "jsonrpc": "2.0", "method": "textDocument/didOpen",
                "params": { "textDocument": {
                    "uri": uri, "languageId": "toml", "version": 1,
                    "text": "a  =   1\n[b\n",
                }},
            }),
        )
        .await?;
        let published = next_matching(&mut reader, &mut out, |message| {
            message.method.as_deref() == Some("textDocument/publishDiagnostics")
        })
        .await?;
        let diagnostics = published
            .params
            .as_ref()
            .and_then(|params| params.get("diagnostics"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        assert!(diagnostics > 0, "an unclosed table header is an error");

        send(
            &mut out,
            &json!({
                "jsonrpc": "2.0", "method": "textDocument/didChange",
                "params": {
                    "textDocument": { "uri": uri, "version": 2 },
                    "contentChanges": [{ "text": "a  =   1\n" }],
                },
            }),
        )
        .await?;
        send(
            &mut out,
            &json!({
                "jsonrpc": "2.0", "id": 2, "method": "textDocument/formatting",
                "params": {
                    "textDocument": { "uri": uri },
                    "options": { "tabSize": 2, "insertSpaces": true },
                },
            }),
        )
        .await?;
        let formatted = next_matching(&mut reader, &mut out, is_response(2)).await?;
        let edits = formatted.result.unwrap_or_default();
        assert_eq!(edits[0]["newText"], json!("a = 1\n"), "{edits}");

        send(
            &mut out,
            &json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null }),
        )
        .await?;
        let stopped = next_matching(&mut reader, &mut out, is_response(3)).await?;
        assert!(stopped.error.is_none(), "{:?}", stopped.error);
        send(&mut out, &json!({ "jsonrpc": "2.0", "method": "exit" })).await?;

        // `exit` ends the server, which closes its end of the pipe.
        let closed = tokio::time::timeout(PATIENCE, async {
            while read_message(&mut reader).await.is_some() {}
        })
        .await;
        assert!(closed.is_ok(), "the server kept its end open after exit");
        assert!(thread.join().is_ok(), "the server thread panicked");
        Ok(())
    }
}
