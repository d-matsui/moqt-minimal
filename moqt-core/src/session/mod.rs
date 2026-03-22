//! # session: MOQT session management
//!
//! Protocol logic and high-level API for MOQT sessions.
//!
//! The main entry points are `MoqtSession` (session lifecycle) and
//! `SessionEvent` (incoming events). Other types represent individual
//! protocol interactions:
//!
//! - `subgroup`: High-level Subgroup reader/writer (hides ObjectHeader)
//! - `subscribe_request`: Incoming SUBSCRIBE request handler
//! - `publish_namespace_request`: Incoming PUBLISH_NAMESPACE request handler
//! - `subscription`: Established subscription state

pub mod publish_namespace_request;
pub mod subgroup;
pub mod subscribe_request;
pub mod subscription;

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Result, bail};
use quinn::Connection;

use crate::session::publish_namespace_request::PublishNamespaceRequest;
use crate::session::subgroup::{SubgroupReader, SubgroupWriter};
use crate::session::subscribe_request::SubscribeRequest;
use crate::session::subscription::Subscription;
use crate::stream::control::{ControlStreamReader, ControlStreamWriter};
use crate::stream::data::{DataStreamReader, DataStreamWriter};
use crate::stream::request::{RequestMessage, RequestStreamReader, RequestStreamWriter};
use crate::wire::parameter::MessageParameter;
use crate::wire::publish_namespace::PublishNamespaceMessage;
use crate::wire::setup::{SetupMessage, SetupOption};
use crate::wire::subgroup_header::SubgroupHeader;
use crate::wire::subscribe::SubscribeMessage;
use crate::wire::track_namespace::TrackNamespace;

// === RequestIdAllocator ===

/// Request ID allocator.
///
/// MOQT assigns a unique ID to each request (SUBSCRIBE, PUBLISH_NAMESPACE, etc.).
/// Even/odd parity distinguishes the originator:
/// - Client (publisher/subscriber): even (0, 2, 4, ...)
/// - Server (relay): odd (1, 3, 5, ...)
///
/// This scheme ensures both sides can independently generate IDs without collision.
/// Uses atomic operations so it can be shared across tasks via `&self`.
struct RequestIdAllocator {
    next_id: AtomicU64,
}

impl RequestIdAllocator {
    /// Create a client allocator (even IDs: 0, 2, 4, ...).
    fn client() -> Self {
        Self {
            next_id: AtomicU64::new(0),
        }
    }

    /// Create a server allocator (odd IDs: 1, 3, 5, ...).
    fn server() -> Self {
        Self {
            next_id: AtomicU64::new(1),
        }
    }

    /// Allocate the next request ID. Increments by 2 each time.
    fn allocate(&self) -> u64 {
        self.next_id.fetch_add(2, Ordering::Relaxed)
    }
}

// === SessionEvent ===

/// An event received on the session.
pub enum SessionEvent {
    /// A SUBSCRIBE request was received on a bidi stream.
    Subscribe(SubscribeRequest),
    /// A PUBLISH_NAMESPACE request was received on a bidi stream.
    PublishNamespace(PublishNamespaceRequest),
    /// A data stream was received on a uni stream.
    DataStream(SubgroupReader),
}

// === MoqtSession ===

/// A MOQT session over a QUIC connection.
/// Created after SETUP exchange is complete.
///
/// Holds the two control streams (one per peer) for the session lifetime.
/// Dropping the writer would send FIN, which is a protocol violation (Section 3.3).
pub struct MoqtSession {
    connection: Connection,
    request_id_alloc: RequestIdAllocator,
    /// Writer for this peer's control stream (must not be dropped).
    _ctrl_writer: ControlStreamWriter,
    /// Reader for the peer's control stream (for future GOAWAY reception).
    _ctrl_reader: ControlStreamReader,
}

impl MoqtSession {
    /// Establish a client-side MOQT session.
    /// Sends SETUP with Path and Authority options, then receives the server's SETUP.
    pub async fn connect(connection: Connection) -> Result<Self> {
        let ctrl_send = connection.open_uni().await?;
        let mut ctrl_writer = ControlStreamWriter::new(ctrl_send);
        let setup = SetupMessage {
            setup_options: vec![
                SetupOption::Path(b"/".to_vec()),
                SetupOption::Authority(b"localhost".to_vec()),
            ],
        };
        ctrl_writer.write_setup(&setup).await?;

        let recv = connection.accept_uni().await?;
        let mut ctrl_reader = ControlStreamReader::new(recv);
        let _server_setup = ctrl_reader.read_setup().await?;

        Ok(Self {
            connection,
            request_id_alloc: RequestIdAllocator::client(),
            _ctrl_writer: ctrl_writer,
            _ctrl_reader: ctrl_reader,
        })
    }

    /// Establish a server-side MOQT session.
    /// Sends an empty SETUP, then receives the client's SETUP.
    pub async fn accept(connection: Connection) -> Result<Self> {
        let ctrl_send = connection.open_uni().await?;
        let mut ctrl_writer = ControlStreamWriter::new(ctrl_send);
        let setup = SetupMessage {
            setup_options: vec![],
        };
        ctrl_writer.write_setup(&setup).await?;

        let recv = connection.accept_uni().await?;
        let mut ctrl_reader = ControlStreamReader::new(recv);
        let _client_setup = ctrl_reader.read_setup().await?;

        Ok(Self {
            connection,
            request_id_alloc: RequestIdAllocator::server(),
            _ctrl_writer: ctrl_writer,
            _ctrl_reader: ctrl_reader,
        })
    }

