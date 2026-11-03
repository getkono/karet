//! The kernel's control channel: the one deadline every control request
//! waits under, and the out-of-band connection an interrupt travels on.

use std::time::Duration;

use jupyter_protocol::JupyterMessage;
use jupyter_protocol::messaging::JupyterMessageContent;
use zeromq::Socket;
use zeromq::SocketRecv;
use zeromq::SocketSend;

use super::Connection;
use super::KernelError;
use super::transport::decode;
use super::transport::encode;

/// How long a control request (interrupt, shutdown) may wait for its reply.
pub(super) const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);

/// An out-of-band control connection: its own DEALER to the control port, so
/// an interrupt reaches a kernel whose shell traffic is busy running a cell.
pub struct KernelControl {
    key: String,
    socket: zeromq::DealerSocket,
}

impl KernelControl {
    /// Connect to the kernel's control port.
    ///
    /// # Errors
    /// Returns [`KernelError::Transport`] when the socket cannot connect.
    pub async fn connect(connection: &Connection) -> Result<Self, KernelError> {
        let info = connection.info();
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

    async fn request(&mut self, content: JupyterMessageContent) -> Result<(), KernelError> {
        let message = JupyterMessage::new(content, None);
        let request_id = message.header.msg_id.clone();
        let wire = encode(&message, &self.key)?;
        self.socket
            .send(wire)
            .await
            .map_err(|error| KernelError::Transport(error.to_string()))?;
        let deadline = tokio::time::Instant::now() + CONTROL_TIMEOUT;
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
