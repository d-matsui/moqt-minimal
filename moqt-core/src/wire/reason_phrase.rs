use anyhow::{Result, ensure};

use super::varint::{decode_varint, encode_varint};

const MAX_LENGTH: u64 = 1024;

/// Reason Phrase: a length-prefixed UTF-8 string, max 1024 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasonPhrase {
    pub value: Vec<u8>,
}

pub fn encode_reason_phrase(rp: &ReasonPhrase, buf: &mut Vec<u8>) {
    encode_varint(rp.value.len() as u64, buf);
    buf.extend_from_slice(&rp.value);
}

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

    // 1.3: 空の reason phrase
    #[test]
    fn empty() {
        let rp = ReasonPhrase { value: vec![] };
        roundtrip(&rp);

        let mut buf = Vec::new();
        encode_reason_phrase(&rp, &mut buf);
        assert_eq!(buf, vec![0x00]);
    }

    // 1.3: UTF-8 文字列
    #[test]
    fn utf8_string() {
        let rp = ReasonPhrase {
            value: b"not found".to_vec(),
        };
        roundtrip(&rp);
    }

    // 1024バイトちょうどはOK
    #[test]
    fn max_length_ok() {
        let rp = ReasonPhrase {
            value: vec![b'x'; 1024],
        };
        roundtrip(&rp);
    }

    // 1025バイトはエラー
    #[test]
    fn too_long_is_error() {
        let mut buf = Vec::new();
        encode_varint(1025, &mut buf);
        buf.extend_from_slice(&vec![b'x'; 1025]);
        let mut slice = buf.as_slice();
        assert!(decode_reason_phrase(&mut slice).is_err());
    }

    // バッファ不足
    #[test]
    fn truncated() {
        let mut buf = Vec::new();
        encode_varint(10, &mut buf);
        buf.extend_from_slice(b"short");
        let mut slice = buf.as_slice();
        assert!(decode_reason_phrase(&mut slice).is_err());
    }
}
