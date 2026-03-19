//! # stream_utils: Shared utilities for reading from QUIC streams
//!
//! Provides low-level readers that work with any `quinn::RecvStream`:
//! - `read_varint`: Read a single varint incrementally (used by all stream types)
//! - `read_message_frame`: Read one message frame (Type + Length + Payload),
//!   shared by `ControlStreamReader` and `RequestStreamReader`

use anyhow::Result;
use quinn::RecvStream;

use crate::primitives::varint::{decode_varint, varint_byte_length};

/// Read a single varint from a QUIC stream.
/// Returns (decoded value, raw bytes).
///
/// Since QUIC streams deliver data incrementally, this reads the first byte
/// to determine the varint length, then reads the remaining bytes.
pub async fn read_varint(stream: &mut RecvStream) -> Result<(u64, Vec<u8>)> {
    // Read the first byte to determine total length
    let mut first = [0u8; 1];
    stream.read_exact(&mut first).await?;

    let total_len = varint_byte_length(first[0])?;

    // Read remaining bytes
    let mut raw = vec![0u8; total_len];
    raw[0] = first[0];
    if total_len > 1 {
        stream.read_exact(&mut raw[1..]).await?;
    }

    // Decode the complete varint
    let mut slice = raw.as_slice();
    let value = decode_varint(&mut slice)?;
    Ok((value, raw))
}

/// Read one message frame (Type + Length + Payload) from a QUIC stream.
/// Returns the concatenated raw bytes.
///
/// Frame format:
/// 1. Message type (varint)
/// 2. Payload length (2-byte big-endian u16)
/// 3. Payload bytes
pub async fn read_message_frame(stream: &mut RecvStream) -> Result<Vec<u8>> {
    // Read message type (varint)
    let (_type_val, type_bytes) = read_varint(stream).await?;

    // Read payload length (2-byte big-endian u16)
    let mut len_bytes = [0u8; 2];
    stream.read_exact(&mut len_bytes).await?;
    let payload_len = u16::from_be_bytes(len_bytes) as usize;

    // Read payload body
    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        stream.read_exact(&mut payload).await?;
    }

    // Concatenate all bytes for the caller to decode
    let mut result = Vec::with_capacity(type_bytes.len() + 2 + payload_len);
    result.extend_from_slice(&type_bytes);
    result.extend_from_slice(&len_bytes);
    result.extend_from_slice(&payload);
    Ok(result)
}
