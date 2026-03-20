//! # subscribe_request: Incoming SUBSCRIBE request (publisher/relay side)
//!
//! Represents a SUBSCRIBE that has been received but not yet responded to.
//! The holder can inspect the request, then accept or reject it.
//! After accepting, PUBLISH_DONE can be sent to signal end of publishing.

use anyhow::Result;

use crate::message::publish_done::PublishDoneMessage;
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

    /// Accept the subscription by sending SUBSCRIBE_OK.
    pub async fn accept(&mut self, ok: &SubscribeOkMessage) -> Result<()> {
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

    /// Send PUBLISH_DONE to signal end of publishing for this subscription.
    pub async fn send_publish_done(&mut self, done: &PublishDoneMessage) -> Result<()> {
        self.writer.write_publish_done(done).await
    }
}
