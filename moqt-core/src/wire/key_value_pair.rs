//! # key_value_pair: SETUP メッセージ等で使われるキー・バリューペア
//!
//! MOQT の SETUP メッセージでは、接続オプション（PATH, AUTHORITY 等）を
//! キー・バリューペアのリストとして送信する。
//!
//! ## デルタエンコーディング
//! ペアは Type の昇順で並べる必要があり、各ペアの Type は
//! 前のペアの Type との差分（デルタ）としてエンコードされる。
//! これにより、ワイヤー上のサイズを節約できる。
//!
//! ## 値の型の決定方法
//! Type ID が偶数なら値は varint、奇数なら長さ付きバイト列。
//! これにより、新しいパラメータ型を追加しても後方互換性を保てる。

use anyhow::{Result, ensure};

use super::varint::{decode_varint, encode_varint};

/// バリュー部の最大バイト長（奇数 Type の場合）。DoS 攻撃防止用。
const MAX_VALUE_LENGTH: u64 = (1 << 16) - 1; // 2^16 - 1 = 65535

/// デコード済みのキー・バリューペア。Type は絶対値（デルタではない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValuePair {
    pub type_id: u64,
    pub value: KvValue,
}

/// キー・バリューペアの値。Type ID の偶奇で型が決まる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KvValue {
    /// 偶数 Type: 値は varint 整数
    Varint(u64),
    /// 奇数 Type: 値は長さ付きバイト列
    Bytes(Vec<u8>),
}

/// キー・バリューペアのリストをエンコードする。
/// ペアは Type の昇順で並んでいる必要がある。
/// 各ペアの Type はデルタ（前のペアとの差分）としてエンコードされる。
pub fn encode_key_value_pairs(pairs: &[KeyValuePair], buf: &mut Vec<u8>) {
    let mut prev_type: u64 = 0;
    for pair in pairs {
        // デルタエンコーディング: 前の Type との差分を書き込む
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

/// バッファが空になるまでキー・バリューペアをデコードする。
/// デルタを累積して絶対 Type ID を復元する。
pub fn decode_key_value_pairs(buf: &mut &[u8]) -> Result<Vec<KeyValuePair>> {
    let mut pairs = Vec::new();
    let mut prev_type: u64 = 0;

    while !buf.is_empty() {
        let delta = decode_varint(buf)?;
        // デルタを累積して絶対 Type ID を復元。オーバーフローを検出する。
        let type_id = prev_type
            .checked_add(delta)
            .ok_or_else(|| anyhow::anyhow!("type overflow"))?;

        // Type ID の偶奇で値の解釈方法が異なる
        let value = if type_id % 2 == 0 {
            // 偶数 Type: varint 値
            KvValue::Varint(decode_varint(buf)?)
        } else {
            // 奇数 Type: 長さ付きバイト列
            let len = decode_varint(buf)?;
            ensure!(
                len <= MAX_VALUE_LENGTH,
                "value too long: {len} bytes (max {MAX_VALUE_LENGTH})"
            );
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
