//! # request_ok: REQUEST_OK message (Section 9.6)
//!
//! Success response to requests such as PUBLISH_NAMESPACE.
//! This minimal implementation does not include any parameters.

use anyhow::{Result, ensure};

use super::{MSG_REQUEST_OK, decode_message, encode_message};
use crate::primitives::varint::{decode_varint, encode_varint};

/// REQUEST_OK message. Indicates that a request was successful.
///
/// ```text
/// Type (vi64) = 0x7,
/// Length (u16),
/// Number of Parameters (vi64),
/// Parameters (..) ...
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestOkMessage {
    // Parameters: LARGEST_OBJECT (0x09) can appear here when responding to
    // REQUEST_UPDATE or TRACK_STATUS, but this minimal implementation only
    // uses REQUEST_OK for PUBLISH_NAMESPACE responses, so count = 0.
}

impl RequestOkMessage {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        let mut payload = Vec::new();
        // Parameter count = 0 (minimal implementation)
        encode_varint(0, &mut payload);
        encode_message(MSG_REQUEST_OK, &payload, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let (msg_type, payload) = decode_message(buf)?;
        ensure!(
            msg_type == MSG_REQUEST_OK,
            "expected REQUEST_OK (0x{MSG_REQUEST_OK:X}), got 0x{msg_type:X}"
        );
        let mut p = payload.as_slice();
        let num_params = decode_varint(&mut p)?;
        ensure!(
            num_params == 0,
            "parameters not supported in minimal implementation"
        );
        Ok(RequestOkMessage {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let msg = RequestOkMessage {};
        let mut buf = Vec::new();
        msg.encode(&mut buf);
        let mut slice = buf.as_slice();
        let decoded = RequestOkMessage::decode(&mut slice).unwrap();
        assert_eq!(msg, decoded);
        assert!(slice.is_empty());
    }

    #[test]
    fn wrong_message_type_is_error() {
        let mut buf = Vec::new();
        encode_message(0x03, &[], &mut buf);
        let mut slice = buf.as_slice();
        assert!(RequestOkMessage::decode(&mut slice).is_err());
    }

    #[test]
    fn nonzero_params_is_error() {
        let mut payload = Vec::new();
        encode_varint(1, &mut payload); // parameter count = 1
        encode_varint(0x09, &mut payload); // fake parameter type
        encode_varint(0, &mut payload); // fake parameter value
        let mut buf = Vec::new();
        encode_message(MSG_REQUEST_OK, &payload, &mut buf);
        let mut slice = buf.as_slice();
        assert!(RequestOkMessage::decode(&mut slice).is_err());
    }
}
