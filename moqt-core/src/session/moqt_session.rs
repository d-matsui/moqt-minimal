//! # moqt_session: MOQT session over a QUIC connection
//!
//! Wraps a QUIC connection with completed SETUP exchange.
//! Provides the entry point for all MOQT protocol operations.

use anyhow::Result;
use quinn::Connection;

use crate::message::setup::{SetupMessage, SetupOption};
use crate::session::control_stream::{ControlStreamReader, ControlStreamWriter};

/// A MOQT session over a QUIC connection.
/// Created after SETUP exchange is complete.
pub struct MoqtSession {
    connection: Connection,
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

        Ok(Self { connection })
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

        Ok(Self { connection })
    }

    /// Get a reference to the underlying QUIC connection.
    /// Used by relay/pub/sub for stream operations not yet abstracted.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }
}
