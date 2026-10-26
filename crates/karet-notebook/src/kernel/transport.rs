//! The wire transport: the Jupyter multipart frame layout over ZMTP, behind
//! the [`KernelTransport`] seam so the client is testable without sockets.
//!
//! Frame layout (both directions):
//! `identities… | "<IDS|MSG>" | hmac-sha256 hex | header | parent_header |
//! metadata | content | buffers…` — the signature covers exactly the four
//! JSON frames.

use std::future::Future;

use hmac::Mac;
use jupyter_protocol::ConnectionInfo;
use jupyter_protocol::Header;
use jupyter_protocol::JupyterMessage;
use jupyter_protocol::messaging::JupyterMessageContent;
use zeromq::Socket;
use zeromq::SocketRecv;
use zeromq::SocketSend;
use zeromq::ZmqMessage;

use super::KernelError;

/// The delimiter separating routing identities from the signed payload.
const DELIMITER: &[u8] = b"<IDS|MSG>";

/// The channel a message travels on. karet drives `shell` (execution),
/// `control` (interrupt/shutdown), and `iopub` (the broadcast stream);
/// `stdin` never opens — execute requests set `allow_stdin: false`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelChannel {
    /// Requests/replies for execution and introspection.
    Shell,
    /// Out-of-band requests (interrupt, shutdown).
    Control,
    /// The kernel's broadcast stream (outputs, status).
    Iopub,
}

/// The transport seam: production is [`ZmqTransport`]; tests script an
/// in-process fake. Implementations merge every inbound channel into one
/// `recv` stream.
pub trait KernelTransport: Send {
    /// Send one message on `channel`.
    fn send(
        &mut self,
        channel: KernelChannel,
        message: JupyterMessage,
    ) -> impl Future<Output = Result<(), KernelError>> + Send;

    /// The next inbound message from any channel.
    fn recv(
        &mut self,
    ) -> impl Future<Output = Result<(KernelChannel, JupyterMessage), KernelError>> + Send;
}

/// The production transport: DEALER shell/control, SUB iopub, all loopback.
pub struct ZmqTransport {
    key: String,
    shell: zeromq::DealerSocket,
    control: zeromq::DealerSocket,
    iopub: zeromq::SubSocket,
}

impl ZmqTransport {
    /// Connect the three sockets to the endpoints in `info`.
    ///
    /// # Errors
    /// Returns [`KernelError::Transport`] when a socket cannot connect.
    pub async fn connect(info: &ConnectionInfo) -> Result<Self, KernelError> {
        let url = |port: u16| format!("tcp://{}:{port}", info.ip);
        let fail = |error: zeromq::ZmqError| KernelError::Transport(error.to_string());
        let mut shell = zeromq::DealerSocket::new();
        shell.connect(&url(info.shell_port)).await.map_err(fail)?;
        let mut control = zeromq::DealerSocket::new();
        control
            .connect(&url(info.control_port))
            .await
            .map_err(fail)?;
        let mut iopub = zeromq::SubSocket::new();
        iopub.connect(&url(info.iopub_port)).await.map_err(fail)?;
        iopub.subscribe("").await.map_err(fail)?;
        Ok(Self {
            key: info.key.clone(),
            shell,
            control,
            iopub,
        })
    }
}

impl KernelTransport for ZmqTransport {
    async fn send(
        &mut self,
        channel: KernelChannel,
        message: JupyterMessage,
    ) -> Result<(), KernelError> {
        let wire = encode(&message, &self.key)?;
        let fail = |error: zeromq::ZmqError| KernelError::Transport(error.to_string());
        match channel {
            KernelChannel::Shell => self.shell.send(wire).await.map_err(fail),
            KernelChannel::Control => self.control.send(wire).await.map_err(fail),
            KernelChannel::Iopub => Err(KernelError::Protocol(
                "iopub is a broadcast stream; clients never send on it".to_owned(),
            )),
        }
    }

    async fn recv(&mut self) -> Result<(KernelChannel, JupyterMessage), KernelError> {
        let fail = |error: zeromq::ZmqError| KernelError::Transport(error.to_string());
        let (channel, wire) = tokio::select! {
            wire = self.iopub.recv() => (KernelChannel::Iopub, wire.map_err(fail)?),
            wire = self.shell.recv() => (KernelChannel::Shell, wire.map_err(fail)?),
            wire = self.control.recv() => (KernelChannel::Control, wire.map_err(fail)?),
        };
        Ok((channel, decode(wire, &self.key)?))
    }
}

