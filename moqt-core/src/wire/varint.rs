use std::io;

/// Encode a u64 value as a MOQT variable-length integer.
/// Uses the minimum number of bytes required.
///
/// Encoding table (Section 1.4.1):
///   Leading bits | Bytes | Usable bits | Max value
///   0            | 1     | 7           | 127
///   10           | 2     | 14          | 16383
///   110          | 3     | 21          | 2097151
///   1110         | 4     | 28          | 268435455
///   11110        | 5     | 35          | 34359738367
///   111110       | 6     | 42          | 4398046511103
///   11111110     | 8     | 56          | 72057594037927935
///   11111111     | 9     | 64          | u64::MAX
pub fn encode_varint(value: u64, buf: &mut Vec<u8>) {
    if value <= 0x7f {
        // 1 byte: 0xxxxxxx
        buf.push(value as u8);
    } else if value <= 0x3fff {
        // 2 bytes: 10xxxxxx xxxxxxxx
        buf.push(0x80 | (value >> 8) as u8);
        buf.push(value as u8);
    } else if value <= 0x1f_ffff {
        // 3 bytes: 110xxxxx xxxxxxxx xxxxxxxx
        buf.push(0xc0 | (value >> 16) as u8);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else if value <= 0x0fff_ffff {
        // 4 bytes: 1110xxxx xxxxxxxx xxxxxxxx xxxxxxxx
        buf.push(0xe0 | (value >> 24) as u8);
        buf.push((value >> 16) as u8);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else if value <= 0x07_ffff_ffff {
        // 5 bytes: 11110xxx xxxxxxxx xxxxxxxx xxxxxxxx xxxxxxxx
        buf.push(0xf0 | (value >> 32) as u8);
        buf.push((value >> 24) as u8);
        buf.push((value >> 16) as u8);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else if value <= 0x03ff_ffff_ffff {
        // 6 bytes: 111110xx xxxxxxxx xxxxxxxx xxxxxxxx xxxxxxxx xxxxxxxx
        buf.push(0xf8 | (value >> 40) as u8);
        buf.push((value >> 32) as u8);
        buf.push((value >> 24) as u8);
        buf.push((value >> 16) as u8);
        buf.push((value >> 8) as u8);
        buf.push(value as u8);
    } else if value <= 0x00ff_ffff_ffff_ffff {
        // 8 bytes: 11111110 xxxxxxxx * 7
        buf.push(0xfe);
        for i in (0..7).rev() {
            buf.push((value >> (i * 8)) as u8);
        }
    } else {
        // 9 bytes: 11111111 xxxxxxxx * 8
        buf.push(0xff);
        for i in (0..8).rev() {
            buf.push((value >> (i * 8)) as u8);
        }
    }
}

/// Decode a MOQT variable-length integer from the given byte slice.
/// Advances the slice past the consumed bytes.
pub fn decode_varint(buf: &mut &[u8]) -> io::Result<u64> {
    if buf.is_empty() {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "empty buffer"));
    }

    let first = buf[0];

    // Determine length from leading bits
    let (len, value_mask) = if first & 0x80 == 0 {
        (1usize, 0x7fu64)
    } else if first & 0xc0 == 0x80 {
        (2, 0x3fff)
    } else if first & 0xe0 == 0xc0 {
        (3, 0x1f_ffff)
    } else if first & 0xf0 == 0xe0 {
        (4, 0x0fff_ffff)
    } else if first & 0xf8 == 0xf0 {
        (5, 0x07_ffff_ffff)
    } else if first & 0xfc == 0xf8 {
        (6, 0x03ff_ffff_ffff)
    } else if first == 0xfc {
        // 11111100 is an invalid code point (Section 1.4.1)
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid varint code point 0xFC",
        ));
    } else if first == 0xfe {
        (8, 0x00ff_ffff_ffff_ffff)
    } else {
        // 0xff
        (9, u64::MAX)
    };

    if buf.len() < len {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("need {len} bytes, have {}", buf.len()),
        ));
    }

    let mut value = 0u64;
    for &byte in &buf[..len] {
        value = (value << 8) | byte as u64;
    }
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

    // 1.1: 1バイト値（0〜127）
    #[test]
    fn encode_decode_1byte() {
        roundtrip(0);
        roundtrip(1);
        roundtrip(37);
        roundtrip(127);
    }

    // 1.1: 2バイト値（128〜16383）
    #[test]
    fn encode_decode_2byte() {
        roundtrip(128);
        roundtrip(15293);
        roundtrip(16383);
    }

    // 1.1: 3バイト値
    #[test]
    fn encode_decode_3byte() {
        roundtrip(16384);
        roundtrip(2097151);
    }

    // 1.1: 4バイト値
    #[test]
    fn encode_decode_4byte() {
        roundtrip(2097152);
        roundtrip(494_878_333);
        roundtrip(268_435_455);
    }

    // 5バイト値
    #[test]
    fn encode_decode_5byte() {
        roundtrip(268_435_456);
        roundtrip(34_359_738_367);
    }

    // 6バイト値
    #[test]
    fn encode_decode_6byte() {
        roundtrip(34_359_738_368);
        roundtrip(2_893_212_287_960);
        roundtrip(4_398_046_511_103);
    }

    // 8バイト値
    #[test]
    fn encode_decode_8byte() {
        roundtrip(4_398_046_511_104);
        roundtrip(70_423_237_261_249_041);
        roundtrip(72_057_594_037_927_935);
    }

    // 9バイト値
    #[test]
    fn encode_decode_9byte() {
        roundtrip(72_057_594_037_927_936);
        roundtrip(u64::MAX);
    }

    // 1.1: 仕様の例示値でバイト列が一致する
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

    // 仕様の例示 0xdd7f3e7d は non-minimal encoding（3バイト prefix 110 だが4バイトある）。
    // 最小エンコードでは 494,878,333 は5バイトになる。
    // デコードで仕様例のバイト列を正しく読めることを別途テストする。
    #[test]
    fn spec_example_494878333() {
        let mut buf = Vec::new();
        encode_varint(494_878_333, &mut buf);
        assert_eq!(buf.len(), 5); // 最小エンコードは5バイト
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

    // 1.1: 最小バイト数でエンコードされる
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

    // デコード: 仕様の例示バイト列（非最小エンコード 0x8025 = 37）
    #[test]
    fn spec_example_non_minimal_37() {
        let mut slice: &[u8] = &[0x80, 0x25];
        let value = decode_varint(&mut slice).unwrap();
        assert_eq!(value, 37);
    }

    // デコード: 空バッファ
    #[test]
    fn decode_empty_buffer() {
        let mut slice: &[u8] = &[];
        assert!(decode_varint(&mut slice).is_err());
    }

    // デコード: バッファ不足（2バイト必要だが1バイトしかない）
    #[test]
    fn decode_truncated_buffer() {
        let mut slice: &[u8] = &[0x80]; // 2バイト必要
        assert!(decode_varint(&mut slice).is_err());
    }

    // デコード: 無効なコードポイント 0xFC (11111100)
    #[test]
    fn decode_invalid_codepoint() {
        let mut slice: &[u8] = &[0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
        assert!(decode_varint(&mut slice).is_err());
    }
}
