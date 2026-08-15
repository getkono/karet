//! The kernel client: readiness, execution, interrupt, shutdown — all
//! correlated by parent message id, with "cell done" defined as the iopub
//! `status: idle` whose parent is the execute request (per the protocol).

use std::time::Duration;

use jupyter_protocol::ExecutionState;
use jupyter_protocol::JupyterMessage;
use jupyter_protocol::messaging::ExecuteRequest;
use jupyter_protocol::messaging::InterruptRequest;
use jupyter_protocol::messaging::JupyterMessageContent;
use jupyter_protocol::messaging::KernelInfoRequest;
use jupyter_protocol::messaging::ShutdownRequest;
use serde_json::Map;
use serde_json::Value;

use super::KernelChannel;
use super::KernelError;
use super::KernelTransport;
use crate::Output;
use crate::Source;

/// How long one cell may run before the client gives up waiting.
const EXECUTE_TIMEOUT: Duration = Duration::from_secs(120);
/// How long control requests (interrupt, shutdown) may take.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
/// The interval between readiness probes while the kernel boots.
const READY_PROBE_INTERVAL: Duration = Duration::from_millis(300);

/// One executed cell's outcome, in the crate's own output model.
#[derive(Clone, Debug, PartialEq)]
pub struct CellOutcome {
    /// The kernel's execution counter for this run.
    pub execution_count: Option<i64>,
    /// Whether the cell raised (the queue's stop-on-error signal).
    pub errored: bool,
    /// The outputs, iopub order.
    pub outputs: Vec<Output>,
}

/// A client driving one kernel over a [`KernelTransport`].
pub struct KernelClient<T: KernelTransport> {
    transport: T,
}

impl<T: KernelTransport> KernelClient<T> {
    /// Wrap a connected transport.
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    /// Wait until the kernel answers a `kernel_info_request`, probing every
    /// [`READY_PROBE_INTERVAL`] — kernels drop shell traffic while booting,
    /// so a single unanswered probe means "not yet", not "dead".
    ///
    /// # Errors
    /// Returns [`KernelError::Timeout`] when `deadline` passes unanswered.
    pub async fn wait_ready(&mut self, deadline: Duration) -> Result<(), KernelError> {
        let until = tokio::time::Instant::now() + deadline;
        loop {
            let probe = JupyterMessage::new(KernelInfoRequest {}, None);
            let probe_id = probe.header.msg_id.clone();
            self.transport.send(KernelChannel::Shell, probe).await?;
            let wait = tokio::time::timeout_at(
                until.min(tokio::time::Instant::now() + READY_PROBE_INTERVAL),
                self.reply_to(&probe_id),
            );
            match wait.await {
                Ok(result) => return result.map(|_| ()),
                Err(_probe_elapsed) if tokio::time::Instant::now() < until => {},
                Err(_deadline) => return Err(KernelError::Timeout),
            }
        }
    }

    /// Execute `code`, collecting outputs until the kernel goes idle for
    /// this request. Stdin never opens (`allow_stdin: false`).
    ///
    /// # Errors
    /// Returns [`KernelError::Timeout`] after [`EXECUTE_TIMEOUT`], or a
    /// transport failure.
    pub async fn execute(&mut self, code: &str) -> Result<CellOutcome, KernelError> {
        let request = JupyterMessage::new(
            ExecuteRequest {
                code: code.to_owned(),
                silent: false,
                store_history: true,
                user_expressions: None,
                allow_stdin: false,
                stop_on_error: true,
            },
            None,
        );
        let request_id = request.header.msg_id.clone();
        self.transport.send(KernelChannel::Shell, request).await?;

        let mut outcome = CellOutcome {
            execution_count: None,
            errored: false,
            outputs: Vec::new(),
        };
        let mut reply_seen = false;
        let mut idle_seen = false;
        let until = tokio::time::Instant::now() + EXECUTE_TIMEOUT;
        while !(reply_seen && idle_seen) {
            let (channel, message) = tokio::time::timeout_at(until, self.transport.recv())
                .await
                .map_err(|_| KernelError::Timeout)??;
            if message
                .parent_header
                .as_ref()
                .map(|header| header.msg_id.as_str())
                != Some(request_id.as_str())
            {
                continue; // someone else's traffic (a restart banner, say)
            }
            match (channel, &message.content) {
                (KernelChannel::Shell, JupyterMessageContent::ExecuteReply(reply)) => {
                    outcome.execution_count = i64::try_from(reply.execution_count.value()).ok();
                    reply_seen = true;
                },
                (KernelChannel::Iopub, JupyterMessageContent::Status(status)) => {
                    if status.execution_state == ExecutionState::Idle {
                        idle_seen = true;
                    }
                },
                (KernelChannel::Iopub, content) => {
                    if let Some(output) = output_from(content) {
                        if matches!(output, Output::Error { .. }) {
                            outcome.errored = true;
                        }
                        outcome.outputs.push(output);
                    }
                },
                _ => {},
            }
        }
        Ok(outcome)
    }

