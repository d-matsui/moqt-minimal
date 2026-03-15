use std::io;

use crate::wire::reason_phrase::{decode_reason_phrase, encode_reason_phrase, ReasonPhrase};
use crate::wire::varint::{decode_varint, encode_varint};
use super::{decode_message_header, encode_message_frame, MSG_PUBLISH_DONE};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishDoneMessage {
    pub status_code: u64,
    pub stream_count: u64,
    pub reason_phrase: ReasonPhrase,
}

impl PublishDoneMessage {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        let mut payload = Vec::new();
        encode_varint(self.status_code, &mut payload);
        encode_varint(self.stream_count, &mut payload);
        encode_reason_phrase(&self.reason_phrase, &mut payload);
        encode_message_frame(MSG_PUBLISH_DONE, &payload, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> io::Result<Self> {
        let (msg_type, payload) = decode_message_header(buf)?;
        if msg_type != MSG_PUBLISH_DONE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected PUBLISH_DONE (0x{MSG_PUBLISH_DONE:X}), got 0x{msg_type:X}"),
            ));
        }
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
}