/// Encode one message into the signed multipart layout.
pub(crate) fn encode(message: &JupyterMessage, key: &str) -> Result<ZmqMessage, KernelError> {
    let header = serde_json::to_vec(&message.header)
        .map_err(|error| KernelError::Protocol(error.to_string()))?;
    let parent = match &message.parent_header {
        Some(parent) => serde_json::to_vec(parent),
        None => serde_json::to_vec(&serde_json::json!({})),
    }
    .map_err(|error| KernelError::Protocol(error.to_string()))?;
    let metadata = serde_json::to_vec(&message.metadata)
        .map_err(|error| KernelError::Protocol(error.to_string()))?;
    let content = serde_json::to_vec(&message.content)
        .map_err(|error| KernelError::Protocol(error.to_string()))?;
    let signature = sign(
        key,
        [
            header.as_slice(),
            parent.as_slice(),
            metadata.as_slice(),
            content.as_slice(),
        ],
    );

    let mut frames: Vec<bytes::Bytes> = Vec::new();
    frames.extend(message.zmq_identities.iter().cloned());
    frames.push(bytes::Bytes::from_static(DELIMITER));
    frames.push(signature.into_bytes().into());
    frames.push(header.into());
    frames.push(parent.into());
    frames.push(metadata.into());
    frames.push(content.into());
    frames.extend(message.buffers.iter().cloned());
    ZmqMessage::try_from(frames).map_err(|_| KernelError::Protocol("empty wire message".to_owned()))
}

/// Decode one signed multipart message, verifying its signature.
pub(crate) fn decode(wire: ZmqMessage, key: &str) -> Result<JupyterMessage, KernelError> {
    let frames = wire.into_vec();
    let delimiter = frames
        .iter()
        .position(|frame| frame.as_ref() == DELIMITER)
        .ok_or_else(|| KernelError::Protocol("no <IDS|MSG> delimiter".to_owned()))?;
    let identities = frames[..delimiter].to_vec();
    let rest = &frames[delimiter + 1..];
    let [signature, header, parent, metadata, content, buffers @ ..] = rest else {
        return Err(KernelError::Protocol("truncated wire message".to_owned()));
    };
    if !verify(key, [header.as_ref(), parent, metadata, content], signature) {
        return Err(KernelError::Transport("signature mismatch".to_owned()));
    }
    let header: Header = serde_json::from_slice(header)
        .map_err(|error| KernelError::Protocol(format!("bad header: {error}")))?;
    let parent: Option<Header> = serde_json::from_slice::<serde_json::Value>(parent)
        .ok()
        .filter(|value| value.get("msg_id").is_some())
        .and_then(|value| serde_json::from_value(value).ok());
    let metadata: serde_json::Value = serde_json::from_slice(metadata).unwrap_or_default();
    let content_value: serde_json::Value = serde_json::from_slice(content)
        .map_err(|error| KernelError::Protocol(format!("bad content: {error}")))?;
    let content = JupyterMessageContent::from_type_and_content(&header.msg_type, content_value)
        .map_err(|error| KernelError::Protocol(format!("bad {}: {error}", header.msg_type)))?;
    Ok(JupyterMessage {
        zmq_identities: identities,
        header,
        parent_header: parent,
        metadata,
        content,
        buffers: buffers.to_vec(),
        channel: None,
    })
}

/// Whether `signature` (hex) is the hmac-sha256 of the four signed frames.
///
/// Compares through `verify_slice`, which is constant-time. Comparing the hex
/// strings directly would return on the first differing byte, leaking through
/// timing how much of a guessed signature was right — enough, over a loopback
/// socket a local process can hammer, to build a valid one byte by byte.
fn verify(key: &str, frames: [&[u8]; 4], signature: &[u8]) -> bool {
    let Ok(mut mac) = hmac::Hmac::<sha2::Sha256>::new_from_slice(key.as_bytes()) else {
        return false;
    };
    for frame in frames {
        mac.update(frame);
    }
    let Some(raw) = unhex(signature) else {
        return false;
    };
    mac.verify_slice(&raw).is_ok()
}

/// Decode an even-length ASCII hex string; `None` if it is not one.
fn unhex(hex: &[u8]) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    hex.chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(text, 16).ok()
        })
        .collect()
}

