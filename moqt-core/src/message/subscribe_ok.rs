//! # subscribe_ok: SUBSCRIBE_OK message (Section 9.9)
//!
//! Success response to SUBSCRIBE. Indicates that the publisher accepted
//! the subscription. Contains a Track Alias used to identify the track
//! in subsequent data streams.

use anyhow::{Result, ensure};

use super::parameter::{MessageParameter, decode_parameters, encode_parameters};
use super::{MSG_SUBSCRIBE_OK, decode_message, encode_message};
use crate::wire::varint::{decode_varint, encode_varint};

/// SUBSCRIBE_OK message. Success response to a subscription.
///
/// ```text
/// Type (vi64) = 0x4,
/// Length (u16),
/// Track Alias (vi64),
/// Number of Parameters (vi64),
/// Parameters (..) ...,
/// Track Properties (..)
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscribeOkMessage {
    /// Track alias assigned by the publisher.
    /// Used in subsequent data streams (SubgroupHeader) to identify the track,
    /// avoiding the need to send the full namespace + track name each time.
    pub track_alias: u64,
    /// Response parameters (e.g. LARGEST_OBJECT).
    pub parameters: Vec<MessageParameter>,
    // Track Properties: empty in this minimal implementation
}

impl SubscribeOkMessage {
    pub fn encode(&self, buf: &mut Vec<u8>) -> Result<()> {
        let mut payload = Vec::new();
        encode_varint(self.track_alias, &mut payload);
        encode_parameters(&self.parameters, &mut payload)?;
        // Track Properties: empty (length 0) in this minimal implementation
        encode_varint(0, &mut payload);
        encode_message(MSG_SUBSCRIBE_OK, &payload, buf);
        Ok(())
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let (msg_type, payload) = decode_message(buf)?;
        ensure!(
            msg_type == MSG_SUBSCRIBE_OK,
            "expected SUBSCRIBE_OK (0x{MSG_SUBSCRIBE_OK:X}), got 0x{msg_type:X}"
        );
        let mut p = payload.as_slice();
        let track_alias = decode_varint(&mut p)?;
        let parameters = decode_parameters(&mut p)?;
        // Skip Track Properties (read length and skip that many bytes)
        let props_len = decode_varint(&mut p)? as usize;
        ensure!(p.len() >= props_len, "track properties truncated");
        let _ = &p[..props_len];
        Ok(SubscribeOkMessage {
            track_alias,
            parameters,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::parameter::MessageParameter;

    fn roundtrip(msg: &SubscribeOkMessage) {
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();
        let mut slice = buf.as_slice();
        let decoded = SubscribeOkMessage::decode(&mut slice).unwrap();
        assert_eq!(msg, &decoded);
        assert!(slice.is_empty());
    }

    #[test]
    fn basic() {
        let msg = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
        };
        roundtrip(&msg);
    }

    #[test]
    fn with_largest_object() {
        let msg = SubscribeOkMessage {
            track_alias: 42,
            parameters: vec![MessageParameter::LargestObject {
                group: 10,
                object: 5,
            }],
        };
        roundtrip(&msg);
    }

    #[test]
    fn track_properties_empty() {
        let msg = SubscribeOkMessage {
            track_alias: 0,
            parameters: vec![],
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();
        let mut slice = buf.as_slice();
        let decoded = SubscribeOkMessage::decode(&mut slice).unwrap();
        assert_eq!(decoded.track_alias, 0);
    }

    #[test]
    fn wrong_message_type_is_error() {
        let mut buf = Vec::new();
        encode_message(0x03, &[], &mut buf);
        let mut slice = buf.as_slice();
        assert!(SubscribeOkMessage::decode(&mut slice).is_err());
    }
}
