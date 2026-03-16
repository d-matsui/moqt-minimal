//! # message: MOQT control message definitions
//!
//! Defines the control messages exchanged over a MOQT session.
//! Each message is sent/received on a bidirectional stream (bidi stream).
//!
//! ## Message frame structure
//! ```text
//! [Message Type (varint)] [Payload Length (u16 BE)] [Payload]
//! ```
//! Note: Payload Length is a fixed 16-bit big-endian integer, not a varint.
//!
//! ## Typical message flow
//! 1. SETUP: Both sides exchange during connection establishment
//! 2. PUBLISH_NAMESPACE: Publisher registers available namespaces
//! 3. SUBSCRIBE: Subscriber requests a specific track
//! 4. SUBSCRIBE_OK / REQUEST_ERROR: Success/failure response to subscription
//! 5. PUBLISH_DONE: Publisher signals end of publishing

pub mod parameter;
pub mod publish_done;
pub mod publish_namespace;
pub mod request_error;
pub mod request_ok;
pub mod setup;
pub mod subscribe;
pub mod subscribe_ok;

use anyhow::{Result, ensure};

use crate::wire::varint::{decode_varint, encode_varint};

/// Message Type IDs (defined in spec Section 9)
pub const MSG_SUBSCRIBE: u64 = 0x03;
pub const MSG_SUBSCRIBE_OK: u64 = 0x04;
pub const MSG_REQUEST_ERROR: u64 = 0x05;
pub const MSG_PUBLISH_NAMESPACE: u64 = 0x06;
pub const MSG_REQUEST_OK: u64 = 0x07;
pub const MSG_PUBLISH_DONE: u64 = 0x0B;
/// SETUP message has a special Type ID (0x2F00).
/// Also used to identify the control stream.
pub const MSG_SETUP: u64 = 0x2F00;

/// Encode a control message frame.
/// Format: [Type (varint)] [Length (u16 BE)] [Payload]
///
/// Length is a fixed u16 (not varint) so the receiver can determine
/// the buffer size in advance.
pub fn encode_message_frame(msg_type: u64, payload: &[u8], buf: &mut Vec<u8>) {
    encode_varint(msg_type, buf);
    let len = payload.len() as u16;
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(payload);
}

/// Decode a control message frame header, returning (message type, payload).
pub fn decode_message_header(buf: &mut &[u8]) -> Result<(u64, Vec<u8>)> {
    let msg_type = decode_varint(buf)?;
    ensure!(buf.len() >= 2, "need 2 bytes for message length");
    let len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
    *buf = &buf[2..];
    ensure!(
        buf.len() >= len,
        "need {len} bytes for message payload, have {}",
        buf.len()
    );
    let payload = buf[..len].to_vec();
    *buf = &buf[len..];
    Ok((msg_type, payload))
}