    /// Interrupt the running cell over the control channel (the message
    /// mode; signal-mode kernels are interrupted by their process owner).
    ///
    /// # Errors
    /// Returns [`KernelError::Timeout`] if the kernel never acknowledges.
    pub async fn interrupt(&mut self) -> Result<(), KernelError> {
        let request = JupyterMessage::new(InterruptRequest {}, None);
        let request_id = request.header.msg_id.clone();
        self.transport.send(KernelChannel::Control, request).await?;
        tokio::time::timeout(CONTROL_TIMEOUT, self.reply_to(&request_id))
            .await
            .map_err(|_| KernelError::Timeout)?
            .map(|_| ())
    }

    /// Ask the kernel to exit (`restart: false`); the process owner reaps it.
    ///
    /// # Errors
    /// Returns [`KernelError::Timeout`] if the kernel never acknowledges.
    pub async fn shutdown(&mut self) -> Result<(), KernelError> {
        let request = JupyterMessage::new(ShutdownRequest { restart: false }, None);
        let request_id = request.header.msg_id.clone();
        self.transport.send(KernelChannel::Control, request).await?;
        tokio::time::timeout(CONTROL_TIMEOUT, self.reply_to(&request_id))
            .await
            .map_err(|_| KernelError::Timeout)?
            .map(|_| ())
    }

    /// Drain inbound traffic until a non-iopub reply to `request_id` arrives.
    async fn reply_to(&mut self, request_id: &str) -> Result<JupyterMessage, KernelError> {
        loop {
            let (channel, message) = self.transport.recv().await?;
            if channel != KernelChannel::Iopub
                && message
                    .parent_header
                    .as_ref()
                    .is_some_and(|header| header.msg_id == request_id)
            {
                return Ok(message);
            }
        }
    }
}

/// Map one iopub broadcast onto the crate's [`Output`] model; `None` for
/// messages that are not cell outputs (execute_input echoes, comms, …).
fn output_from(content: &JupyterMessageContent) -> Option<Output> {
    match content {
        JupyterMessageContent::StreamContent(stream) => Some(Output::Stream {
            name: match stream.name {
                jupyter_protocol::messaging::Stdio::Stdout => "stdout".to_owned(),
                jupyter_protocol::messaging::Stdio::Stderr => "stderr".to_owned(),
            },
            text: Source::Joined(stream.text.clone()),
            extra: Map::new(),
        }),
        JupyterMessageContent::ExecuteResult(result) => Some(Output::ExecuteResult {
            execution_count: i64::try_from(result.execution_count.value()).ok(),
            data: media_map(&result.data),
            metadata: result.metadata.clone(),
            extra: Map::new(),
        }),
        JupyterMessageContent::DisplayData(display) => Some(Output::DisplayData {
            data: media_map(&display.data),
            metadata: Map::new(),
            extra: Map::new(),
        }),
        JupyterMessageContent::ErrorOutput(error) => Some(Output::Error {
            ename: error.ename.clone(),
            evalue: error.evalue.clone(),
            traceback: error.traceback.clone(),
            extra: Map::new(),
        }),
        _ => None,
    }
}

