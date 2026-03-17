//! # subscribe_ok: SUBSCRIBE_OK メッセージ
//!
//! SUBSCRIBE の成功応答。パブリッシャーが購読を受け入れたことを示す。
//! Track Alias を含み、以降のデータストリームでこの Alias を使って
//! トラックを識別する。
//!
//! ## ワイヤーフォーマット
//! ```text
//! [Track Alias (varint)]
//! [Parameters]
//! [Track Properties Length (varint)] [Track Properties...]
//! ```

use anyhow::{Result, ensure};

use super::parameter::{MessageParameter, decode_parameters, encode_parameters};
use super::{MSG_SUBSCRIBE_OK, decode_message, encode_message};
use crate::wire::varint::{decode_varint, encode_varint};

/// SUBSCRIBE_OK メッセージ。購読の成功応答。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscribeOkMessage {
    /// パブリッシャーが割り当てたトラックエイリアス。
    /// 以降のデータストリーム（SubgroupHeader）でトラックの識別に使われる。
    /// これにより、フルのトラック名前空間+トラック名をデータストリームごとに
    /// 送る必要がなくなる。
    pub track_alias: u64,
    /// 応答パラメータ（LARGEST_OBJECT 等）。
    pub parameters: Vec<MessageParameter>,
    // Track Properties: 最小実装では空
}

impl SubscribeOkMessage {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        let mut payload = Vec::new();
        encode_varint(self.track_alias, &mut payload);
        encode_parameters(&self.parameters, &mut payload);
        // Track Properties: 最小実装では空（長さ 0）
        encode_varint(0, &mut payload);
        encode_message(MSG_SUBSCRIBE_OK, &payload, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let (msg_type, payload) = decode_message(buf)?;
        ensure!(
            msg_type == MSG_SUBSCRIBE_OK,
            "expected SUBSCRIBE_OK (0x{MSG_SUBSCRIBE_OK:X}), got 0x{msg_type:X}"
        );
        let mut p = payload.as_slice();
        let track_alias = decode_varint(&mut p)?;
        let parameters = decode_parameters(&mut p)?;
        // Track Properties をスキップ（長さを読んでその分をスキップ）
        let props_len = decode_varint(&mut p)? as usize;
        ensure!(p.len() >= props_len, "track properties truncated");
        let _ = &p[..props_len];
        Ok(SubscribeOkMessage {
            track_alias,
            parameters,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::parameter::MessageParameter;

    fn roundtrip(msg: &SubscribeOkMessage) {
        let mut buf = Vec::new();
        msg.encode(&mut buf);
        let mut slice = buf.as_slice();
        let decoded = SubscribeOkMessage::decode(&mut slice).unwrap();
        assert_eq!(msg, &decoded);
        assert!(slice.is_empty());
    }

    #[test]
    fn basic() {
        let msg = SubscribeOkMessage {
            track_alias: 1,
            parameters: vec![],
        };
        roundtrip(&msg);
    }

    #[test]
    fn with_largest_object() {
        let msg = SubscribeOkMessage {
            track_alias: 42,
            parameters: vec![MessageParameter::LargestObject {
                group: 10,
                object: 5,
            }],
        };
        roundtrip(&msg);
    }

    #[test]
    fn track_properties_empty() {
        let msg = SubscribeOkMessage {
            track_alias: 0,
            parameters: vec![],
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf);
        // Verify Track Properties length = 0 is at the end of payload
        let mut slice = buf.as_slice();
        let decoded = SubscribeOkMessage::decode(&mut slice).unwrap();
        assert_eq!(decoded.track_alias, 0);
    }
}
