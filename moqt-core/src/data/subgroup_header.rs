//! # subgroup_header: サブグループヘッダー
//!
//! QUIC 単方向ストリームの先頭に書き込まれるヘッダー。
//! どのトラック（Track Alias）のどのグループ（Group ID）のデータかを示す。
//!
//! ## Subgroup Header Type (0x38) のビットフィールド
//! ```text
//! ビット 0   (PROPERTIES):       0 — プロパティなし
//! ビット 1-2 (SUBGROUP_ID_MODE): 00 — Subgroup ID = 0（省略）
//! ビット 3   (END_OF_GROUP):     1 — このストリームでグループが完結する
//! ビット 4   (固定):             1 — Subgroup Header であることを示す
//! ビット 5   (DEFAULT_PRIORITY): 1 — デフォルト優先度を使用
//! ```
//! → 0b00111000 = 0x38
//!
//! 最小実装では、1つの Group = 1つの Subgroup = 1つの QUIC ストリーム。

use anyhow::{Result, ensure};

use crate::wire::varint::{decode_varint, encode_varint};

/// 最小実装で使用する Subgroup Header Type。
/// ビットフィールドの意味は上記モジュールドキュメント参照。
const SUBGROUP_TYPE: u64 = 0x38;

/// サブグループヘッダー。QUIC 単方向ストリームの先頭に書き込まれる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubgroupHeader {
    /// SUBSCRIBE_OK で割り当てられたトラックエイリアス。
    /// フルのトラック名前空間を毎回送る代わりに、短い数値で識別する。
    pub track_alias: u64,
    /// このストリームが含むグループの ID。連番で増加する。
    pub group_id: u64,
}

impl SubgroupHeader {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        encode_varint(SUBGROUP_TYPE, buf);
        encode_varint(self.track_alias, buf);
        encode_varint(self.group_id, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let stream_type = decode_varint(buf)?;
        ensure!(
            stream_type == SUBGROUP_TYPE,
            "expected SUBGROUP_HEADER type 0x{SUBGROUP_TYPE:X}, got 0x{stream_type:X}"
        );
        let track_alias = decode_varint(buf)?;
        let group_id = decode_varint(buf)?;
        Ok(SubgroupHeader {
            track_alias,
            group_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(header: &SubgroupHeader) {
        let mut buf = Vec::new();
        header.encode(&mut buf);
        let mut slice = buf.as_slice();
        let decoded = SubgroupHeader::decode(&mut slice).unwrap();
        assert_eq!(header, &decoded);
        assert!(slice.is_empty());
    }

    #[test]
    fn basic() {
        roundtrip(&SubgroupHeader {
            track_alias: 1,
            group_id: 0,
        });
    }

    #[test]
    fn large_ids() {
        roundtrip(&SubgroupHeader {
            track_alias: 42,
            group_id: 1000,
        });
    }

    #[test]
    fn type_on_wire() {
        let header = SubgroupHeader {
            track_alias: 0,
            group_id: 0,
        };
        let mut buf = Vec::new();
        header.encode(&mut buf);
        // 0x38 fits in 1-byte varint
        assert_eq!(buf[0], 0x38);
    }

    #[test]
    fn wrong_type() {
        let mut buf = Vec::new();
        encode_varint(0x10, &mut buf); // wrong type
        encode_varint(1, &mut buf);
        encode_varint(0, &mut buf);
        let mut slice = buf.as_slice();
        assert!(SubgroupHeader::decode(&mut slice).is_err());
    }
}
