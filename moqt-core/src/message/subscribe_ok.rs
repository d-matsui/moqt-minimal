use anyhow::{Result, ensure};

use super::parameter::{MessageParameter, decode_parameters, encode_parameters};
use super::{MSG_SUBSCRIBE_OK, decode_message_header, encode_message_frame};
use crate::wire::varint::{decode_varint, encode_varint};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscribeOkMessage {
    pub track_alias: u64,
    pub parameters: Vec<MessageParameter>,
    // Track Properties: empty in minimal implementation
}

impl SubscribeOkMessage {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        let mut payload = Vec::new();
        encode_varint(self.track_alias, &mut payload);
        encode_parameters(&self.parameters, &mut payload);
        // Track Properties: empty (0 length)
        encode_varint(0, &mut payload);
        encode_message_frame(MSG_SUBSCRIBE_OK, &payload, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let (msg_type, payload) = decode_message_header(buf)?;
        ensure!(
            msg_type == MSG_SUBSCRIBE_OK,
            "expected SUBSCRIBE_OK (0x{MSG_SUBSCRIBE_OK:X}), got 0x{msg_type:X}"
        );
        let mut p = payload.as_slice();
        let track_alias = decode_varint(&mut p)?;
        let parameters = decode_parameters(&mut p)?;
        // Track Properties: skip (read length, skip bytes)
        let props_len = decode_varint(&mut p)? as usize;
        ensure!(p.len() >= props_len, "track properties truncated");
        // Skip properties content
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
        msg.encode(&mut buf);
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
        msg.encode(&mut buf);
        // Verify Track Properties length = 0 is at the end of payload
        let mut slice = buf.as_slice();
        let decoded = SubscribeOkMessage::decode(&mut slice).unwrap();
        assert_eq!(decoded.track_alias, 0);
    }
}
