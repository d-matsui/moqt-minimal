//! # publish_namespace_request: Incoming PUBLISH_NAMESPACE request (relay/subscriber side)
//!
//! Represents a PUBLISH_NAMESPACE that has been received but not yet responded to.
//! The holder can inspect the request, then accept or reject it.

use anyhow::Result;

use crate::message::publish_namespace::PublishNamespaceMessage;
use crate::message::request_error::RequestErrorMessage;
use crate::message::request_ok::RequestOkMessage;
use crate::stream::request::RequestStreamWriter;

/// An incoming PUBLISH_NAMESPACE request that has not yet been responded to.
pub struct PublishNamespaceRequest {
    /// The received PUBLISH_NAMESPACE message.
    pub message: PublishNamespaceMessage,
    writer: RequestStreamWriter,
}

impl PublishNamespaceRequest {
    pub(crate) fn new(message: PublishNamespaceMessage, writer: RequestStreamWriter) -> Self {
        Self { message, writer }
    }

    /// Accept the namespace registration by sending REQUEST_OK.
    pub async fn accept(&mut self) -> Result<()> {
        let ok = RequestOkMessage {};
        self.writer.write_request_ok(&ok).await
    }

    /// Reject the namespace registration by sending REQUEST_ERROR.
    pub async fn reject(&mut self, err: &RequestErrorMessage) -> Result<()> {
        self.writer.write_request_error(err).await
    }
}
