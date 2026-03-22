//! # key_value_pair: Key-value pairs used in SETUP messages, etc. (Section 1.4.3)
//!
//! In MOQT SETUP messages, connection options (PATH, AUTHORITY, etc.) are sent
//! as a list of key-value pairs.
//!
//! ## Delta encoding
//! Pairs must be ordered by ascending Type. Each pair's Type is encoded as a
//! delta (difference) from the previous pair's Type, saving wire space.
//!
//! ## Value type determination
//! If the Type ID is even, the value is a varint; if odd, a length-prefixed byte sequence.
//! This allows adding new parameter types while maintaining backward compatibility.

use anyhow::{Result, bail, ensure};

use super::varint::{decode_varint, encode_varint};

/// Maximum byte length for values (odd Type). Prevents DoS attacks.
const MAX_VALUE_LENGTH: u64 = (1 << 16) - 1; // 2^16 - 1 = 65535

/// A decoded key-value pair. Type is an absolute value (not a delta).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValuePair {
    pub type_id: u64,
    pub value: KvValue,
}

/// Value of a key-value pair. The type is determined by the parity of the Type ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KvValue {
    /// Even Type: value is a varint integer
    Varint(u64),
    /// Odd Type: value is a length-prefixed byte sequence
    Bytes(Vec<u8>),
}

/// Encode a list of key-value pairs.
/// Pairs must be in ascending Type order.
/// Each pair's Type is delta-encoded (difference from the previous pair).
///
/// Even Type wire format:
/// ```text
/// Delta Type (vi64),
/// Value (vi64),
/// ```
///
/// Odd Type wire format:
/// ```text
/// Delta Type (vi64),
/// Length (vi64),
/// Value (Length bytes),
/// ```
pub fn encode_key_value_pairs(pairs: &[KeyValuePair], buf: &mut Vec<u8>) -> Result<()> {
    let mut prev_type: u64 = 0;
    for pair in pairs {
        // Validate ascending order
        ensure!(
            pair.type_id >= prev_type || prev_type == 0,
            "type_id {} is not in ascending order (prev: {})",
            pair.type_id,
            prev_type
        );
        // Validate parity matches value type
        match (&pair.value, pair.type_id % 2) {
            (KvValue::Varint(_), 1) => {
                bail!(
                    "type_id {} is odd but got Varint, expected Bytes",
                    pair.type_id
                )
            }
            (KvValue::Bytes(_), 0) => {
                bail!(
                    "type_id {} is even but got Bytes, expected Varint",
                    pair.type_id
                )
            }
            _ => {}
        }
        // Delta encoding: write the difference from the previous Type
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
    Ok(())
}

/// Decode key-value pairs until the buffer is empty.
/// Accumulates deltas to restore absolute Type IDs.
pub fn decode_key_value_pairs(buf: &mut &[u8]) -> Result<Vec<KeyValuePair>> {
    let mut pairs = Vec::new();
    let mut prev_type: u64 = 0;

    while !buf.is_empty() {
        let delta = decode_varint(buf)?;
        // Accumulate delta to restore absolute Type ID. Detect overflow.
        let type_id = prev_type
            .checked_add(delta)
            .ok_or_else(|| anyhow::anyhow!("type overflow"))?;

        // Value interpretation depends on parity of the Type ID
        let value = if type_id % 2 == 0 {
            // Even Type: varint value
            KvValue::Varint(decode_varint(buf)?)
        } else {
            // Odd Type: length-prefixed byte sequence
            let len = decode_varint(buf)?;
            ensure!(
                len <= MAX_VALUE_LENGTH,
                "value too long: {len} bytes (max {MAX_VALUE_LENGTH})"
            );
            // Cast to usize for slice indexing
            let len = len as usize;
            ensure!(
                buf.len() >= len,
                "need {len} bytes for value, have {}",
                buf.len()
            );
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
        encode_key_value_pairs(pairs, &mut buf).unwrap();
        let mut slice = buf.as_slice();
        let decoded = decode_key_value_pairs(&mut slice).unwrap();
        assert_eq!(pairs, decoded.as_slice());
        assert!(slice.is_empty());
    }

    // Even Type (varint value)
    #[test]
    fn even_type_varint() {
        let pairs = vec![KeyValuePair {
            type_id: 4,
            value: KvValue::Varint(42),
        }];
        roundtrip(&pairs);
    }

    // Odd Type (length-prefixed bytes)
    #[test]
    fn odd_type_bytes() {
        let pairs = vec![KeyValuePair {
            type_id: 1,
            value: KvValue::Bytes(b"/path".to_vec()),
        }];
        roundtrip(&pairs);
    }

    // Delta Type is computed correctly
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
        encode_key_value_pairs(&pairs, &mut buf).unwrap();
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

    // Empty pair list
    #[test]
    fn empty() {
        roundtrip(&[]);
    }

    // Mixed (even + odd)
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

    // Decode: value length exceeding 2^16-1 is an error
    #[test]
    fn value_too_long_is_error() {
        let mut buf = Vec::new();
        encode_varint(1, &mut buf); // delta type = 1 (odd)
        encode_varint(65536, &mut buf); // length > max
        buf.extend_from_slice(&vec![0; 65536]);
        let mut slice = buf.as_slice();
        assert!(decode_key_value_pairs(&mut slice).is_err());
    }

    // Encode: even type_id with Bytes value is an error
    #[test]
    fn even_type_with_bytes_is_error() {
        let pairs = vec![KeyValuePair {
            type_id: 4,
            value: KvValue::Bytes(b"bad".to_vec()),
        }];
        let mut buf = Vec::new();
        assert!(encode_key_value_pairs(&pairs, &mut buf).is_err());
    }

    // Encode: odd type_id with Varint value is an error
    #[test]
    fn odd_type_with_varint_is_error() {
        let pairs = vec![KeyValuePair {
            type_id: 1,
            value: KvValue::Varint(42),
        }];
        let mut buf = Vec::new();
        assert!(encode_key_value_pairs(&pairs, &mut buf).is_err());
    }

    // Decode: type overflow
    #[test]
    fn type_overflow_is_error() {
        let mut buf = Vec::new();
        encode_varint(u64::MAX, &mut buf); // delta = u64::MAX, prev = 0 -> type = u64::MAX
        encode_varint(0, &mut buf); // varint value (even type)
        encode_varint(1, &mut buf); // delta = 1 -> would overflow
        encode_varint(0, &mut buf);
        let mut slice = buf.as_slice();
        assert!(decode_key_value_pairs(&mut slice).is_err());
    }
}
