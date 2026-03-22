//! # control: MOQT control stream reader/writer
//!
//! In MOQT, each peer opens one unidirectional QUIC stream for control messages.
//! The first message on a control stream is always SETUP.
//! A control stream MUST NOT be closed during the session lifetime.
//!
//! For per-request bidirectional streams, see `request`.

use anyhow::Result;

use quinn::{RecvStream, SendStream};

/// Write side of a MOQT control stream.
/// Corresponds to the QUIC unidirectional stream each peer opens.
pub struct ControlStreamWriter {
    stream: SendStream,
}

impl ControlStreamWriter {
    pub fn new(stream: SendStream) -> Self {
        Self { stream }
    }

    /// Write a SETUP message to the control stream.
    /// Delegates encoding to `SetupMessage::encode`.
    pub async fn write_setup(&mut self, msg: &crate::message::setup::SetupMessage) -> Result<()> {
        let mut buf = Vec::new();
        msg.encode(&mut buf)?;
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    /// Write a pre-framed control message (Type + Length + Payload) as-is.
    pub async fn write_raw(&mut self, data: &[u8]) -> Result<()> {
        self.stream.write_all(data).await?;
        Ok(())
    }
}

/// Read side of a MOQT control stream.
pub struct ControlStreamReader {
    stream: RecvStream,
}

impl ControlStreamReader {
    pub fn new(stream: RecvStream) -> Self {
        Self { stream }
    }

    /// Read a SETUP message from the control stream.
    /// Expects stream type (0x2F00) followed by the SETUP message bytes.
    pub async fn read_setup(&mut self) -> Result<crate::message::setup::SetupMessage> {
        let buf = self.read_message_bytes().await?;
        let mut slice = buf.as_slice();
        crate::message::setup::SetupMessage::decode(&mut slice)
    }

    /// Read one control message from the stream, returning the full frame
    /// (Type + Length + Payload) as raw bytes.
    pub async fn read_message_bytes(&mut self) -> Result<Vec<u8>> {
        crate::stream::utils::read_message_frame(&mut self.stream).await
    }
}