    /// Register a namespace with the peer.
    /// Opens a bidi stream, sends PUBLISH_NAMESPACE, and waits for REQUEST_OK.
    /// Returns an error if the peer responds with REQUEST_ERROR.
    pub async fn publish_namespace(&self, namespace: TrackNamespace) -> Result<()> {
        let (send, recv) = self.connection.open_bi().await?;
        let mut writer = RequestStreamWriter::new(send);
        let mut reader = RequestStreamReader::new(recv);

        let msg = PublishNamespaceMessage {
            request_id: self.request_id_alloc.allocate(),
            required_request_id_delta: 0,
            track_namespace: namespace,
        };
        writer.write_publish_namespace(&msg).await?;

        let response = reader.read_message().await?;
        match response {
            RequestMessage::RequestOk(_) => Ok(()),
            RequestMessage::RequestError(err) => {
                bail!(
                    "PUBLISH_NAMESPACE rejected: {}",
                    String::from_utf8_lossy(&err.reason_phrase.value)
                )
            }
            _ => bail!("unexpected response to PUBLISH_NAMESPACE"),
        }
    }

    /// Subscribe to a track.
    /// Opens a bidi stream, sends SUBSCRIBE, and waits for SUBSCRIBE_OK.
    /// Returns a `Subscription` that can be used to receive PUBLISH_DONE.
    pub async fn subscribe(
        &self,
        namespace: TrackNamespace,
        track_name: &str,
        parameters: Vec<MessageParameter>,
    ) -> Result<Subscription> {
        let (send, recv) = self.connection.open_bi().await?;
        let mut writer = RequestStreamWriter::new(send);
        let mut reader = RequestStreamReader::new(recv);

        let msg = SubscribeMessage {
            request_id: self.request_id_alloc.allocate(),
            required_request_id_delta: 0,
            track_namespace: namespace,
            track_name: track_name.as_bytes().to_vec(),
            parameters,
        };
        writer.write_subscribe(&msg).await?;

        let response = reader.read_message().await?;
        match response {
            RequestMessage::SubscribeOk(ok) => Ok(Subscription::new(ok, reader)),
            RequestMessage::RequestError(err) => {
                bail!(
                    "SUBSCRIBE rejected: {}",
                    String::from_utf8_lossy(&err.reason_phrase.value)
                )
            }
            _ => bail!("unexpected response to SUBSCRIBE"),
        }
    }

    /// Wait for the next event on this session.
    /// Concurrently waits for a bidi request or a uni data stream.
    /// Only `accept_bi()` / `accept_uni()` are inside the `select!`,
    /// so cancellation of the losing branch is safe (no data consumed).
    pub async fn next_event(&self) -> Result<SessionEvent> {
        tokio::select! {
            bi = self.connection.accept_bi() => {
                let (send, recv) = bi?;
                let writer = RequestStreamWriter::new(send);
                let mut reader = RequestStreamReader::new(recv);
                let msg = reader.read_message().await?;
                match msg {
                    RequestMessage::Subscribe(sub) => {
                        Ok(SessionEvent::Subscribe(SubscribeRequest::new(sub, writer)))
                    }
                    RequestMessage::PublishNamespace(pub_ns) => {
                        Ok(SessionEvent::PublishNamespace(
                            PublishNamespaceRequest::new(pub_ns, writer),
                        ))
                    }
                    _ => bail!("unexpected message on request stream"),
                }
            }
            uni = self.connection.accept_uni() => {
                let recv = uni?;
                let mut reader = DataStreamReader::new(recv);
                let (header, _raw) = reader.read_subgroup_header().await?;
                Ok(SessionEvent::DataStream(SubgroupReader::new(header, reader)))
            }
        }
    }

    /// Open a subgroup for writing objects.
    /// Creates a data stream with the given SubgroupHeader fields
    /// and returns a `SubgroupWriter` that accepts payloads only.
    pub async fn open_subgroup(
        &self,
        track_alias: u64,
        group_id: u64,
        subgroup_id: u64,
    ) -> Result<SubgroupWriter> {
        let header = SubgroupHeader {
            track_alias,
            group_id,
            has_properties: false,
            end_of_group: true,
            subgroup_id: Some(subgroup_id),
            publisher_priority: None,
        };
        let writer = self.open_data_stream(&header).await?;
        Ok(SubgroupWriter::new(writer))
    }

    /// Open an outgoing data stream (unidirectional).
    /// Writes the SubgroupHeader and returns a DataStreamWriter
    /// for writing subsequent Objects.
    /// For low-level access (e.g. relay pass-through). Prefer `open_subgroup` for clients.
    pub async fn open_data_stream(&self, header: &SubgroupHeader) -> Result<DataStreamWriter> {
        let uni = self.connection.open_uni().await?;
        let mut writer = DataStreamWriter::new(uni);
        writer.write_subgroup_header(header).await?;
        Ok(writer)
    }

    /// Close the session (send QUIC CONNECTION_CLOSE).
    pub fn close(&self) {
        self.connection.close(0u32.into(), b"done");
    }

    /// Get a reference to the underlying QUIC connection.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_generates_even() {
        let alloc = RequestIdAllocator::client();
        assert_eq!(alloc.allocate(), 0);
        assert_eq!(alloc.allocate(), 2);
        assert_eq!(alloc.allocate(), 4);
    }

    #[test]
    fn server_generates_odd() {
        let alloc = RequestIdAllocator::server();
        assert_eq!(alloc.allocate(), 1);
        assert_eq!(alloc.allocate(), 3);
        assert_eq!(alloc.allocate(), 5);
    }
}
