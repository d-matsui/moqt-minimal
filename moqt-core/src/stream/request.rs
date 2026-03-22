//! # request: MOQT request stream reader/writer
//!
//! In MOQT, each request (SUBSCRIBE, PUBLISH_NAMESPACE, etc.) is carried on
//! a dedicated bidirectional QUIC stream. This module provides reader/writer
//! wrappers for those bidi streams.
//!
//! The frame format (varint type, u16 length, payload) is the same as control
//! streams. However, request streams are semantically distinct: they are
//! per-request, short-lived, and cancellable, whereas control streams last
//! for the entire session.

use anyhow::{Result, bail};
use quinn::{RecvStream, SendStream};

use crate::wire::publish_done::PublishDoneMessage;
use crate::wire::publish_namespace::PublishNamespaceMessage;
use crate::wire::request_error::RequestErrorMessage;
use crate::wire::request_ok::RequestOkMessage;
use crate::wire::subscribe::SubscribeMessage;
use crate::wire::subscribe_ok::SubscribeOkMessage;
use crate::wire::varint::decode_varint;
use crate::wire::{
    MSG_PUBLISH_DONE, MSG_PUBLISH_NAMESPACE, MSG_REQUEST_ERROR, MSG_REQUEST_OK, MSG_SUBSCRIBE,
    MSG_SUBSCRIBE_OK,
};

/// A typed message read from or written to a request stream.
pub enum RequestMessage {
    PublishNamespace(PublishNamespaceMessage),
    Subscribe(SubscribeMessage),
    SubscribeOk(SubscribeOkMessage),
    RequestOk(RequestOkMessage),
    RequestError(RequestErrorMessage),
    PublishDone(PublishDoneMessage),
}

/// Parse a raw message frame (Type + Length + Payload) into a typed `RequestMessage`.
pub fn parse_request_message(bytes: &[u8]) -> Result<RequestMessage> {
    let mut slice = bytes;
    let msg_type = decode_varint(&mut slice)?;

    let mut slice = bytes;
    match msg_type {
        MSG_PUBLISH_NAMESPACE => Ok(RequestMessage::PublishNamespace(
            PublishNamespaceMessage::decode(&mut slice)?,
        )),
        MSG_SUBSCRIBE => Ok(RequestMessage::Subscribe(SubscribeMessage::decode(
            &mut slice,
        )?)),
        MSG_SUBSCRIBE_OK => Ok(RequestMessage::SubscribeOk(SubscribeOkMessage::decode(
            &mut slice,
        )?)),
        MSG_REQUEST_OK => Ok(RequestMessage::RequestOk(RequestOkMessage::decode(
            &mut slice,
        )?)),
        MSG_REQUEST_ERROR => Ok(RequestMessage::RequestError(RequestErrorMessage::decode(
            &mut slice,
        )?)),
        MSG_PUBLISH_DONE => Ok(RequestMessage::PublishDone(PublishDoneMessage::decode(
            &mut slice,
        )?)),
        _ => bail!("unknown request message type: 0x{msg_type:X}"),
    }
}

/// Read side of a MOQT request stream (bidirectional).
pub struct RequestStreamReader {
    stream: RecvStream,
}

impl RequestStreamReader {
    pub fn new(stream: RecvStream) -> Self {
        Self { stream }
    }

    /// Read one typed message from the request stream.
    pub async fn read_message(&mut self) -> Result<RequestMessage> {
        let bytes = self.read_message_bytes().await?;
        parse_request_message(&bytes)
    }

