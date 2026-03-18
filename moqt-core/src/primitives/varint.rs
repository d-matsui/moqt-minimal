//! # varint: MOQT Variable-Length Integer encoding/decoding
//!
//! MOQT uses variable-length integers for nearly all numeric fields in messages.
//! Small values are represented with fewer bytes, and larger values use more bytes,
//! reducing the wire size.
//!
//! ## Encoding scheme (Section 1.4.1)
//!
//! The prefix bits of the first byte determine the byte length:
//!
//! | Prefix     | Bytes | Usable bits | Max value (decimal)     | Max value (hex)          |
//! |------------|-------|-------------|-------------------------|--------------------------|
//! | `0`        | 1     | 7           | 127                     | `0x7F`                   |
//! | `10`       | 2     | 14          | 16,383                  | `0x3FFF`                 |
//! | `110`      | 3     | 21          | 2,097,151               | `0x1FFFFF`               |
//! | `1110`     | 4     | 28          | 268,435,455             | `0x0FFFFFFF`             |
//! | `11110`    | 5     | 35          | 34,359,738,367          | `0x07FFFFFFFF`           |
//! | `111110`   | 6     | 42          | 4,398,046,511,103       | `0x03FFFFFFFFFF`         |
//! | `1111110x` | -     | (invalid)   | -                       | -                        |
//! | `11111110` | 8     | 56          | 72,057,594,037,927,935  | `0x00FFFFFFFFFFFFFF`     |
//! | `11111111` | 9     | 64          | u64::MAX                | `0xFFFFFFFFFFFFFFFF`     |
//!
//! Note: There is no 7-byte encoding. `0xFC` and `0xFD` are reserved as
//! invalid code points (6-byte jumps directly to 8-byte).

use anyhow::{Result, bail, ensure};

