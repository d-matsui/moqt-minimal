//! # publish_namespace: PUBLISH_NAMESPACE メッセージ
//!
//! パブリッシャーがリレーサーバーに対して「この名前空間でメディアを配信する」
//! ことを宣言するメッセージ。リレーはこの情報を使って、サブスクライバーの
//! SUBSCRIBE を適切なパブリッシャーに転送する。
//!
//! ## プロトコルフロー
//! 1. パブリッシャー → リレー: PUBLISH_NAMESPACE
//! 2. リレー → パブリッシャー: REQUEST_OK（受理）または REQUEST_ERROR（拒否）

use anyhow::{Result, ensure};

use super::{MSG_PUBLISH_NAMESPACE, decode_message_header, encode_message_frame};
use crate::wire::track_namespace::{
    TrackNamespace, decode_track_namespace, encode_track_namespace,
};
use crate::wire::varint::{decode_varint, encode_varint};

/// PUBLISH_NAMESPACE メッセージ。パブリッシャーが配信名前空間を登録する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishNamespaceMessage {
    pub request_id: u64,
    /// 依存する先行リクエストとの ID 差分。最小実装では常に 0。
    pub required_request_id_delta: u64,
    /// 登録する名前空間。
    pub track_namespace: TrackNamespace,
    // Parameters: 最小実装では省略（count = 0）
}

impl PublishNamespaceMessage {
    pub fn encode(&self, buf: &mut Vec<u8>) -> Result<()> {
        let mut payload = Vec::new();
        encode_varint(self.request_id, &mut payload);
        encode_varint(self.required_request_id_delta, &mut payload);
        encode_track_namespace(&self.track_namespace, &mut payload)?;
        // パラメータ数 = 0（最小実装）
        encode_varint(0, &mut payload);
        encode_message_frame(MSG_PUBLISH_NAMESPACE, &payload, buf);
        Ok(())
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let (msg_type, payload) = decode_message_header(buf)?;
        ensure!(
            msg_type == MSG_PUBLISH_NAMESPACE,
            "expected PUBLISH_NAMESPACE (0x{MSG_PUBLISH_NAMESPACE:X}), got 0x{msg_type:X}"
        );
        let mut p = payload.as_slice();
        let request_id = decode_varint(&mut p)?;
        let required_request_id_delta = decode_varint(&mut p)?;
        let track_namespace = decode_track_namespace(&mut p)?;
        let num_params = decode_varint(&mut p)?;
        ensure!(
            num_params == 0,
            "parameters not supported in minimal implementation"
        );
        Ok(PublishNamespaceMessage {
            request_id,
            required_request_id_delta,
            track_namespace,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let msg = PublishNamespaceMessage {
            request_id: 0,
            required_request_id_delta: 0,
            track_namespace: TrackNamespace {
                fields: vec![b"example".to_vec()],
            },
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();
        let mut slice = buf.as_slice();
        let decoded = PublishNamespaceMessage::decode(&mut slice).unwrap();
        assert_eq!(msg, decoded);
        assert!(slice.is_empty());
    }

    #[test]
    fn empty_namespace() {
        let msg = PublishNamespaceMessage {
            request_id: 2,
            required_request_id_delta: 0,
            track_namespace: TrackNamespace { fields: vec![] },
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();
        let mut slice = buf.as_slice();
        let decoded = PublishNamespaceMessage::decode(&mut slice).unwrap();
        assert_eq!(msg, decoded);
    }
}
