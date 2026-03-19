//! # subscription: Established MOQT subscription (subscriber side)
//!
//! Represents a subscription after SUBSCRIBE_OK has been received.
//! Holds the bidi stream's recv side to receive PUBLISH_DONE later.

use anyhow::{Result, bail};

use crate::message::publish_done::PublishDoneMessage;
use crate::session::request_stream::{RequestMessage, RequestStreamReader};

/// An established subscription (subscriber side).
/// Created by `MoqtSession::subscribe()` after receiving SUBSCRIBE_OK.
pub struct Subscription {
    /// Track alias assigned by the publisher.
    pub track_alias: u64,
    reader: RequestStreamReader,
}

impl Subscription {
    pub(crate) fn new(track_alias: u64, reader: RequestStreamReader) -> Self {
        Self {
            track_alias,
            reader,
        }
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
