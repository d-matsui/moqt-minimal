//! # track_namespace: MOQT トラック名前空間
//!
//! トラック名前空間は、パブリッシャーが配信するメディアストリームを識別するための
//! 階層的な名前構造。0〜32個のフィールド（各フィールドは1バイト以上のバイト列）で構成される。
//!
//! 例: `["example", "live"]` のような2フィールドの名前空間で、
//! パブリッシャーの配信先を一意に特定できる。
//!
//! ## ワイヤーフォーマット
//! ```text
//! [フィールド数 (varint)] [フィールド1の長さ (varint)] [フィールド1のデータ] ...
//! ```

use anyhow::{Result, ensure};

use super::varint::{decode_varint, encode_varint};

/// トラック名前空間: 0〜32個のバイト列フィールドの順序付きリスト。
/// Hash トレイトを実装しているので、HashMap のキーとして使える。
/// これはリレーサーバーでパブリッシャーの検索に利用される。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrackNamespace {
    pub fields: Vec<Vec<u8>>,
}

/// 仕様上のフィールド数の上限。DoS 攻撃防止のために制限している。
const MAX_FIELDS: u64 = 32;

/// トラック名前空間をバイト列にエンコードする。
/// フォーマット: [フィールド数] [各フィールドの長さ + データ]...
pub fn encode_track_namespace(ns: &TrackNamespace, buf: &mut Vec<u8>) {
    encode_varint(ns.fields.len() as u64, buf);
    for field in &ns.fields {
        encode_varint(field.len() as u64, buf);
        buf.extend_from_slice(field);
    }
}

/// バイト列からトラック名前空間をデコードする。
/// フィールド数が上限を超えたり、フィールド長が0の場合はエラーを返す。
pub fn decode_track_namespace(buf: &mut &[u8]) -> Result<TrackNamespace> {
    let num_fields = decode_varint(buf)?;
    ensure!(
        num_fields <= MAX_FIELDS,
        "too many namespace fields: {num_fields} (max {MAX_FIELDS})"
    );

    let mut fields = Vec::with_capacity(num_fields as usize);
    for _ in 0..num_fields {
        let field_len = decode_varint(buf)?;
        // 仕様により各フィールドは最低1バイト必要
        ensure!(field_len > 0, "namespace field length must be at least 1");
        let field_len = field_len as usize;
        ensure!(
            buf.len() >= field_len,
            "need {field_len} bytes for namespace field, have {}",
            buf.len()
        );
        fields.push(buf[..field_len].to_vec());
        *buf = &buf[field_len..];
    }

    Ok(TrackNamespace { fields })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(ns: &TrackNamespace) {
        let mut buf = Vec::new();
        encode_track_namespace(ns, &mut buf);
        let mut slice = buf.as_slice();
        let decoded = decode_track_namespace(&mut slice).unwrap();
        assert_eq!(ns, &decoded);
        assert!(slice.is_empty(), "all bytes should be consumed");
    }

    // 1.2: フィールド数0
    #[test]
    fn empty_namespace() {
        let ns = TrackNamespace { fields: vec![] };
        roundtrip(&ns);

        let mut buf = Vec::new();
        encode_track_namespace(&ns, &mut buf);
        // Number of Fields = 0 (1 byte varint)
        assert_eq!(buf, vec![0x00]);
    }

    // 1.2: フィールド数1
    #[test]
    fn single_field() {
        let ns = TrackNamespace {
            fields: vec![b"example".to_vec()],
        };
        roundtrip(&ns);
    }

    // 1.2: 複数フィールド
    #[test]
    fn multiple_fields() {
        let ns = TrackNamespace {
            fields: vec![b"example".to_vec(), b"live".to_vec()],
        };
        roundtrip(&ns);
    }

    // バイナリフィールド値
    #[test]
    fn binary_field_value() {
        let ns = TrackNamespace {
            fields: vec![vec![0x00, 0xff, 0x42]],
        };
        roundtrip(&ns);
    }

    // デコード: フィールド長0はエラー
    #[test]
    fn decode_zero_length_field_is_error() {
        // Number of Fields = 1, Field Length = 0
        let data = vec![0x01, 0x00];
        let mut slice = data.as_slice();
        assert!(decode_track_namespace(&mut slice).is_err());
    }

    // デコード: 33フィールド以上はエラー
    #[test]
    fn decode_too_many_fields_is_error() {
        let mut buf = Vec::new();
        encode_varint(33, &mut buf);
        for _ in 0..33 {
            encode_varint(1, &mut buf); // field length = 1
            buf.push(b'x'); // field value
        }
        let mut slice = buf.as_slice();
        assert!(decode_track_namespace(&mut slice).is_err());
    }

    // 32フィールドはOK
    #[test]
    fn decode_32_fields_ok() {
        let ns = TrackNamespace {
            fields: (0..32).map(|i| vec![b'a' + (i % 26) as u8]).collect(),
        };
        roundtrip(&ns);
    }

    // デコード: バッファ不足
    #[test]
    fn decode_truncated() {
        // Number of Fields = 1, Field Length = 5 だがデータが足りない
        let data = vec![0x01, 0x05, 0x41, 0x42];
        let mut slice = data.as_slice();
        assert!(decode_track_namespace(&mut slice).is_err());
    }
}
