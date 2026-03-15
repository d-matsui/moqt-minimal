use std::io;

use quinn::{RecvStream, SendStream};

use crate::message::MSG_SETUP;
use crate::wire::varint::{decode_varint, encode_varint};

/// Writer for a MOQT control stream (unidirectional, opened by each peer).
pub struct ControlStreamWriter {
    stream: SendStream,
}

impl ControlStreamWriter {
    pub fn new(stream: SendStream) -> Self {
        Self { stream }
    }

    /// Write the SETUP message as the first message on the control stream.
    /// Also writes the stream type (0x2F00) before the SETUP message.
    pub async fn write_setup(
        &mut self,
        msg: &crate::message::setup::SetupMessage,
    ) -> io::Result<()> {
        // Control stream starts with stream type = SETUP (0x2F00)
        let mut buf = Vec::new();
        encode_varint(MSG_SETUP, &mut buf);

        // Then the SETUP message payload (Setup Options as Key-Value-Pairs)
        let mut payload = Vec::new();
        use crate::wire::key_value_pair::{encode_key_value_pairs, KeyValuePair, KvValue};
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
        encode_key_value_pairs(&kvs, &mut payload);

        let len = payload.len() as u16;
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(&payload);

        self.stream.write_all(&buf).await.map_err(io::Error::other)
    }

    /// Write a raw control message (already framed with Type + Length + Payload).
    pub async fn write_raw(&mut self, data: &[u8]) -> io::Result<()> {
        self.stream.write_all(data).await.map_err(io::Error::other)
    }
}

/// Reader for a MOQT control stream (unidirectional, opened by peer).
pub struct ControlStreamReader {
    stream: RecvStream,
}

impl ControlStreamReader {
    pub fn new(stream: RecvStream) -> Self {
        Self { stream }
    }

    /// Read the SETUP message from the control stream.
    /// Expects stream type (0x2F00) followed by the SETUP message.
    pub async fn read_setup(&mut self) -> io::Result<crate::message::setup::SetupMessage> {
        let buf = self.read_message_bytes().await?;
        let mut slice = buf.as_slice();
        crate::message::setup::SetupMessage::decode(&mut slice)
    }

    /// Read a raw control message and return the full framed bytes
    /// (Type + Length + Payload).
    pub async fn read_message_bytes(&mut self) -> io::Result<Vec<u8>> {
        // Read stream type / message type (varint)
        let (_type_val, type_bytes) = self.read_varint().await?;

        // Read length (2 bytes, big-endian u16)
        let mut len_bytes = [0u8; 2];
        self.stream
            .read_exact(&mut len_bytes)
            .await
            .map_err(io::Error::other)?;
        let payload_len = u16::from_be_bytes(len_bytes) as usize;

        // Read payload
        let mut payload = vec![0u8; payload_len];
        if payload_len > 0 {
            self.stream
                .read_exact(&mut payload)
                .await
                .map_err(io::Error::other)?;
        }

        // Reconstruct the full message bytes
        let mut result = Vec::with_capacity(type_bytes.len() + 2 + payload_len);
        result.extend_from_slice(&type_bytes);
        result.extend_from_slice(&len_bytes);
        result.extend_from_slice(&payload);
        Ok(result)
    }

    /// Read a varint from the stream, returning (value, raw_bytes).
    async fn read_varint(&mut self) -> io::Result<(u64, Vec<u8>)> {
        let mut first = [0u8; 1];
        self.stream
            .read_exact(&mut first)
            .await
            .map_err(io::Error::other)?;

        let byte = first[0];
        let total_len = if byte & 0x80 == 0 {
            1
        } else if byte & 0xc0 == 0x80 {
            2
        } else if byte & 0xe0 == 0xc0 {
            3
        } else if byte & 0xf0 == 0xe0 {
            4
        } else if byte & 0xf8 == 0xf0 {
            5
        } else if byte & 0xfc == 0xf8 {
            6
        } else if byte == 0xfc {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid varint code point 0xFC",
            ));
        } else if byte == 0xfe {
            8
        } else {
            9
        };

        let mut raw = vec![0u8; total_len];
        raw[0] = byte;
        if total_len > 1 {
            self.stream
                .read_exact(&mut raw[1..])
                .await
                .map_err(io::Error::other)?;
        }

        let mut slice = raw.as_slice();
        let value = decode_varint(&mut slice)?;
        Ok((value, raw))
    }
}
