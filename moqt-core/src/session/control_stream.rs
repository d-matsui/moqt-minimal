//! # control_stream: MOQT control stream reader/writer
//!
//! In MOQT, each peer opens one unidirectional QUIC stream for control messages.
//! The first message on a control stream is always SETUP.
//! A control stream MUST NOT be closed during the session lifetime.
//!
//! For per-request bidirectional streams, see `request_stream`.

use anyhow::Result;

use quinn::{RecvStream, SendStream};

use crate::message::MSG_SETUP;
use crate::primitives::varint::encode_varint;

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
    /// Writes: stream type (0x2F00) -> payload length -> payload.
    ///
    /// The stream type (SETUP Type ID 0x2F00) must be written first as a varint
    /// to identify the stream kind.
    pub async fn write_setup(&mut self, msg: &crate::message::setup::SetupMessage) -> Result<()> {
        // Write stream type = SETUP (0x2F00) first
        let mut buf = Vec::new();
        encode_varint(MSG_SETUP, &mut buf);

        // Encode SETUP message payload (Setup Options as Key-Value-Pairs)
        let mut payload = Vec::new();
        use crate::primitives::key_value_pair::{KeyValuePair, KvValue, encode_key_value_pairs};
        let kvs: Vec<KeyValuePair> = msg
            .setup_options
            .iter()
            .map(|opt| match opt {
                crate::message::setup::SetupOption::Path(v) => KeyValuePair {
                    type_id: 0x01,
                    value: KvValue::Bytes(v.clone()),
                },
                crate::message::setup::SetupOption::Authority(v) => KeyValuePair {
                    type_id: 0x05,
                    value: KvValue::Bytes(v.clone()),
                },
                crate::message::setup::SetupOption::Unknown { type_id, value } => KeyValuePair {
                    type_id: *type_id,
                    value: value.clone(),
                },
            })
            .collect();
        encode_key_value_pairs(&kvs, &mut payload)?;

        // Write payload length as u16 BE, followed by the payload
        let len = payload.len() as u16;
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(&payload);

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
        crate::session::stream_utils::read_message_frame(&mut self.stream).await
    }
}
