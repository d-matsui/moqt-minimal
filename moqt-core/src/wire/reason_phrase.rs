//! # reason_phrase: Length-prefixed string for error reasons, etc. (Section 1.4.4)
//!
//! A human-readable reason string included in REQUEST_ERROR and PUBLISH_DONE
//! messages. Represented as a UTF-8 byte sequence.
//!
//! ## Wire format
//! ```text
//! Reason Phrase Length (vi64),
//! Reason Phrase Value (..)
//! ```
//! Maximum length is 1024 bytes. Empty strings (length 0) are allowed.

use anyhow::{Result, ensure};

use super::varint::{decode_varint, encode_varint};

/// Maximum byte length for Reason Phrase. Prevents oversized data.
const MAX_LENGTH: u64 = 1024;

/// A length-prefixed UTF-8 string representing an error or termination reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasonPhrase {
    pub value: Vec<u8>,
}

impl From<&str> for ReasonPhrase {
    fn from(s: &str) -> Self {
        Self {
            value: s.as_bytes().to_vec(),
        }
    }
}

/// Encode a ReasonPhrase into bytes.
pub fn encode_reason_phrase(rp: &ReasonPhrase, buf: &mut Vec<u8>) {
    encode_varint(rp.value.len() as u64, buf);
    buf.extend_from_slice(&rp.value);
}

/// Decode a ReasonPhrase from bytes.
/// Returns an error if the length exceeds MAX_LENGTH.
pub fn decode_reason_phrase(buf: &mut &[u8]) -> Result<ReasonPhrase> {
    let len = decode_varint(buf)?;
    ensure!(
        len <= MAX_LENGTH,
        "reason phrase too long: {len} bytes (max {MAX_LENGTH})"
    );
    let len = len as usize;
    ensure!(
        buf.len() >= len,
        "need {len} bytes for reason phrase, have {}",
        buf.len()
    );
    let value = buf[..len].to_vec();
    *buf = &buf[len..];
    Ok(ReasonPhrase { value })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(rp: &ReasonPhrase) {
        let mut buf = Vec::new();
        encode_reason_phrase(rp, &mut buf);
        let mut slice = buf.as_slice();
        let decoded = decode_reason_phrase(&mut slice).unwrap();
        assert_eq!(rp, &decoded);
        assert!(slice.is_empty());
    }

    // Empty reason phrase
    #[test]
    fn empty() {
        let rp = ReasonPhrase { value: vec![] };
        roundtrip(&rp);

        let mut buf = Vec::new();
        encode_reason_phrase(&rp, &mut buf);
        assert_eq!(buf, vec![0x00]);
    }

    // UTF-8 string
    #[test]
    fn utf8_string() {
        let rp = ReasonPhrase {
            value: b"not found".to_vec(),
        };
        roundtrip(&rp);
    }

    // 1024 bytes is OK
    #[test]
    fn max_length_ok() {
        let rp = ReasonPhrase {
            value: vec![b'x'; 1024],
        };
        roundtrip(&rp);
    }

    // 1025 bytes is an error
    #[test]
    fn too_long_is_error() {
        let mut buf = Vec::new();
        encode_varint(1025, &mut buf);
        buf.extend_from_slice(&vec![b'x'; 1025]);
        let mut slice = buf.as_slice();
        assert!(decode_reason_phrase(&mut slice).is_err());
    }

    // Truncated buffer
    #[test]
    fn truncated_is_error() {
        let mut buf = Vec::new();
        encode_varint(10, &mut buf);
        buf.extend_from_slice(b"short");
        let mut slice = buf.as_slice();
        assert!(decode_reason_phrase(&mut slice).is_err());
    }
}
