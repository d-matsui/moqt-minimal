//! # subscribe_request: Incoming SUBSCRIBE request (publisher/relay side)
//!
//! Represents a SUBSCRIBE that has been received but not yet responded to.
//! The holder can inspect the request, then accept or reject it.
//! After accepting, PUBLISH_DONE can be sent to signal end of publishing.

use anyhow::Result;

use crate::message::publish_done::{PublishDoneMessage, STATUS_TRACK_ENDED};
use crate::message::request_error::RequestErrorMessage;
use crate::message::subscribe::SubscribeMessage;
use crate::message::subscribe_ok::SubscribeOkMessage;
use crate::primitives::reason_phrase::ReasonPhrase;
use crate::session::request_stream::RequestStreamWriter;

/// An incoming SUBSCRIBE request that has not yet been responded to.
pub struct SubscribeRequest {
    /// The received SUBSCRIBE message.
    pub message: SubscribeMessage,
    writer: RequestStreamWriter,
}

impl SubscribeRequest {
    pub(crate) fn new(message: SubscribeMessage, writer: RequestStreamWriter) -> Self {
        Self { message, writer }
    }

    /// Accept the subscription by sending SUBSCRIBE_OK with the given track alias.
    /// Uses default values for parameters and track properties.
    pub async fn accept(&mut self, track_alias: u64) -> Result<()> {
        let ok = SubscribeOkMessage {
            track_alias,
            parameters: vec![],
            track_properties_raw: vec![],
        };
        self.writer.write_subscribe_ok(&ok).await
    }

    /// Accept the subscription by forwarding a full SUBSCRIBE_OK message.
    /// Used by the relay to forward the publisher's response as-is.
    pub async fn forward_subscribe_ok(&mut self, ok: &SubscribeOkMessage) -> Result<()> {
        self.writer.write_subscribe_ok(ok).await
    }

    /// Reject the subscription by sending REQUEST_ERROR.
    pub async fn reject(&mut self, error_code: u64, reason: &str) -> Result<()> {
        let err = RequestErrorMessage {
            error_code,
            retry_interval: 0,
            reason_phrase: ReasonPhrase::from(reason),
        };
        self.writer.write_request_error(&err).await
    }

    /// Send PUBLISH_DONE with TRACK_ENDED status to signal normal end of publishing.
    pub async fn send_publish_done(&mut self, stream_count: u64) -> Result<()> {
        let done = PublishDoneMessage {
            status_code: STATUS_TRACK_ENDED,
            stream_count,
            reason_phrase: ReasonPhrase::from(""),
        };
        self.writer.write_publish_done(&done).await
    }

    /// Forward a full PUBLISH_DONE message.
    /// Used by the relay to forward the publisher's message as-is.
    pub async fn forward_publish_done(&mut self, done: &PublishDoneMessage) -> Result<()> {
        self.writer.write_publish_done(done).await
    }
}
