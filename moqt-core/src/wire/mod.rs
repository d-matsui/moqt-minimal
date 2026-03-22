//! # wire: MOQT wire format definitions
//!
//! All protocol structures that are encoded/decoded on the wire.
//! Includes both control messages (Section 9) and data stream headers (Section 10),
//! as well as the primitive types they are built from.
//!
//! ## Primitives (building blocks)
//! - `varint`: Variable-length integer encoding/decoding
//! - `track_namespace`: Track namespace (identifies media published by a publisher)
//! - `key_value_pair`: Key-value pairs used in SETUP messages, etc.
//! - `reason_phrase`: Length-prefixed UTF-8 string for error reasons
//!
//! ## Control messages (Section 9)
//! - `setup`: Session initialization
//! - `publish_namespace`: Namespace registration
//! - `subscribe` / `subscribe_ok`: Track subscription
//! - `request_ok` / `request_error`: Generic request response
//! - `publish_done`: End of publishing
//! - `parameter`: Message parameters (subscription filter, etc.)
//!
//! ## Data stream headers (Section 10)
//! - `subgroup_header`: Stream header (track alias, group ID, subgroup ID)
//! - `object`: Object header (ID delta, payload length)
//!
//! ## Message framing
//! ```text
//! Message Type (vi64),
//! Payload Length (u16),
//! Payload (..)
//! ```
//! Note: Payload Length is a fixed 16-bit big-endian integer, not a varint.

// Primitives
pub mod key_value_pair;
pub mod reason_phrase;
pub mod track_namespace;
pub mod varint;

// Control messages
pub mod parameter;
pub mod publish_done;
pub mod publish_namespace;
pub mod request_error;
pub mod request_ok;
pub mod setup;
pub mod subscribe;
pub mod subscribe_ok;

// Data stream headers
pub mod object;
pub mod subgroup_header;

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

/// Encode a control message.
///
/// Length is a fixed u16 (not varint) so the receiver can determine
/// the buffer size in advance.
pub fn encode_message(msg_type: u64, payload: &[u8], buf: &mut Vec<u8>) {
    encode_varint(msg_type, buf);
    let len = payload.len() as u16;
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(payload);
}

/// Decode a control message, returning (message type, payload).
pub fn decode_message(buf: &mut &[u8]) -> Result<(u64, Vec<u8>)> {
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
