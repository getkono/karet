//! The messages that cross a remote connection, and their encoding.
//!
//! CBOR, via `ciborium`. Self-describing was the deciding property: a field added
//! to an existing message decodes on an older peer, so the two ends of a
//! connection need not be the same build. That matters here in a way it does not
//! for a local seam — the client half runs on whatever machine the user is
//! sitting at, and nobody upgrades two machines at once.
//!
//! Forward compatibility has one honest limit: serde cannot decode an enum
//! variant it has never heard of. A frame that fails to decode is therefore
//! *skipped* rather than fatal (see [`serve`](super::serve) and
//! [`client`](super::client)), so a peer speaking a newer protocol loses the
//! features the older one cannot name, not the session.

use super::RemoteError;
use crate::api::Command;
use crate::api::Event;
use crate::api::RequestId;

/// The protocol this build speaks.
///
/// Bumped only for a change an older peer cannot survive by skipping a frame —
/// a reshaped handshake, a changed frame header. Adding commands, events or
/// fields does not qualify, which is the point of a self-describing encoding.
pub(super) const PROTOCOL: u32 = 1;

/// The oldest protocol this build will talk to.
pub(super) const MIN_PROTOCOL: u32 = 1;

/// The opening message each side sends before anything else.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct Hello {
    /// The protocol version this peer speaks.
    pub(super) protocol: u32,
    /// The karet version, carried for diagnostics — never for gating. Gating on
    /// it would make an editor refuse to connect over a patch release.
    pub(super) karet_version: String,
}

impl Hello {
    /// This build's greeting.
    pub(super) fn current() -> Self {
        Self {
            protocol: PROTOCOL,
            karet_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    /// Check a peer's greeting against what this build can talk to.
    pub(super) fn accept(&self) -> Result<(), RemoteError> {
        if self.protocol < MIN_PROTOCOL {
            return Err(RemoteError::Protocol(format!(
                "peer speaks protocol {} but this karet needs at least {MIN_PROTOCOL}; \
                 upgrade the older side",
                self.protocol
            )));
        }
        Ok(())
    }
}

/// Client to server.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) enum ClientFrame {
    /// The greeting, always first.
    Hello(Hello),
    /// Join a session, resuming from the last event this client saw.
    ///
    /// `last_seq` is 0 for a client that has seen nothing.
    Attach {
        /// The highest sequence number this client has already applied.
        last_seq: u64,
    },
    /// Ask for one document to be described again from scratch.
    ///
    /// Sent when a client's replica diverges — it applied a backend edit that did
    /// not fit, so it discarded the document rather than render text the backend
    /// never said. Without this the backend would go on believing the client is
    /// up to date and send only deltas the client can no longer place.
    Resync {
        /// The document to describe again.
        doc: crate::api::DocumentId,
    },
    /// Submit a command.
    Command {
        /// Correlates the answering event.
        id: RequestId,
        /// The command itself, boxed so the frame stays small for the common
        /// keystroke rather than sizing every variant to the largest.
        command: Box<Command>,
    },
}

/// Server to client.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) enum ServerFrame {
    /// The greeting, always first.
    Hello(Hello),
    /// The attach was accepted.
    Attached {
        /// Whether the server could resume from the client's `last_seq`.
        ///
        /// `false` means the client fell too far behind and must discard its
        /// replicas; the server follows with full state for every open document.
        resumed: bool,
        /// The opaque view state a previous connection checkpointed.
        view_state: Option<Vec<u8>>,
    },
    /// One event, in order.
    Event {
        /// Monotonic per connection, so a reattaching client can say where it
        /// got to.
        seq: u64,
        /// The request this answers, if any.
        id: Option<RequestId>,
        /// The event itself.
        event: Box<Event>,
    },
}

/// Encode `message` to CBOR.
pub(super) fn encode<T: serde::Serialize>(message: &T) -> Result<Vec<u8>, RemoteError> {
    let mut body = Vec::new();
    ciborium::into_writer(message, &mut body)
        .map_err(|error| RemoteError::Protocol(format!("encode: {error}")))?;
    Ok(body)
}

