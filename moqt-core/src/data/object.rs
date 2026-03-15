//! # object: サブグループ内のオブジェクト
//!
//! サブグループストリーム上で、SubgroupHeader の後に続くオブジェクトデータ。
//! 各オブジェクトはヘッダー（Object ID Delta + Payload Length）とペイロードで構成される。
//!
//! ## Object ID のデルタエンコーディング
//! Object ID は直接エンコードされず、前のオブジェクトとの差分（デルタ）で表現される。
//! - 最初のオブジェクト: Object ID = delta
//! - 2番目以降: Object ID = 前の Object ID + delta + 1
//!
//! 連番（0, 1, 2, ...）の場合、全てのオブジェクトで delta = 0 となり効率的。
//! デルタが 0 より大きい場合、間のオブジェクトは別のサブグループにあるか存在しない。

use anyhow::Result;

use crate::wire::varint::{decode_varint, encode_varint};

/// オブジェクトヘッダー。ペイロードの前に書き込まれる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectHeader {
    /// 前のオブジェクトとの Object ID 差分。連番なら常に 0。
    pub object_id_delta: u64,
    /// 後続するペイロードのバイト数。受信側はこの値で読み取り量を確定できる。
    pub payload_length: u64,
}

impl ObjectHeader {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        encode_varint(self.object_id_delta, buf);
        encode_varint(self.payload_length, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let object_id_delta = decode_varint(buf)?;
        let payload_length = decode_varint(buf)?;
        Ok(ObjectHeader {
            object_id_delta,
            payload_length,
        })
    }
}

/// デルタから絶対 Object ID を計算する。
/// - 最初のオブジェクト（prev_object_id = None）: Object ID = delta
/// - 2番目以降: Object ID = prev_object_id + delta + 1
///   （+1 は「連番の場合 delta=0 で済む」ようにするため）
pub fn resolve_object_id(prev_object_id: Option<u64>, delta: u64) -> u64 {
    match prev_object_id {
        None => delta,
        Some(prev) => prev + delta + 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(header: &ObjectHeader) {
        let mut buf = Vec::new();
        header.encode(&mut buf);
        let mut slice = buf.as_slice();
        let decoded = ObjectHeader::decode(&mut slice).unwrap();
        assert_eq!(header, &decoded);
        assert!(slice.is_empty());
    }

    #[test]
    fn basic() {
        roundtrip(&ObjectHeader {
            object_id_delta: 0,
            payload_length: 100,
        });
    }

    #[test]
    fn large_payload() {
        roundtrip(&ObjectHeader {
            object_id_delta: 0,
            payload_length: 1_000_000,
        });
    }

    #[test]
    fn nonzero_delta() {
        roundtrip(&ObjectHeader {
            object_id_delta: 5,
            payload_length: 50,
        });
    }

    // Object ID resolution: consecutive IDs (0,1,2,...) with delta=0
    #[test]
    fn resolve_consecutive_ids() {
        assert_eq!(resolve_object_id(None, 0), 0);
        assert_eq!(resolve_object_id(Some(0), 0), 1);
        assert_eq!(resolve_object_id(Some(1), 0), 2);
        assert_eq!(resolve_object_id(Some(2), 0), 3);
    }

    // Object ID resolution: first object with nonzero delta
    #[test]
    fn resolve_first_object_nonzero_delta() {
        assert_eq!(resolve_object_id(None, 5), 5);
    }

    // Object ID resolution: gap (delta > 0 for non-first)
    #[test]
    fn resolve_with_gap() {
        // prev=2, delta=2 → 2 + 2 + 1 = 5 (objects 3 and 4 are in different subgroups or don't exist)
        assert_eq!(resolve_object_id(Some(2), 2), 5);
    }

    // Multiple objects on a stream: encode/decode sequence
    #[test]
    fn multiple_objects_on_stream() {
        let objects = vec![
            (
                ObjectHeader {
                    object_id_delta: 0,
                    payload_length: 3,
                },
                b"abc".to_vec(),
            ),
            (
                ObjectHeader {
                    object_id_delta: 0,
                    payload_length: 3,
                },
                b"def".to_vec(),
            ),
            (
                ObjectHeader {
                    object_id_delta: 0,
                    payload_length: 3,
                },
                b"ghi".to_vec(),
            ),
        ];

        let mut buf = Vec::new();
        for (header, payload) in &objects {
            header.encode(&mut buf);
            buf.extend_from_slice(payload);
        }

        let mut slice = buf.as_slice();
        let mut prev_id: Option<u64> = None;
        for (i, (_, expected_payload)) in objects.iter().enumerate() {
            let header = ObjectHeader::decode(&mut slice).unwrap();
            let id = resolve_object_id(prev_id, header.object_id_delta);
            assert_eq!(id, i as u64);

            let payload = &slice[..header.payload_length as usize];
            slice = &slice[header.payload_length as usize..];
            assert_eq!(payload, expected_payload.as_slice());

            prev_id = Some(id);
        }
        assert!(slice.is_empty());
    }
}
