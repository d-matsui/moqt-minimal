//! # request_stream: MOQT request stream reader/writer
//!
//! In MOQT, each request (SUBSCRIBE, PUBLISH_NAMESPACE, etc.) is carried on
//! a dedicated bidirectional QUIC stream. This module provides reader/writer
//! wrappers for those bidi streams.
//!
//! The frame format (varint type, u16 length, payload) is the same as control
//! streams. However, request streams are semantically distinct: they are
//! per-request, short-lived, and cancellable, whereas control streams last
//! for the entire session.

use anyhow::Result;
use quinn::{RecvStream, SendStream};

/// Read side of a MOQT request stream (bidirectional).
pub struct RequestStreamReader {
    stream: RecvStream,
}

impl RequestStreamReader {
    pub fn new(stream: RecvStream) -> Self {
        Self { stream }
    }

    /// Read one message frame (Type + Length + Payload) from the request stream.
    pub async fn read_message_bytes(&mut self) -> Result<Vec<u8>> {
        crate::session::stream_utils::read_message_frame(&mut self.stream).await
    }
}

/// Write side of a MOQT request stream (bidirectional).
pub struct RequestStreamWriter {
    stream: SendStream,
}

impl RequestStreamWriter {
    pub fn new(stream: SendStream) -> Self {
        Self { stream }
    }

    /// Write a pre-framed message (Type + Length + Payload) as-is.
    pub async fn write_raw(&mut self, data: &[u8]) -> Result<()> {
        self.stream.write_all(data).await?;
        Ok(())
    }
}
