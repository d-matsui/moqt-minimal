pub mod parameter;
pub mod publish_done;
pub mod publish_namespace;
pub mod request_error;
pub mod request_ok;
pub mod setup;
pub mod subscribe;
pub mod subscribe_ok;

use std::io;

use crate::wire::varint::{decode_varint, encode_varint};

/// Message type IDs (Section 9)
pub const MSG_SUBSCRIBE: u64 = 0x03;
pub const MSG_SUBSCRIBE_OK: u64 = 0x04;
pub const MSG_REQUEST_ERROR: u64 = 0x05;
pub const MSG_PUBLISH_NAMESPACE: u64 = 0x06;
pub const MSG_REQUEST_OK: u64 = 0x07;
pub const MSG_PUBLISH_DONE: u64 = 0x0B;
pub const MSG_SETUP: u64 = 0x2F00;

/// Encode a control message frame: Type (vi64) + Length (16-bit) + Payload.
pub fn encode_message_frame(msg_type: u64, payload: &[u8], buf: &mut Vec<u8>) {
    encode_varint(msg_type, buf);
    let len = payload.len() as u16;
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(payload);
}

/// Decode a control message frame header: returns (message_type, payload_bytes).
/// The caller is responsible for reading exactly `payload.len()` bytes.
pub fn decode_message_header(buf: &mut &[u8]) -> io::Result<(u64, Vec<u8>)> {
    let msg_type = decode_varint(buf)?;
    if buf.len() < 2 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "need 2 bytes for message length",
        ));
    }
    let len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
    *buf = &buf[2..];
    if buf.len() < len {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("need {len} bytes for message payload, have {}", buf.len()),
        ));
    }
    let payload = buf[..len].to_vec();
    *buf = &buf[len..];
    Ok((msg_type, payload))
}
