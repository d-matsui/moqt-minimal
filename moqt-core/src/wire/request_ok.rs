//! # request_ok: REQUEST_OK message (Section 9.6)
//!
//! Success response to requests such as PUBLISH_NAMESPACE.

use anyhow::{Result, ensure};

use super::{MSG_REQUEST_OK, decode_message, encode_message};
use crate::wire::parameter::{decode_parameters, encode_parameters};

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
    // Parameters (Section 9.3): LARGEST_OBJECT (0x09) can appear here.
    // This implementation always encodes with count = 0; on decode,
    // parameter handling is delegated to decode_parameters.
}

impl RequestOkMessage {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        let mut payload = Vec::new();
        encode_parameters(&[], &mut payload).expect("empty params never fail");
        encode_message(MSG_REQUEST_OK, &payload, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let (msg_type, payload) = decode_message(buf)?;
        ensure!(
            msg_type == MSG_REQUEST_OK,
            "expected REQUEST_OK (0x{MSG_REQUEST_OK:X}), got 0x{msg_type:X}"
        );
        let mut p = payload.as_slice();
        let _params = decode_parameters(&mut p)?;
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
    fn decode_with_parameters() {
        use crate::wire::varint::encode_varint;
        // A REQUEST_OK with a LARGEST_OBJECT parameter.
        // decode_parameters reads it into the result; decode should succeed.
        let mut payload = Vec::new();
        encode_varint(1, &mut payload); // parameter count = 1
        encode_varint(0x09, &mut payload); // delta = LARGEST_OBJECT type
        encode_varint(5, &mut payload); // group
        encode_varint(3, &mut payload); // object
        let mut buf = Vec::new();
        encode_message(MSG_REQUEST_OK, &payload, &mut buf);
        let mut slice = buf.as_slice();
        let decoded = RequestOkMessage::decode(&mut slice).unwrap();
        assert_eq!(decoded, RequestOkMessage {});
    }
}