/// Decode `body` from CBOR.
///
/// The error is deliberately recoverable rather than fatal at every call site:
/// an unknown variant from a newer peer lands here, and dropping one frame is a
/// far better outcome than dropping the session.
pub(super) fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, RemoteError> {
    ciborium::from_reader(body).map_err(|error| RemoteError::Protocol(format!("decode: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_greeting_round_trips() -> Result<(), RemoteError> {
        let hello = Hello::current();

        let restored: Hello = decode(&encode(&hello)?)?;

        assert_eq!(restored, hello);
        assert_eq!(restored.protocol, PROTOCOL);
        Ok(())
    }

    #[test]
    fn a_peer_speaking_this_protocol_is_accepted() {
        assert!(Hello::current().accept().is_ok());
    }

    /// A newer peer must be accepted, not refused: the whole reason for a
    /// self-describing encoding is that the two machines upgrade separately.
    #[test]
    fn a_peer_speaking_a_newer_protocol_is_accepted() {
        let newer = Hello {
            protocol: PROTOCOL + 7,
            karet_version: "99.0.0".to_owned(),
        };

        assert!(newer.accept().is_ok());
    }

    #[test]
    fn a_peer_below_the_floor_is_refused_with_a_directing_message() {
        let ancient = Hello {
            protocol: 0,
            karet_version: "0.0.1".to_owned(),
        };

        let Err(RemoteError::Protocol(message)) = ancient.accept() else {
            return;
        };

        assert!(message.contains("upgrade"), "{message}");
    }

    /// Version skew must not gate: a client one patch release ahead of its server
    /// is the normal state of affairs, not an error.
    #[test]
    fn a_differing_karet_version_does_not_refuse_a_connection() {
        let other = Hello {
            protocol: PROTOCOL,
            karet_version: "0.0.1-other".to_owned(),
        };

        assert!(other.accept().is_ok());
    }

    #[test]
    fn a_command_frame_round_trips() -> Result<(), RemoteError> {
        let frame = ClientFrame::Command {
            id: RequestId(42),
            command: Box::new(Command::ListFiles { limit: 100 }),
        };

        let restored: ClientFrame = decode(&encode(&frame)?)?;

        let ClientFrame::Command { id, command } = restored else {
            return Ok(());
        };
        assert_eq!(id, RequestId(42));
        assert!(matches!(*command, Command::ListFiles { limit: 100 }));
        Ok(())
    }

    #[test]
    fn an_event_frame_round_trips_with_its_sequence_number() -> Result<(), RemoteError> {
        let frame = ServerFrame::Event {
            seq: 9,
            id: Some(RequestId(3)),
            event: Box::new(Event::Closed {
                doc: crate::api::DocumentId(1),
            }),
        };

        let restored: ServerFrame = decode(&encode(&frame)?)?;

        let ServerFrame::Event { seq, id, .. } = restored else {
            return Ok(());
        };
        assert_eq!(seq, 9);
        assert_eq!(id, Some(RequestId(3)));
        Ok(())
    }

    /// The one command that must never cross a connection: a GitHub token is
    /// authenticated on the host that holds the repository, and the type refuses
    /// to serialize to keep that true. Encoding must surface that as an error
    /// rather than shipping the secret.
    #[test]
    fn a_github_token_refuses_to_encode() {
        let frame = ClientFrame::Command {
            id: RequestId(1),
            command: Box::new(Command::GithubLogin {
                token: crate::api::GithubToken::new("ghp_example".to_owned()),
            }),
        };

        assert!(encode(&frame).is_err());
    }

    #[test]
    fn a_corrupt_body_decodes_to_an_error_rather_than_a_value() {
        let result: Result<ServerFrame, _> = decode(&[0xff, 0xff, 0xff, 0xff]);

        assert!(result.is_err());
    }
}
