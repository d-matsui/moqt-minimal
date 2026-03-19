//! # moqt_session: MOQT session over a QUIC connection
//!
//! Wraps a QUIC connection with completed SETUP exchange.
//! Provides the entry point for all MOQT protocol operations.

use anyhow::{Result, bail};
use quinn::Connection;

use crate::message::publish_namespace::PublishNamespaceMessage;
use crate::message::setup::{SetupMessage, SetupOption};
use crate::primitives::track_namespace::TrackNamespace;
use crate::session::control_stream::{ControlStreamReader, ControlStreamWriter};
use crate::session::request_id::RequestIdAllocator;
use crate::session::request_stream::{RequestMessage, RequestStreamReader, RequestStreamWriter};

/// A MOQT session over a QUIC connection.
/// Created after SETUP exchange is complete.
pub struct MoqtSession {
    connection: Connection,
    request_id_alloc: RequestIdAllocator,
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
        })
    }

    /// Register a namespace with the peer.
    /// Opens a bidi stream, sends PUBLISH_NAMESPACE, and waits for REQUEST_OK.
    /// Returns an error if the peer responds with REQUEST_ERROR.
    pub async fn publish_namespace(&mut self, namespace: TrackNamespace) -> Result<()> {
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

    /// Get a reference to the underlying QUIC connection.
    /// Used by relay/pub/sub for stream operations not yet abstracted.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}
