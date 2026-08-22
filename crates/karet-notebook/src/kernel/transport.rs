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

/// An out-of-band control connection: its own DEALER to the control port, so
/// an interrupt reaches a kernel whose shell traffic is busy running a cell.
pub struct KernelControl {
    key: String,
    socket: zeromq::DealerSocket,
}

/// How long control requests may wait for their reply.
const CONTROL_REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

impl KernelControl {
    /// Connect to the kernel's control port.
    ///
    /// # Errors
    /// Returns [`KernelError::Transport`] when the socket cannot connect.
    pub async fn connect(info: &ConnectionInfo) -> Result<Self, KernelError> {
        let mut socket = zeromq::DealerSocket::new();
        socket
            .connect(&format!("tcp://{}:{}", info.ip, info.control_port))
            .await
            .map_err(|error| KernelError::Transport(error.to_string()))?;
        Ok(Self {
            key: info.key.clone(),
            socket,
        })
    }

    /// Interrupt the running cell (message mode).
    ///
    /// # Errors
    /// Returns [`KernelError::Timeout`] if the kernel never acknowledges.
    pub async fn interrupt(&mut self) -> Result<(), KernelError> {
        self.request(jupyter_protocol::messaging::InterruptRequest {}.into())
            .await
    }

    /// Ask the kernel to exit (or restart itself when `restart`).
    ///
    /// # Errors
    /// Returns [`KernelError::Timeout`] if the kernel never acknowledges.
    pub async fn shutdown(&mut self, restart: bool) -> Result<(), KernelError> {
        self.request(jupyter_protocol::messaging::ShutdownRequest { restart }.into())
            .await
    }

    async fn request(&mut self, content: JupyterMessageContent) -> Result<(), KernelError> {
        let message = JupyterMessage::new(content, None);
        let request_id = message.header.msg_id.clone();
        let wire = encode(&message, &self.key)?;
        self.socket
            .send(wire)
            .await
            .map_err(|error| KernelError::Transport(error.to_string()))?;
        let deadline = tokio::time::Instant::now() + CONTROL_REPLY_TIMEOUT;
        loop {
            let wire = tokio::time::timeout_at(deadline, self.socket.recv())
                .await
                .map_err(|_| KernelError::Timeout)?
                .map_err(|error| KernelError::Transport(error.to_string()))?;
            let reply = decode(wire, &self.key)?;
            if reply
                .parent_header
                .as_ref()
                .is_some_and(|header| header.msg_id == request_id)
            {
                return Ok(());
            }
        }
    }
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
}
