//! The seam between the broker skeleton and the messages it brokers.
//!
//! The skeleton in [`super::serve`] owns process management — client identity,
//! the pending-request map, request-id rewriting, fanout and retirement — and
//! knows nothing about the wire vocabulary. [`BrokerProtocol`] supplies that
//! vocabulary: what a client message means, where a server message goes, and
//! what state the broker must keep on the protocol's behalf.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::Value;

use crate::broker::framing::Framing;
use crate::broker::serve::Core;

/// Identifier the skeleton assigns to each connected client.
pub type ClientId = u64;

/// What the skeleton does with a message a client just sent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ClientFlow<T> {
    /// Rewrite the request id, record this client as its owner, send upstream.
    Proxy(T),
    /// Send upstream unchanged (a notification, or a reply to the server).
    Forward,
    /// Swallow it; the hook has already answered the client if it needed to.
    Drop,
    /// Swallow it and end this client's session.
    Stop,
}

/// Where a server-originated message goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ServerRoute {
    /// Deliver to one client only — the first still connected.
    SingleClient,
    /// Deliver a copy to every connected client.
    AllClients,
    /// Deliver to nobody.
    Discard,
}

/// The message semantics a broker skeleton is parameterised with.
///
/// Implementors are zero-sized markers: every hook is an associated function,
/// so the skeleton dispatches statically and never stores a protocol value.
/// Hook futures are spelled `impl Future<Output = _> + Send` because the
/// skeleton `tokio::spawn`s the tasks that await them.
pub trait BrokerProtocol: Sized + Send + Sync + 'static {
    /// Broker-wide state the protocol keeps; the skeleton owns its lifetime.
    type State: Default + Send + Sync + 'static;
    /// Per-request payload the skeleton parks in the pending map.
    type RequestTag: Default + Send + Sync + 'static;
    /// How messages are framed on both the client and the server transports.
    type Framing: Framing;

    /// Directory under the state root holding endpoint and lock files.
    const STATE_DIR: &'static str;
    /// Bumped whenever brokers of different builds must not be shared.
    const PROTOCOL_VERSION: &'static str;
    /// Environment flag selecting the hidden broker entry point.
    const MODE_ENV: &'static str;
    /// Environment variable carrying the encoded broker specification.
    const SPEC_ENV: &'static str;
    /// Literal prefix of the client authentication line, token excluded.
    const PRELUDE: &'static str;
    /// Human-readable name used in the hidden process's error output.
    const DISPLAY_NAME: &'static str;

    /// Decide what happens to one client-to-server message.
    ///
    /// The hook may rewrite `message` in place and may answer the client
    /// directly through `link` before returning [`ClientFlow::Drop`].
    fn on_client_message(
        message: &mut Value,
        link: &ClientLink<'_, Self>,
    ) -> impl Future<Output = ClientFlow<Self::RequestTag>> + Send;

    /// Route a server message that is not a reply to a proxied request.
    fn route_server_message(message: &Value) -> ServerRoute;

    /// Inspect a reply before its original request id is restored.
    fn on_response(
        _message: &mut Value,
        _tag: &Self::RequestTag,
        _link: &ServerLink<'_, Self>,
    ) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }

    /// React to a client session ending, after it left the client map.
    fn on_client_gone(_link: &ClientLink<'_, Self>) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }

    /// Say goodbye to the server before the broker exits cleanly.
    fn retire(_link: &ServerLink<'_, Self>) -> impl Future<Output = ()> + Send {
        std::future::ready(())
    }
}

/// A hook's handle on the broker as a whole.
///
/// Handed to hooks that have no originating client — a server reply, or
/// retirement.
pub struct ServerLink<'a, P: BrokerProtocol> {
    core: &'a Arc<Core<P>>,
}

impl<'a, P: BrokerProtocol> ServerLink<'a, P> {
    pub(crate) fn new(core: &'a Arc<Core<P>>) -> Self {
        Self { core }
    }

    /// The protocol's broker-wide state.
    #[must_use]
    pub fn state(&self) -> &Arc<P::State> {
        &self.core.state
    }

    /// Queue a message for the server, dropping it if the queue is gone.
    pub async fn send_upstream(&self, message: Value) {
        let _ = self.core.upstream.send(message).await;
    }

    /// Reserve a request id nobody else will use on this broker.
    #[must_use]
    pub fn next_request_id(&self) -> u64 {
        self.core.next_request.fetch_add(1, Ordering::Relaxed)
    }
}

/// A hook's handle on the one client it is serving.
pub struct ClientLink<'a, P: BrokerProtocol> {
    core: &'a Arc<Core<P>>,
    client: ClientId,
    sender: &'a tokio::sync::mpsc::Sender<Value>,
}

impl<'a, P: BrokerProtocol> ClientLink<'a, P> {
    pub(crate) fn new(
        core: &'a Arc<Core<P>>,
        client: ClientId,
        sender: &'a tokio::sync::mpsc::Sender<Value>,
    ) -> Self {
        Self {
            core,
            client,
            sender,
        }
    }

    /// Identifier of the client this hook is serving.
    #[must_use]
    pub fn client(&self) -> ClientId {
        self.client
    }

    /// The protocol's broker-wide state.
    #[must_use]
    pub fn state(&self) -> &Arc<P::State> {
        &self.core.state
    }

    /// Answer this client directly, bypassing the server.
    pub async fn reply(&self, message: Value) {
        let _ = self.sender.send(message).await;
    }

    /// Queue a message for the server, dropping it if the queue is gone.
    pub async fn send_upstream(&self, message: Value) {
        let _ = self.core.upstream.send(message).await;
    }

    /// Reserve a request id nobody else will use on this broker.
    #[must_use]
    pub fn next_request_id(&self) -> u64 {
        self.core.next_request.fetch_add(1, Ordering::Relaxed)
    }
}