    /// Read one message frame (Type + Length + Payload) as raw bytes.
    pub async fn read_message_bytes(&mut self) -> Result<Vec<u8>> {
        crate::stream::read_message_frame(&mut self.stream).await
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

    pub async fn write_publish_namespace(&mut self, msg: &PublishNamespaceMessage) -> Result<()> {
        let mut buf = Vec::new();
        msg.encode(&mut buf)?;
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    pub async fn write_subscribe(&mut self, msg: &SubscribeMessage) -> Result<()> {
        let mut buf = Vec::new();
        msg.encode(&mut buf)?;
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    pub async fn write_subscribe_ok(&mut self, msg: &SubscribeOkMessage) -> Result<()> {
        let mut buf = Vec::new();
        msg.encode(&mut buf)?;
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    pub async fn write_request_ok(&mut self, msg: &RequestOkMessage) -> Result<()> {
        let mut buf = Vec::new();
        msg.encode(&mut buf);
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    pub async fn write_request_error(&mut self, msg: &RequestErrorMessage) -> Result<()> {
        let mut buf = Vec::new();
        msg.encode(&mut buf);
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    pub async fn write_publish_done(&mut self, msg: &PublishDoneMessage) -> Result<()> {
        let mut buf = Vec::new();
        msg.encode(&mut buf);
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    /// Write a pre-framed message (Type + Length + Payload) as-is.
    pub async fn write_raw(&mut self, data: &[u8]) -> Result<()> {
        self.stream.write_all(data).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::parameter::{MessageParameter, SubscriptionFilter};
    use crate::wire::reason_phrase::ReasonPhrase;
    use crate::wire::track_namespace::TrackNamespace;

    /// Helper: encode a message and parse it back via parse_request_message.
    fn roundtrip_parse<F>(encode: F) -> RequestMessage
    where
        F: FnOnce(&mut Vec<u8>),
    {
        let mut buf = Vec::new();
        encode(&mut buf);
        parse_request_message(&buf).unwrap()
    }

    #[test]
    fn parse_publish_namespace() {
        let msg = roundtrip_parse(|buf| {
            let msg = PublishNamespaceMessage {
                request_id: 0,
                required_request_id_delta: 0,
                track_namespace: TrackNamespace {
                    fields: vec![b"example".to_vec()],
                },
            };
            msg.encode(buf).unwrap();
        });
        assert!(matches!(msg, RequestMessage::PublishNamespace(_)));
    }

    #[test]
    fn parse_subscribe() {
        let msg = roundtrip_parse(|buf| {
            let msg = SubscribeMessage {
                request_id: 0,
                required_request_id_delta: 0,
                track_namespace: TrackNamespace {
                    fields: vec![b"example".to_vec()],
                },
                track_name: b"video".to_vec(),
                parameters: vec![MessageParameter::SubscriptionFilter(
                    SubscriptionFilter::NextGroupStart,
                )],
            };
            msg.encode(buf).unwrap();
        });
        assert!(matches!(msg, RequestMessage::Subscribe(_)));
    }

    #[test]
    fn parse_subscribe_ok() {
        let msg = roundtrip_parse(|buf| {
            let msg = SubscribeOkMessage {
                track_alias: 1,
                parameters: vec![],
                track_properties_raw: vec![],
            };
            msg.encode(buf).unwrap();
        });
        assert!(matches!(msg, RequestMessage::SubscribeOk(_)));
    }

    #[test]
    fn parse_request_ok() {
        let msg = roundtrip_parse(|buf| {
            let msg = RequestOkMessage {};
            msg.encode(buf);
        });
        assert!(matches!(msg, RequestMessage::RequestOk(_)));
    }

    #[test]
    fn parse_request_error() {
        let msg = roundtrip_parse(|buf| {
            let msg = RequestErrorMessage {
                error_code: 0x01,
                retry_interval: 0,
                reason_phrase: ReasonPhrase {
                    value: b"error".to_vec(),
                },
            };
            msg.encode(buf);
        });
        assert!(matches!(msg, RequestMessage::RequestError(_)));
    }

    #[test]
    fn parse_publish_done() {
        let msg = roundtrip_parse(|buf| {
            let msg = PublishDoneMessage {
                status_code: 0x00,
                stream_count: 0,
                reason_phrase: ReasonPhrase { value: Vec::new() },
            };
            msg.encode(buf);
        });
        assert!(matches!(msg, RequestMessage::PublishDone(_)));
    }

    #[test]
    fn parse_unknown_type_is_error() {
        // Encode a fake message with type 0xFF
        let mut buf = Vec::new();
        crate::wire::varint::encode_varint(0xFF, &mut buf);
        buf.extend_from_slice(&0u16.to_be_bytes());
        assert!(parse_request_message(&buf).is_err());
    }
}