/// A `Media` bundle as the model's plain MIME map.
fn media_map(media: &jupyter_protocol::media::Media) -> Map<String, Value> {
    match serde_json::to_value(media) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use jupyter_protocol::messaging::ExecuteReply;
    use jupyter_protocol::messaging::InterruptReply;
    use jupyter_protocol::messaging::Status;
    use jupyter_protocol::messaging::StreamContent;
    use serde_json::json;

    use super::*;

    /// A scripted kernel: `send` inspects the request and queues the wire
    /// traffic a real kernel would answer with; `recv` drains the queue.
    #[derive(Default)]
    struct FakeTransport {
        inbound: VecDeque<(KernelChannel, JupyterMessage)>,
        /// Ignore this many shell requests first (a booting kernel).
        drop_probes: usize,
        /// Send an unrelated broadcast before real answers once.
        noise: bool,
    }

    impl KernelTransport for FakeTransport {
        async fn send(
            &mut self,
            channel: KernelChannel,
            message: JupyterMessage,
        ) -> Result<(), KernelError> {
            if self.noise {
                self.noise = false;
                let stranger = JupyterMessage::new(
                    StreamContent {
                        name: jupyter_protocol::messaging::Stdio::Stdout,
                        text: "from another client\n".to_owned(),
                    },
                    None,
                );
                self.inbound.push_back((KernelChannel::Iopub, stranger));
            }
            match &message.content {
                JupyterMessageContent::KernelInfoRequest(_) => {
                    if self.drop_probes > 0 {
                        self.drop_probes -= 1;
                        return Ok(());
                    }
                    let reply = JupyterMessage::new(
                        Status {
                            execution_state: ExecutionState::Idle,
                        },
                        Some(&message),
                    );
                    // A status stands in for the kernel_info_reply shape; the
                    // client only correlates channel + parent id.
                    self.inbound.push_back((channel, reply));
                },
                JupyterMessageContent::ExecuteRequest(request) => {
                    let stream = JupyterMessage::new(
                        StreamContent {
                            name: jupyter_protocol::messaging::Stdio::Stdout,
                            text: format!("ran: {}\n", request.code),
                        },
                        Some(&message),
                    );
                    self.inbound.push_back((KernelChannel::Iopub, stream));
                    if request.code.contains("boom") {
                        let error: JupyterMessageContent =
                            JupyterMessageContent::from_type_and_content(
                                "error",
                                json!({"ename": "ValueError", "evalue": "boom",
                                       "traceback": ["tb"]}),
                            )
                            .unwrap_or(JupyterMessageContent::Status(
                                Status {
                                    execution_state: ExecutionState::Idle,
                                },
                            ));
                        self.inbound.push_back((
                            KernelChannel::Iopub,
                            JupyterMessage::new(error, Some(&message)),
                        ));
                    }
                    let reply = JupyterMessage::new(ExecuteReply::default(), Some(&message));
                    self.inbound.push_back((KernelChannel::Shell, reply));
                    let idle = JupyterMessage::new(
                        Status {
                            execution_state: ExecutionState::Idle,
                        },
                        Some(&message),
                    );
                    self.inbound.push_back((KernelChannel::Iopub, idle));
                },
                JupyterMessageContent::InterruptRequest(_) => {
                    let reply = JupyterMessage::new(InterruptReply::default(), Some(&message));
                    self.inbound.push_back((KernelChannel::Control, reply));
                },
                _ => {},
            }
            Ok(())
        }

        async fn recv(&mut self) -> Result<(KernelChannel, JupyterMessage), KernelError> {
            match self.inbound.pop_front() {
                Some(item) => Ok(item),
                None => {
                    // A silent kernel: park until the caller's timeout fires.
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    Err(KernelError::Timeout)
                },
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn execute_collects_outputs_until_idle_and_skips_noise() -> Result<(), KernelError> {
        let mut client = KernelClient::new(FakeTransport {
            noise: true,
            ..FakeTransport::default()
        });
        let outcome = client.execute("print(1)").await?;
        assert!(!outcome.errored);
        assert_eq!(outcome.outputs.len(), 1, "{:?}", outcome.outputs);
        assert!(matches!(
            &outcome.outputs[0],
            Output::Stream { text, .. } if text.text().contains("ran: print(1)")
        ));
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn an_error_output_marks_the_outcome() -> Result<(), KernelError> {
        let mut client = KernelClient::new(FakeTransport::default());
        let outcome = client.execute("boom()").await?;
        assert!(outcome.errored);
        assert!(matches!(
            outcome.outputs.last(),
            Some(Output::Error { ename, .. }) if ename == "ValueError"
        ));
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn readiness_survives_dropped_probes_and_times_out_on_silence() {
        let mut client = KernelClient::new(FakeTransport {
            drop_probes: 2,
            ..FakeTransport::default()
        });
        let ready = client.wait_ready(Duration::from_secs(5)).await;
        assert!(ready.is_ok(), "{ready:?}");

        let mut dead = KernelClient::new(FakeTransport {
            drop_probes: usize::MAX,
            ..FakeTransport::default()
        });
        let never = dead.wait_ready(Duration::from_secs(2)).await;
        assert!(matches!(never, Err(KernelError::Timeout)), "{never:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn interrupt_round_trips_on_the_control_channel() -> Result<(), KernelError> {
        let mut client = KernelClient::new(FakeTransport::default());
        client.interrupt().await
    }
}