/// The hex hmac-sha256 over the four signed frames.
fn sign<'a>(key: &str, frames: impl IntoIterator<Item = &'a [u8]>) -> String {
    let Ok(mut mac) = hmac::Hmac::<sha2::Sha256>::new_from_slice(key.as_bytes()) else {
        return String::new(); // hmac accepts any key length; unreachable
    };
    for frame in frames {
        mac.update(frame);
    }
    let digest = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

#[cfg(test)]
mod tests {
    use jupyter_protocol::messaging::KernelInfoRequest;

    use super::*;

    #[test]
    fn encode_decode_round_trips_and_verifies() -> Result<(), KernelError> {
        let message = JupyterMessage::new(KernelInfoRequest {}, None);
        let wire = encode(&message, "secret")?;
        let decoded = decode(wire, "secret")?;
        assert_eq!(decoded.header.msg_id, message.header.msg_id);
        assert_eq!(decoded.header.msg_type, "kernel_info_request");
        assert!(decoded.parent_header.is_none(), "{{}} parent reads as none");
        let tampered = encode(&message, "secret")?;
        assert!(matches!(
            decode(tampered, "other-key"),
            Err(KernelError::Transport(_))
        ));
        Ok(())
    }

    #[test]
    fn a_malformed_or_forged_signature_is_refused() -> Result<(), KernelError> {
        // Verification decodes the hex before comparing, so anything that is
        // not a well-formed signature has to be refused rather than panicking
        // or slipping through.
        let message = JupyterMessage::new(KernelInfoRequest {}, None);
        for forged in [
            String::new(),
            "zz".repeat(32), // not hex
            "ab".repeat(31), // right shape, wrong length
            "a".repeat(63),  // odd length
            "0".repeat(64),  // all zeroes
            "ff".repeat(32), // all ones
        ] {
            let wire = encode(&message, "secret")?;
            let mut frames = wire.into_vec();
            let at = frames
                .iter()
                .position(|frame| frame.as_ref() == DELIMITER)
                .unwrap_or(0)
                + 1;
            frames[at] = bytes::Bytes::from(forged.clone().into_bytes());
            let Ok(tampered) = ZmqMessage::try_from(frames) else {
                continue;
            };
            assert!(
                matches!(decode(tampered, "secret"), Err(KernelError::Transport(_))),
                "a forged signature was accepted: {forged:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn tampered_content_does_not_verify() -> Result<(), KernelError> {
        // The signature covers the content frame, so editing it must break it.
        let message = JupyterMessage::new(KernelInfoRequest {}, None);
        let wire = encode(&message, "secret")?;
        let mut frames = wire.into_vec();
        let at = frames
            .iter()
            .position(|frame| frame.as_ref() == DELIMITER)
            .unwrap_or(0);
        // signature, header, parent, metadata, content
        let content = at + 5;
        assert!(content < frames.len(), "expected a content frame");
        frames[content] = bytes::Bytes::from_static(br#"{"evil":true}"#);
        let Ok(tampered) = ZmqMessage::try_from(frames) else {
            return Ok(());
        };
        assert!(matches!(
            decode(tampered, "secret"),
            Err(KernelError::Transport(_))
        ));
        Ok(())
    }

    #[test]
    fn parent_headers_survive() -> Result<(), KernelError> {
        let parent = JupyterMessage::new(KernelInfoRequest {}, None);
        let child = JupyterMessage::new(KernelInfoRequest {}, Some(&parent));
        let decoded = decode(encode(&child, "k")?, "k")?;
        assert_eq!(
            decoded.parent_header.map(|header| header.msg_id),
            Some(parent.header.msg_id)
        );
        Ok(())
    }

    /// A wire message assembled frame by frame.
    fn frames(parts: &[&[u8]]) -> Result<ZmqMessage, KernelError> {
        let frames: Vec<bytes::Bytes> = parts
            .iter()
            .map(|part| bytes::Bytes::copy_from_slice(part))
            .collect();
        ZmqMessage::try_from(frames).map_err(|_| KernelError::Protocol("empty".to_owned()))
    }

    /// A correctly signed message whose four JSON frames are given verbatim.
    fn signed(key: &str, json: [&[u8]; 4]) -> Result<ZmqMessage, KernelError> {
        let signature = sign(key, json);
        frames(&[
            DELIMITER,
            signature.as_bytes(),
            json[0],
            json[1],
            json[2],
            json[3],
        ])
    }

    #[test]
    fn identities_and_buffers_survive_the_frame_layout() -> Result<(), KernelError> {
        let mut message = JupyterMessage::new(KernelInfoRequest {}, None);
        message.zmq_identities = vec![
            bytes::Bytes::from_static(b"peer-a"),
            bytes::Bytes::from_static(b"peer-b"),
        ];
        message.buffers = vec![bytes::Bytes::from_static(b"\x00raw")];
        let wire = encode(&message, "k")?;
        assert_eq!(wire.get(2).map(AsRef::as_ref), Some(DELIMITER));
        let decoded = decode(wire, "k")?;
        assert_eq!(decoded.zmq_identities, message.zmq_identities);
        assert_eq!(decoded.buffers, message.buffers);
        Ok(())
    }

    #[test]
    fn frames_without_a_delimiter_or_payload_are_protocol_errors() -> Result<(), KernelError> {
        let no_delimiter = frames(&[b"id", b"sig", b"{}", b"{}", b"{}", b"{}"])?;
        assert!(matches!(
            decode(no_delimiter, "k"),
            Err(KernelError::Protocol(reason)) if reason.contains("delimiter")
        ));
        // Delimiter, signature, and only three of the four JSON frames.
        let truncated = frames(&[DELIMITER, b"sig", b"{}", b"{}", b"{}"])?;
        assert!(matches!(
            decode(truncated, "k"),
            Err(KernelError::Protocol(reason)) if reason.contains("truncated")
        ));
        Ok(())
    }

    #[test]
    fn signed_but_malformed_frames_are_protocol_errors() -> Result<(), KernelError> {
        let header = serde_json::to_vec(&JupyterMessage::new(KernelInfoRequest {}, None).header)
            .map_err(|error| KernelError::Protocol(error.to_string()))?;
        let bad_header = signed("k", [b"not json", b"{}", b"{}", b"{}"])?;
        assert!(matches!(
            decode(bad_header, "k"),
            Err(KernelError::Protocol(reason)) if reason.starts_with("bad header")
        ));
        let bad_content = signed("k", [&header, b"{}", b"{}", b"[oops"])?;
        assert!(matches!(
            decode(bad_content, "k"),
            Err(KernelError::Protocol(reason)) if reason.starts_with("bad content")
        ));
        // Valid JSON that does not fit the header's msg_type.
        let header_value = serde_json::json!({
            "msg_id": "m", "username": "u", "session": "s",
            "date": "2024-01-01T00:00:00Z", "msg_type": "execute_reply", "version": "5.3"
        })
        .to_string();
        let wrong_shape = signed("k", [header_value.as_bytes(), b"{}", b"{}", b"[1]"])?;
        assert!(matches!(
            decode(wrong_shape, "k"),
            Err(KernelError::Protocol(reason)) if reason.starts_with("bad execute_reply")
        ));
        // Lenient frames: a non-object parent reads as no parent, and garbage
        // metadata reads as `null`, rather than failing the message.
        let lenient = signed("k", [&header, b"null", b"not json", b"{}"])?;
        let decoded = decode(lenient, "k")?;
        assert!(decoded.parent_header.is_none());
        assert_eq!(decoded.metadata, serde_json::Value::Null);
        Ok(())
    }

    /// The kernel side of a loopback session: ROUTER shell and control, PUB
    /// iopub, all bound to OS-assigned loopback ports.
    struct LoopbackKernel {
        info: ConnectionInfo,
        shell: zeromq::RouterSocket,
        control: zeromq::RouterSocket,
        _iopub: zeromq::PubSocket,
    }

    const KEY: &str = "loopback-key";

    /// Bound the whole loopback exchange, so a hung socket fails the test.
    const LOOPBACK_BOUND: std::time::Duration = std::time::Duration::from_secs(10);

    async fn loopback() -> Result<LoopbackKernel, KernelError> {
        let fail = |error: zeromq::ZmqError| KernelError::Transport(error.to_string());
        let port = |endpoint: zeromq::Endpoint| match endpoint {
            zeromq::Endpoint::Tcp(_, port) => Ok(port),
            other => Err(KernelError::Transport(format!("not tcp: {other}"))),
        };
        let mut shell = zeromq::RouterSocket::new();
        let shell_port = port(shell.bind("tcp://127.0.0.1:0").await.map_err(fail)?)?;
        let mut control = zeromq::RouterSocket::new();
        let control_port = port(control.bind("tcp://127.0.0.1:0").await.map_err(fail)?)?;
        let mut iopub = zeromq::PubSocket::new();
        let iopub_port = port(iopub.bind("tcp://127.0.0.1:0").await.map_err(fail)?)?;
        Ok(LoopbackKernel {
            info: ConnectionInfo {
                ip: "127.0.0.1".to_owned(),
                transport: jupyter_protocol::connection_info::Transport::TCP,
                shell_port,
                iopub_port,
                stdin_port: 0,
                control_port,
                hb_port: 0,
                key: KEY.to_owned(),
                signature_scheme: "hmac-sha256".to_owned(),
                kernel_name: None,
            },
            shell,
            control,
            _iopub: iopub,
        })
    }

    /// Receive one request on a ROUTER and answer it with `reply` (signed
    /// with `key`), returning the decoded request.
    async fn answer(
        socket: &mut zeromq::RouterSocket,
        key: &str,
        reply: impl Fn(&JupyterMessage) -> Vec<JupyterMessage>,
    ) -> Result<JupyterMessage, KernelError> {
        let fail = |error: zeromq::ZmqError| KernelError::Transport(error.to_string());
        let request = decode(socket.recv().await.map_err(fail)?, KEY)?;
        for message in reply(&request) {
            socket.send(encode(&message, key)?).await.map_err(fail)?;
        }
        Ok(request)
    }

    fn reply_to(request: &JupyterMessage) -> Vec<JupyterMessage> {
        vec![JupyterMessage::new(
            jupyter_protocol::messaging::InterruptReply::default(),
            Some(request),
        )]
    }

    #[tokio::test]
    async fn zmq_transport_routes_shell_and_control_replies_by_channel() -> Result<(), KernelError>
    {
        tokio::time::timeout(LOOPBACK_BOUND, async {
            let mut kernel = loopback().await?;
            let mut transport = ZmqTransport::connect(&kernel.info).await?;

            for (channel, socket) in [
                (KernelChannel::Shell, &mut kernel.shell),
                (KernelChannel::Control, &mut kernel.control),
            ] {
                let request = JupyterMessage::new(KernelInfoRequest {}, None);
                transport.send(channel, request.clone()).await?;
                let seen = answer(socket, KEY, reply_to).await?;
                assert_eq!(seen.header.msg_id, request.header.msg_id);
                // The ROUTER saw the dealer's routing identity.
                assert_eq!(seen.zmq_identities.len(), 1);
                let (from, reply) = transport.recv().await?;
                assert_eq!(from, channel);
                assert_eq!(
                    reply.parent_header.map(|header| header.msg_id),
                    Some(request.header.msg_id)
                );
            }

            let refused = transport
                .send(
                    KernelChannel::Iopub,
                    JupyterMessage::new(KernelInfoRequest {}, None),
                )
                .await;
            assert!(
                matches!(refused, Err(KernelError::Protocol(_))),
                "{refused:?}"
            );
            Ok(())
        })
        .await
        .map_err(|_| KernelError::Timeout)?
    }

    #[tokio::test]
    async fn zmq_transport_refuses_a_reply_signed_with_another_key() -> Result<(), KernelError> {
        tokio::time::timeout(LOOPBACK_BOUND, async {
            let mut kernel = loopback().await?;
            let mut transport = ZmqTransport::connect(&kernel.info).await?;
            transport
                .send(
                    KernelChannel::Shell,
                    JupyterMessage::new(KernelInfoRequest {}, None),
                )
                .await?;
            answer(&mut kernel.shell, "forged-key", reply_to).await?;
            let received = transport.recv().await;
            assert!(
                matches!(received, Err(KernelError::Transport(ref reason)) if reason == "signature mismatch"),
                "{received:?}"
            );
            Ok(())
        })
        .await
        .map_err(|_| KernelError::Timeout)?
    }

    #[tokio::test]
    async fn kernel_control_skips_unrelated_replies_and_rejects_forged_ones()
    -> Result<(), KernelError> {
        tokio::time::timeout(LOOPBACK_BOUND, async {
            let mut kernel = loopback().await?;
            let mut control = crate::kernel::KernelControl::connect(&kernel.info).await?;

            // A stale reply to some other request arrives first; interrupt
            // must keep waiting for its own.
            let stale_then_real = |request: &JupyterMessage| {
                let stranger = JupyterMessage::new(KernelInfoRequest {}, None);
                let mut stale = JupyterMessage::new(
                    jupyter_protocol::messaging::InterruptReply::default(),
                    Some(&stranger),
                );
                stale.zmq_identities = request.zmq_identities.clone();
                let mut replies = vec![stale];
                replies.extend(reply_to(request));
                replies
            };
            let (interrupted, seen) = tokio::join!(
                control.interrupt(),
                answer(&mut kernel.control, KEY, stale_then_real)
            );
            interrupted?;
            assert_eq!(seen?.header.msg_type, "interrupt_request");

            // A forged reply surfaces as a transport error, not a hang.
            let (forged, seen) = tokio::join!(
                control.interrupt(),
                answer(&mut kernel.control, "forged-key", reply_to)
            );
            seen?;
            assert!(
                matches!(forged, Err(KernelError::Transport(_))),
                "{forged:?}"
            );
            Ok(())
        })
        .await
        .map_err(|_| KernelError::Timeout)?
    }
}
