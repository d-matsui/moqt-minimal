use std::io;

use super::varint::{decode_varint, encode_varint};

const MAX_VALUE_LENGTH: u64 = (1 << 16) - 1; // 2^16 - 1 = 65535

/// A decoded Key-Value-Pair with the absolute Type (not delta).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValuePair {
    pub type_id: u64,
    pub value: KvValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KvValue {
    /// Even type: value is a varint.
    Varint(u64),
    /// Odd type: value is a byte sequence.
    Bytes(Vec<u8>),
}

/// Encode a sequence of Key-Value-Pairs. Types must be in ascending order.
pub fn encode_key_value_pairs(pairs: &[KeyValuePair], buf: &mut Vec<u8>) {
    let mut prev_type: u64 = 0;
    for pair in pairs {
        let delta = pair.type_id - prev_type;
        encode_varint(delta, buf);
        match &pair.value {
            KvValue::Varint(v) => encode_varint(*v, buf),
            KvValue::Bytes(bytes) => {
                encode_varint(bytes.len() as u64, buf);
                buf.extend_from_slice(bytes);
            }
        }
        prev_type = pair.type_id;
    }
}

/// Decode Key-Value-Pairs from the buffer until it is empty.
pub fn decode_key_value_pairs(buf: &mut &[u8]) -> io::Result<Vec<KeyValuePair>> {
    let mut pairs = Vec::new();
    let mut prev_type: u64 = 0;

    while !buf.is_empty() {
        let delta = decode_varint(buf)?;
        let type_id = prev_type.checked_add(delta).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "type overflow")
        })?;

        let value = if type_id % 2 == 0 {
            // Even type: varint value
            KvValue::Varint(decode_varint(buf)?)
        } else {
            // Odd type: length-prefixed bytes
            let len = decode_varint(buf)?;
            if len > MAX_VALUE_LENGTH {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("value too long: {len} bytes (max {MAX_VALUE_LENGTH})"),
                ));
            }
            let len = len as usize;
            if buf.len() < len {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("need {len} bytes for value, have {}", buf.len()),
                ));
            }
            let bytes = buf[..len].to_vec();
            *buf = &buf[len..];
            KvValue::Bytes(bytes)
        };

        pairs.push(KeyValuePair { type_id, value });
        prev_type = type_id;
    }

    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(pairs: &[KeyValuePair]) {
        let mut buf = Vec::new();
        encode_key_value_pairs(pairs, &mut buf);
        let mut slice = buf.as_slice();
        let decoded = decode_key_value_pairs(&mut slice).unwrap();
        assert_eq!(pairs, decoded.as_slice());
        assert!(slice.is_empty());
    }

    // 1.4: 偶数Type（varint value）
    #[test]
    fn even_type_varint() {
        let pairs = vec![KeyValuePair {
            type_id: 4,
            value: KvValue::Varint(42),
        }];
        roundtrip(&pairs);
    }

    // 1.4: 奇数Type（length-prefixed bytes）
    #[test]
    fn odd_type_bytes() {
        let pairs = vec![KeyValuePair {
            type_id: 1,
            value: KvValue::Bytes(b"/path".to_vec()),
        }];
        roundtrip(&pairs);
    }

    // 1.4: Delta Type が正しく計算される
    #[test]
    fn delta_encoding() {
        let pairs = vec![
            KeyValuePair {
                type_id: 1,
                value: KvValue::Bytes(b"/".to_vec()),
            },
            KeyValuePair {
                type_id: 5,
                value: KvValue::Bytes(b"localhost".to_vec()),
            },
        ];
        roundtrip(&pairs);

        // Verify delta encoding on the wire
        let mut buf = Vec::new();
        encode_key_value_pairs(&pairs, &mut buf);
        let mut slice = buf.as_slice();
        // First pair: delta = 1 (from 0)
        let delta1 = decode_varint(&mut slice).unwrap();
        assert_eq!(delta1, 1);
        // Skip length + value of first pair
        let len1 = decode_varint(&mut slice).unwrap();
        slice = &slice[len1 as usize..];
        // Second pair: delta = 4 (5 - 1)
        let delta2 = decode_varint(&mut slice).unwrap();
        assert_eq!(delta2, 4);
    }

    // 空のペア列
    #[test]
    fn empty() {
        roundtrip(&[]);
    }

    // 混在（偶数 + 奇数）
    #[test]
    fn mixed_types() {
        let pairs = vec![
            KeyValuePair {
                type_id: 1,
                value: KvValue::Bytes(b"/".to_vec()),
            },
            KeyValuePair {
                type_id: 4,
                value: KvValue::Varint(100),
            },
            KeyValuePair {
                type_id: 5,
                value: KvValue::Bytes(b"host".to_vec()),
            },
        ];
        roundtrip(&pairs);
    }

    // デコード: value length が 2^16-1 を超えるとエラー
    #[test]
    fn value_too_long_is_error() {
        let mut buf = Vec::new();
        encode_varint(1, &mut buf); // delta type = 1 (odd)
        encode_varint(65536, &mut buf); // length > max
        buf.extend_from_slice(&vec![0; 65536]);
        let mut slice = buf.as_slice();
        assert!(decode_key_value_pairs(&mut slice).is_err());
    }

    // デコード: type overflow
    #[test]
    fn type_overflow_is_error() {
        let mut buf = Vec::new();
        encode_varint(u64::MAX, &mut buf); // delta = u64::MAX, prev = 0 → type = u64::MAX
        encode_varint(0, &mut buf); // varint value (even type)
        encode_varint(1, &mut buf); // delta = 1 → would overflow
        encode_varint(0, &mut buf);
        let mut slice = buf.as_slice();
        assert!(decode_key_value_pairs(&mut slice).is_err());
    }
}
