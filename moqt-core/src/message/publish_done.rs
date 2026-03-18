//! # publish_done: PUBLISH_DONE message (Section 9.13)
//!
//! Sent by a publisher on the SUBSCRIBE bidi stream to notify that it is
//! done publishing objects for that subscription. The relay forwards this
//! to related subscribers.
//!
//! ## Common status codes
//! - `0x0`: INTERNAL_ERROR
//! - `0x2`: TRACK_ENDED (normal termination)

use anyhow::{Result, ensure};

use super::{MSG_PUBLISH_DONE, decode_message, encode_message};
use crate::wire::reason_phrase::{ReasonPhrase, decode_reason_phrase, encode_reason_phrase};
use crate::wire::varint::{decode_varint, encode_varint};

/// PUBLISH_DONE message. Notifies the end of publishing.
///
/// ```text
/// Type (vi64) = 0xB,
/// Length (u16),
/// Status Code (vi64),
/// Stream Count (vi64),
/// Error Reason (Reason Phrase)
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishDoneMessage {
    /// Status code indicating why the subscription ended.
    pub status_code: u64,
    /// Total number of streams (subgroups) sent during publishing.
    pub stream_count: u64,
    /// Human-readable termination reason (for debugging).
    pub reason_phrase: ReasonPhrase,
}

impl PublishDoneMessage {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        let mut payload = Vec::new();
        encode_varint(self.status_code, &mut payload);
        encode_varint(self.stream_count, &mut payload);
        encode_reason_phrase(&self.reason_phrase, &mut payload);
        encode_message(MSG_PUBLISH_DONE, &payload, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let (msg_type, payload) = decode_message(buf)?;
        ensure!(
            msg_type == MSG_PUBLISH_DONE,
            "expected PUBLISH_DONE (0x{MSG_PUBLISH_DONE:X}), got 0x{msg_type:X}"
        );
        let mut p = payload.as_slice();
        let status_code = decode_varint(&mut p)?;
        let stream_count = decode_varint(&mut p)?;
        let reason_phrase = decode_reason_phrase(&mut p)?;
        Ok(PublishDoneMessage {
            status_code,
            stream_count,
            reason_phrase,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let msg = PublishDoneMessage {
            status_code: 0x2, // TRACK_ENDED
            stream_count: 5,
            reason_phrase: ReasonPhrase { value: vec![] },
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf);
        let mut slice = buf.as_slice();
        let decoded = PublishDoneMessage::decode(&mut slice).unwrap();
        assert_eq!(msg, decoded);
        assert!(slice.is_empty());
    }

    #[test]
    fn with_reason() {
        let msg = PublishDoneMessage {
            status_code: 0x0, // INTERNAL_ERROR
            stream_count: 0,
            reason_phrase: ReasonPhrase {
                value: b"unexpected error".to_vec(),
            },
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf);
        let mut slice = buf.as_slice();
        let decoded = PublishDoneMessage::decode(&mut slice).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn wrong_message_type_is_error() {
        let mut buf = Vec::new();
        encode_message(0x03, &[], &mut buf);
        let mut slice = buf.as_slice();
        assert!(PublishDoneMessage::decode(&mut slice).is_err());
    }
}
