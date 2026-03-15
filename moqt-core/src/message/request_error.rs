use std::io;

use crate::wire::reason_phrase::{decode_reason_phrase, encode_reason_phrase, ReasonPhrase};
use crate::wire::varint::{decode_varint, encode_varint};
use super::{decode_message_header, encode_message_frame, MSG_REQUEST_ERROR};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestErrorMessage {
    pub error_code: u64,
    pub retry_interval: u64,
    pub reason_phrase: ReasonPhrase,
}

impl RequestErrorMessage {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        let mut payload = Vec::new();
        encode_varint(self.error_code, &mut payload);
        encode_varint(self.retry_interval, &mut payload);
        encode_reason_phrase(&self.reason_phrase, &mut payload);
        encode_message_frame(MSG_REQUEST_ERROR, &payload, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> io::Result<Self> {
        let (msg_type, payload) = decode_message_header(buf)?;
        if msg_type != MSG_REQUEST_ERROR {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected REQUEST_ERROR (0x{MSG_REQUEST_ERROR:X}), got 0x{msg_type:X}"),
            ));
        }
        let mut p = payload.as_slice();
        let error_code = decode_varint(&mut p)?;
        let retry_interval = decode_varint(&mut p)?;
        let reason_phrase = decode_reason_phrase(&mut p)?;
        Ok(RequestErrorMessage {
            error_code,
            retry_interval,
            reason_phrase,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let msg = RequestErrorMessage {
            error_code: 0x10, // DOES_NOT_EXIST
            retry_interval: 0,
            reason_phrase: ReasonPhrase {
                value: b"track not found".to_vec(),
            },
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf);
        let mut slice = buf.as_slice();
        let decoded = RequestErrorMessage::decode(&mut slice).unwrap();
        assert_eq!(msg, decoded);
        assert!(slice.is_empty());
    }

    #[test]
    fn empty_reason() {
        let msg = RequestErrorMessage {
            error_code: 0x0, // INTERNAL_ERROR
            retry_interval: 1, // immediate retry
            reason_phrase: ReasonPhrase { value: vec![] },
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf);
        let mut slice = buf.as_slice();
        let decoded = RequestErrorMessage::decode(&mut slice).unwrap();
        assert_eq!(msg, decoded);
    }
}
