//! # subscription: Established MOQT subscription (subscriber side)
//!
//! Represents a subscription after SUBSCRIBE_OK has been received.
//! Holds the bidi stream's recv side to receive PUBLISH_DONE later.

use anyhow::{Result, bail};

use crate::message::publish_done::PublishDoneMessage;
use crate::message::subscribe_ok::SubscribeOkMessage;
use crate::session::request_stream::{RequestMessage, RequestStreamReader};

/// An established subscription (subscriber side).
/// Created by `MoqtSession::subscribe()` after receiving SUBSCRIBE_OK.
pub struct Subscription {
    /// The SUBSCRIBE_OK message received from the publisher.
    pub subscribe_ok: SubscribeOkMessage,
    reader: RequestStreamReader,
}

impl Subscription {
    pub(crate) fn new(subscribe_ok: SubscribeOkMessage, reader: RequestStreamReader) -> Self {
        Self {
            subscribe_ok,
            reader,
        }
    }

    /// Track alias assigned by the publisher.
    pub fn track_alias(&self) -> u64 {
        self.subscribe_ok.track_alias
    }

    /// Wait for PUBLISH_DONE from the publisher.
    pub async fn recv_publish_done(&mut self) -> Result<PublishDoneMessage> {
        let msg = self.reader.read_message().await?;
        match msg {
            RequestMessage::PublishDone(done) => Ok(done),
            _ => bail!("expected PUBLISH_DONE"),
        }
    }
}