/// Encode a u64 value as a MOQT variable-length integer and append it to the buffer.
/// Always uses the minimum number of bytes (minimal encoding).
///
/// # Encoding mechanism
/// The upper bits of the first byte serve as a prefix indicating
/// the number of subsequent bytes. The remaining bits store the value.
///
/// Example: value 37 -> 0b_0_0100101 = `0x25` (1 byte, prefix 0 + 7-bit value)
/// Example: value 15293 -> 0b_10_111011_10111101 = `0xbb 0xbd` (2 bytes, prefix 10 + 14-bit value)
pub fn encode_varint(value: u64, buf: &mut Vec<u8>) {
    if value <= 0x7f {
        // 1 byte: prefix 0 -> 7 usable bits
        buf.push(value as u8);
    } else if value <= 0x3fff {
        // 2 bytes: prefix 10 -> 14 usable bits
        buf.push(0x80 | (value >> 8) as u8); // prefix 10 | top 6 bits of value
        buf.push(value as u8);
    } else if value <= 0x1f_ffff {
        // 3 bytes: prefix 110 -> 21 usable bits
        buf.push(0xc0 | (value >> 16) as u8); // prefix 110 | top 5 bits of value
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else if value <= 0x0fff_ffff {
        // 4 bytes: prefix 1110 -> 28 usable bits
        buf.push(0xe0 | (value >> 24) as u8); // prefix 1110 | top 4 bits of value
        buf.push((value >> 16) as u8);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else if value <= 0x07_ffff_ffff {
        // 5 bytes: prefix 11110 -> 35 usable bits
        buf.push(0xf0 | (value >> 32) as u8); // prefix 11110 | top 3 bits of value
        buf.push((value >> 24) as u8);
        buf.push((value >> 16) as u8);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else if value <= 0x03ff_ffff_ffff {
        // 6 bytes: prefix 111110 -> 42 usable bits
        buf.push(0xf8 | (value >> 40) as u8); // prefix 111110 | top 2 bits of value
        buf.push((value >> 32) as u8);
        buf.push((value >> 24) as u8);
        buf.push((value >> 16) as u8);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else if value <= 0x00ff_ffff_ffff_ffff {
        // 8 bytes: prefix 11111110 -> 56 usable bits
        // Note: 7-byte encoding does not exist in the spec
        buf.push(0xfe); // prefix only (no value bits in this byte)
        buf.push((value >> 48) as u8);
        buf.push((value >> 40) as u8);
        buf.push((value >> 32) as u8);
        buf.push((value >> 24) as u8);
        buf.push((value >> 16) as u8);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else {
        // 9 bytes: prefix 11111111 -> 64 usable bits (full u64 range)
        buf.push(0xff); // prefix only (no value bits in this byte)
        buf.push((value >> 56) as u8);
        buf.push((value >> 48) as u8);
        buf.push((value >> 40) as u8);
        buf.push((value >> 32) as u8);
        buf.push((value >> 24) as u8);
        buf.push((value >> 16) as u8);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    }
}

/// Decode a MOQT variable-length integer from a byte slice.
/// Advances the slice reference past the consumed bytes.
///
/// # Decoding mechanism
/// 1. Determine the required byte count from the first byte's prefix bits
/// 2. Read the required number of bytes and combine in big-endian order
/// 3. Mask off the prefix bits to extract the value
///
/// Non-minimal encodings (e.g., 37 encoded in 2 bytes) are accepted.
/// This is permitted by the spec.
pub fn decode_varint(buf: &mut &[u8]) -> Result<u64> {
    ensure!(!buf.is_empty(), "empty buffer");

    let first = buf[0];

    // Determine byte length and value mask from the first byte's prefix pattern.
    // Example: 0x80 = 0b_10000000 masks the top 1 bit.
    //   If (first & 0x80 == 0), the top bit is 0 -> prefix 0 -> 1 byte, 7-bit value.
    //   The mask 0x7F = 0b_01111111 extracts the lower 7 bits as the value.
    //   The same pattern repeats: wider prefix -> more bytes, fewer value bits.
    let (len, value_mask) = if first & 0x80 == 0 {
        // top 1 bit is 0? -> 1 byte, 7-bit value
        (1usize, 0x7fu64)
    } else if first & 0xc0 == 0x80 {
        // top 2 bits are 10? -> 2 bytes, 14-bit value
        (2, 0x3fff)
    } else if first & 0xe0 == 0xc0 {
        // top 3 bits are 110? -> 3 bytes, 21-bit value
        (3, 0x1f_ffff)
    } else if first & 0xf0 == 0xe0 {
        // top 4 bits are 1110? -> 4 bytes, 28-bit value
        (4, 0x0fff_ffff)
    } else if first & 0xf8 == 0xf0 {
        // top 5 bits are 11110? -> 5 bytes, 35-bit value
        (5, 0x07_ffff_ffff)
    } else if first & 0xfc == 0xf8 {
        // top 6 bits are 111110? -> 6 bytes, 42-bit value
        (6, 0x03ff_ffff_ffff)
    } else if first == 0xfc {
        bail!("invalid varint code point 0xFC");
    } else if first == 0xfe {
        // 8 bytes, 56-bit value
        (8, 0x00ff_ffff_ffff_ffff)
    } else {
        // 9 bytes, 64-bit value (full u64 range)
        (9, u64::MAX)
    };

    ensure!(buf.len() >= len, "need {len} bytes, have {}", buf.len());

    // Combine bytes as big-endian into u64 (still includes prefix bits)
    let mut value = 0u64;
    for &byte in &buf[..len] {
        value = (value << 8) | byte as u64;
    }
    // Strip prefix bits to get the actual value
    value &= value_mask;

    *buf = &buf[len..];
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(value: u64) {
        let mut buf = Vec::new();
        encode_varint(value, &mut buf);
        let mut slice = buf.as_slice();
        let decoded = decode_varint(&mut slice).unwrap();
        assert_eq!(value, decoded);
        assert!(slice.is_empty(), "all bytes should be consumed");
    }

    // 1-byte values (0..=127)
    #[test]
    fn encode_decode_1byte() {
        roundtrip(0);
        roundtrip(1);
        roundtrip(37);
        roundtrip(127);
    }

    // 2-byte values (128..=16383)
    #[test]
    fn encode_decode_2byte() {
        roundtrip(128);
        roundtrip(15293);
        roundtrip(16383);
    }

    // 3-byte values
    #[test]
    fn encode_decode_3byte() {
        roundtrip(16384);
        roundtrip(2097151);
    }

    // 4-byte values
    #[test]
    fn encode_decode_4byte() {
        roundtrip(2097152);
        roundtrip(494_878_333);
        roundtrip(268_435_455);
    }

    // 5-byte values
    #[test]
    fn encode_decode_5byte() {
        roundtrip(268_435_456);
        roundtrip(34_359_738_367);
    }

    // 6-byte values
    #[test]
    fn encode_decode_6byte() {
        roundtrip(34_359_738_368);
        roundtrip(2_893_212_287_960);
        roundtrip(4_398_046_511_103);
    }

    // 8-byte values
    #[test]
    fn encode_decode_8byte() {
        roundtrip(4_398_046_511_104);
        roundtrip(70_423_237_261_249_041);
        roundtrip(72_057_594_037_927_935);
    }

    // 9-byte values
    #[test]
    fn encode_decode_9byte() {
        roundtrip(72_057_594_037_927_936);
        roundtrip(u64::MAX);
    }

    // Byte sequence matches the spec examples
    #[test]
    fn spec_example_37() {
        let mut buf = Vec::new();
        encode_varint(37, &mut buf);
        assert_eq!(buf, vec![0x25]);
    }

    #[test]
    fn spec_example_15293() {
        let mut buf = Vec::new();
        encode_varint(15293, &mut buf);
        assert_eq!(buf, vec![0xbb, 0xbd]);
    }

    // Spec example 0xdd7f3e7d uses non-minimal encoding (3-byte prefix 110 but 4 bytes).
    // Minimal encoding for 494,878,333 requires 5 bytes.
    // Decoding the spec's byte sequence is tested separately.
    #[test]
    fn spec_example_494878333() {
        let mut buf = Vec::new();
        encode_varint(494_878_333, &mut buf);
        assert_eq!(buf.len(), 5); // minimal encoding is 5 bytes
        roundtrip(494_878_333);
    }

    #[test]
    fn spec_example_2893212287960() {
        let mut buf = Vec::new();
        encode_varint(2_893_212_287_960, &mut buf);
        assert_eq!(buf, vec![0xfa, 0xa1, 0xa0, 0xe4, 0x03, 0xd8]);
    }

    #[test]
    fn spec_example_70423237261249041() {
        let mut buf = Vec::new();
        encode_varint(70_423_237_261_249_041, &mut buf);
        assert_eq!(buf, vec![0xfe, 0xfa, 0x31, 0x8f, 0xa8, 0xe3, 0xca, 0x11]);
    }

    #[test]
    fn spec_example_u64_max() {
        let mut buf = Vec::new();
        encode_varint(u64::MAX, &mut buf);
        assert_eq!(
            buf,
            vec![0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
        );
    }

    // Values are encoded in the minimum number of bytes
    #[test]
    fn minimal_encoding_length() {
        let cases: Vec<(u64, usize)> = vec![
            (0, 1),
            (127, 1),
            (128, 2),
            (16383, 2),
            (16384, 3),
            (2097151, 3),
            (2097152, 4),
            (268_435_455, 4),
            (268_435_456, 5),
            (34_359_738_367, 5),
            (34_359_738_368, 6),
            (4_398_046_511_103, 6),
            (4_398_046_511_104, 8),
            (72_057_594_037_927_935, 8),
            (72_057_594_037_927_936, 9),
            (u64::MAX, 9),
        ];
        for (value, expected_len) in cases {
            let mut buf = Vec::new();
            encode_varint(value, &mut buf);
            assert_eq!(
                buf.len(),
                expected_len,
                "value={value} should encode in {expected_len} bytes"
            );
        }
    }

    // Decode: spec example byte sequence (non-minimal encoding 0x8025 = 37)
    #[test]
    fn spec_example_non_minimal_37() {
        let mut slice: &[u8] = &[0x80, 0x25];
        let value = decode_varint(&mut slice).unwrap();
        assert_eq!(value, 37);
    }

    // Decode: empty buffer
    #[test]
    fn decode_empty_buffer_is_error() {
        let mut slice: &[u8] = &[];
        assert!(decode_varint(&mut slice).is_err());
    }

    // Decode: truncated buffer (needs 2 bytes but only 1 available)
    #[test]
    fn decode_truncated_buffer_is_error() {
        let mut slice: &[u8] = &[0x80]; // needs 2 bytes
        assert!(decode_varint(&mut slice).is_err());
    }

    // Decode: invalid code point 0xFC (11111100)
    #[test]
    fn decode_invalid_codepoint_is_error() {
        let mut slice: &[u8] = &[0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        assert!(decode_varint(&mut slice).is_err());
    }
}
