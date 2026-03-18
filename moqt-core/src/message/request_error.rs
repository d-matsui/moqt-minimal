//! # request_error: REQUEST_ERROR message (Section 9.7)
//!
//! Error response to requests such as SUBSCRIBE.
//! Contains an error code, retry interval, and a human-readable reason.
//!
//! ## Common error codes
//! - `0x00`: INTERNAL_ERROR
//! - `0x10`: DOES_NOT_EXIST

use anyhow::{Result, ensure};

use super::{MSG_REQUEST_ERROR, decode_message, encode_message};
use crate::primitives::reason_phrase::{ReasonPhrase, decode_reason_phrase, encode_reason_phrase};
use crate::primitives::varint::{decode_varint, encode_varint};

/// REQUEST_ERROR message. Indicates that a request has failed.
///
/// ```text
/// Type (vi64) = 0x5,
/// Length (u16),
/// Error Code (vi64),
/// Retry Interval (vi64),
/// Reason Phrase Length (vi64),
/// Reason Phrase Value (..)
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestErrorMessage {
    /// Error code (spec-defined values).
    pub error_code: u64,
    /// Retry interval in milliseconds. 0 means no retry.
    pub retry_interval: u64,
    /// Human-readable error reason (for debugging).
    pub reason_phrase: ReasonPhrase,
}

impl RequestErrorMessage {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        let mut payload = Vec::new();
        encode_varint(self.error_code, &mut payload);
        encode_varint(self.retry_interval, &mut payload);
        encode_reason_phrase(&self.reason_phrase, &mut payload);
        encode_message(MSG_REQUEST_ERROR, &payload, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let (msg_type, payload) = decode_message(buf)?;
        ensure!(
            msg_type == MSG_REQUEST_ERROR,
            "expected REQUEST_ERROR (0x{MSG_REQUEST_ERROR:X}), got 0x{msg_type:X}"
        );
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
            error_code: 0x0,   // INTERNAL_ERROR
            retry_interval: 1, // immediate retry
            reason_phrase: ReasonPhrase { value: vec![] },
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf);
        let mut slice = buf.as_slice();
        let decoded = RequestErrorMessage::decode(&mut slice).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn wrong_message_type_is_error() {
        let mut buf = Vec::new();
        encode_message(0x06, &[], &mut buf);
        let mut slice = buf.as_slice();
        assert!(RequestErrorMessage::decode(&mut slice).is_err());
    }
}
