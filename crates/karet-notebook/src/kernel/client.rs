//! The kernel client: readiness, execution, shutdown — all correlated by
//! parent message id, with "cell done" defined as the iopub `status: idle`
//! whose parent is the execute request (per the protocol). An interrupt goes
//! out of band, through [`KernelControl`](super::KernelControl).

use std::time::Duration;

use jupyter_protocol::ExecutionState;
use jupyter_protocol::JupyterMessage;
use jupyter_protocol::messaging::ExecuteRequest;
use jupyter_protocol::messaging::JupyterMessageContent;
use jupyter_protocol::messaging::KernelInfoRequest;
use jupyter_protocol::messaging::ShutdownRequest;
use serde_json::Map;
use serde_json::Value;

use super::KernelChannel;
use super::KernelError;
use super::KernelTransport;
use super::control::CONTROL_TIMEOUT;
use crate::Output;
use crate::Source;
use crate::StreamName;

/// How long one cell may run before the client gives up waiting.
const EXECUTE_TIMEOUT: Duration = Duration::from_secs(120);
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
                jupyter_protocol::messaging::Stdio::Stdout => StreamName::Stdout,
                jupyter_protocol::messaging::Stdio::Stderr => StreamName::Stderr,
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
    use jupyter_protocol::messaging::ShutdownReply;
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
        /// The execution counter every `execute_reply` carries.
        count: usize,
        /// Extra iopub broadcasts a cell emits after its stdout echo.
        broadcasts: Vec<JupyterMessageContent>,
        /// Withhold the shell `execute_reply`.
        withhold_reply: bool,
        /// Withhold the iopub `status: idle`.
        withhold_idle: bool,
        /// Broadcast idle *before* sending the reply.
        idle_first: bool,
        /// Never answer control requests.
        mute_control: bool,
        /// Answer control requests on iopub instead of control.
        control_on_iopub: bool,
        /// Every request the client sent, in order.
        sent: Vec<(KernelChannel, String)>,
    }

    impl crate::kernel::transport::sealed::Transport for FakeTransport {
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
            self.sent
                .push((channel, message.content.message_type().to_owned()));
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
                    for content in self.broadcasts.clone() {
                        self.inbound.push_back((
                            KernelChannel::Iopub,
                            JupyterMessage::new(content, Some(&message)),
                        ));
                    }
                    let reply = JupyterMessage::new(
                        ExecuteReply {
                            execution_count: self.count.into(),
                            ..ExecuteReply::default()
                        },
                        Some(&message),
                    );
                    let idle = JupyterMessage::new(
                        Status {
                            execution_state: ExecutionState::Idle,
                        },
                        Some(&message),
                    );
                    let reply = (!self.withhold_reply).then_some((KernelChannel::Shell, reply));
                    let idle = (!self.withhold_idle).then_some((KernelChannel::Iopub, idle));
                    let ordered = if self.idle_first {
                        [idle, reply]
                    } else {
                        [reply, idle]
                    };
                    self.inbound.extend(ordered.into_iter().flatten());
                },
                JupyterMessageContent::InterruptRequest(_)
                | JupyterMessageContent::ShutdownRequest(_)
                    if !self.mute_control =>
                {
                    let reply = match &message.content {
                        JupyterMessageContent::ShutdownRequest(_) => {
                            JupyterMessage::new(ShutdownReply::default(), Some(&message))
                        },
                        _ => JupyterMessage::new(InterruptReply::default(), Some(&message)),
                    };
                    let on = if self.control_on_iopub {
                        KernelChannel::Iopub
                    } else {
                        KernelChannel::Control
                    };
                    self.inbound.push_back((on, reply));
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

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// An iopub content built from its wire type and JSON body.
    fn content(msg_type: &str, body: Value) -> serde_json::Result<JupyterMessageContent> {
        JupyterMessageContent::from_type_and_content(msg_type, body)
    }

    #[tokio::test(start_paused = true)]
    async fn execute_reports_the_reply_counter_and_sends_on_shell() -> Result<(), KernelError> {
        let mut client = KernelClient::new(FakeTransport {
            count: 12,
            ..FakeTransport::default()
        });
        let outcome = client.execute("x").await?;
        assert_eq!(outcome.execution_count, Some(12));
        assert_eq!(
            client.transport.sent,
            vec![(KernelChannel::Shell, "execute_request".to_owned())]
        );
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn execute_maps_every_output_kind_in_iopub_order() -> TestResult {
        let mut client = KernelClient::new(FakeTransport {
            count: 3,
            broadcasts: vec![
                content("stream", json!({"name": "stderr", "text": "warn\n"}))?,
                // Not outputs: the input echo and a busy status are skipped.
                content("execute_input", json!({"code": "x", "execution_count": 3}))?,
                content("status", json!({"execution_state": "busy"}))?,
                content(
                    "execute_result",
                    json!({"execution_count": 3, "data": {"text/plain": "3"},
                           "metadata": {"m": 1}}),
                )?,
                content(
                    "display_data",
                    json!({"data": {"text/html": "<b>"}, "metadata": {"kept": false}}),
                )?,
            ],
            ..FakeTransport::default()
        });
        let outcome = client.execute("x").await?;
        let map = |value: Value| match value {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        assert_eq!(
            outcome,
            CellOutcome {
                execution_count: Some(3),
                errored: false,
                outputs: vec![
                    Output::Stream {
                        name: StreamName::Stdout,
                        text: Source::Joined("ran: x\n".to_owned()),
                        extra: Map::new(),
                    },
                    Output::Stream {
                        name: StreamName::Stderr,
                        text: Source::Joined("warn\n".to_owned()),
                        extra: Map::new(),
                    },
                    Output::ExecuteResult {
                        execution_count: Some(3),
                        data: map(json!({"text/plain": "3"})),
                        metadata: map(json!({"m": 1})),
                        extra: Map::new(),
                    },
                    // Display metadata is not carried over (pinned as-is).
                    Output::DisplayData {
                        data: map(json!({"text/html": "<b>"})),
                        metadata: Map::new(),
                        extra: Map::new(),
                    },
                ],
            }
        );
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn idle_before_the_reply_still_completes_the_cell() -> Result<(), KernelError> {
        let mut client = KernelClient::new(FakeTransport {
            idle_first: true,
            count: 4,
            ..FakeTransport::default()
        });
        let outcome = client.execute("x").await?;
        assert_eq!(outcome.execution_count, Some(4));
        assert_eq!(outcome.outputs.len(), 1);
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn execute_times_out_after_its_deadline_without_both_signals() {
        for (withhold_reply, withhold_idle) in [(true, true), (true, false), (false, true)] {
            let mut client = KernelClient::new(FakeTransport {
                withhold_reply,
                withhold_idle,
                ..FakeTransport::default()
            });
            let started = tokio::time::Instant::now();
            let result = client.execute("x").await;
            assert!(matches!(result, Err(KernelError::Timeout)), "{result:?}");
            assert_eq!(started.elapsed(), EXECUTE_TIMEOUT);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn readiness_gives_up_exactly_at_the_deadline() {
        let mut dead = KernelClient::new(FakeTransport {
            drop_probes: usize::MAX,
            ..FakeTransport::default()
        });
        let started = tokio::time::Instant::now();
        let never = dead.wait_ready(Duration::from_secs(2)).await;
        assert!(matches!(never, Err(KernelError::Timeout)), "{never:?}");
        assert_eq!(started.elapsed(), Duration::from_secs(2));
        // One probe per interval: at 0, 300, …, 1800ms.
        assert_eq!(dead.transport.sent.len(), 7);
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_round_trips_on_the_control_channel() -> Result<(), KernelError> {
        let mut client = KernelClient::new(FakeTransport::default());
        client.shutdown().await?;
        assert_eq!(
            client.transport.sent,
            vec![(KernelChannel::Control, "shutdown_request".to_owned())]
        );
        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn unanswered_control_requests_time_out_after_the_control_deadline() {
        for transport in [
            FakeTransport {
                mute_control: true,
                ..FakeTransport::default()
            },
            // A reply broadcast on iopub is not a reply.
            FakeTransport {
                control_on_iopub: true,
                ..FakeTransport::default()
            },
        ] {
            let mut client = KernelClient::new(transport);
            let started = tokio::time::Instant::now();
            let shutdown = client.shutdown().await;
            assert!(
                matches!(shutdown, Err(KernelError::Timeout)),
                "{shutdown:?}"
            );
            assert_eq!(started.elapsed(), CONTROL_TIMEOUT);
        }
    }

    #[test]
    fn output_mapping_drops_non_outputs_and_unrepresentable_counters() -> TestResult {
        assert_eq!(
            output_from(&content("status", json!({"execution_state": "idle"}))?),
            None
        );
        assert_eq!(
            output_from(&content("clear_output", json!({"wait": false}))?),
            None
        );
        // A counter past i64::MAX cannot be represented: it maps to `None`
        // rather than wrapping.
        let huge =
            JupyterMessageContent::ExecuteResult(jupyter_protocol::messaging::ExecuteResult {
                execution_count: usize::MAX.into(),
                ..Default::default()
            });
        assert!(matches!(
            output_from(&huge),
            Some(Output::ExecuteResult {
                execution_count: None,
                ..
            })
        ));
        assert!(matches!(
            output_from(&content(
                "error",
                json!({"ename": "E", "evalue": "v", "traceback": ["a", "b"]})
            )?),
            Some(Output::Error { ename, evalue, traceback, .. })
                if ename == "E" && evalue == "v" && traceback == ["a", "b"]
        ));
        Ok(())
    }
}
