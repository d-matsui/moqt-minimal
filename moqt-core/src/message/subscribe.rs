//! # subscribe: SUBSCRIBE メッセージ
//!
//! サブスクライバーがリレー経由でパブリッシャーに対して
//! 特定のトラックの購読を要求するためのメッセージ。
//!
//! ## ワイヤーフォーマット
//! ```text
//! [Request ID (varint)]
//! [Required Request ID Delta (varint)]
//! [Track Namespace]
//! [Track Name Length (varint)] [Track Name]
//! [Parameters]
//! ```
//!
//! ## Request ID について
//! - クライアント発のリクエストは偶数 ID（0, 2, 4, ...）
//! - サーバー発のリクエストは奇数 ID（1, 3, 5, ...）
//! - Required Request ID Delta は依存関係の表現に使われる（最小実装では 0）

use anyhow::{Result, ensure};

use super::parameter::{MessageParameter, decode_parameters, encode_parameters};
use super::{MSG_SUBSCRIBE, decode_message_header, encode_message_frame};
use crate::wire::track_namespace::{
    TrackNamespace, decode_track_namespace, encode_track_namespace,
};
use crate::wire::varint::{decode_varint, encode_varint};

/// SUBSCRIBE メッセージ。特定のトラックの購読を要求する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscribeMessage {
    /// このリクエストの一意な ID。応答メッセージとの対応付けに使われる。
    pub request_id: u64,
    /// 依存する先行リクエストとの ID 差分。最小実装では常に 0。
    pub required_request_id_delta: u64,
    /// 購読対象のトラック名前空間。
    pub track_namespace: TrackNamespace,
    /// 購読対象のトラック名。名前空間と組み合わせてトラックを一意に特定する。
    pub track_name: Vec<u8>,
    /// 購読パラメータ（フィルタ、優先度など）。
    pub parameters: Vec<MessageParameter>,
}

impl SubscribeMessage {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        let mut payload = Vec::new();
        encode_varint(self.request_id, &mut payload);
        encode_varint(self.required_request_id_delta, &mut payload);
        encode_track_namespace(&self.track_namespace, &mut payload);
        // Track Name は長さ付きバイト列
        encode_varint(self.track_name.len() as u64, &mut payload);
        payload.extend_from_slice(&self.track_name);
        encode_parameters(&self.parameters, &mut payload);
        encode_message_frame(MSG_SUBSCRIBE, &payload, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let (msg_type, payload) = decode_message_header(buf)?;
        ensure!(
            msg_type == MSG_SUBSCRIBE,
            "expected SUBSCRIBE (0x{MSG_SUBSCRIBE:X}), got 0x{msg_type:X}"
        );
        let mut p = payload.as_slice();
        let request_id = decode_varint(&mut p)?;
        let required_request_id_delta = decode_varint(&mut p)?;
        let track_namespace = decode_track_namespace(&mut p)?;
        let name_len = decode_varint(&mut p)? as usize;
        ensure!(p.len() >= name_len, "track name truncated");
        let track_name = p[..name_len].to_vec();
        p = &p[name_len..];
        let parameters = decode_parameters(&mut p)?;
        Ok(SubscribeMessage {
            request_id,
            required_request_id_delta,
            track_namespace,
            track_name,
            parameters,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::parameter::SubscriptionFilter;

    fn roundtrip(msg: &SubscribeMessage) {
        let mut buf = Vec::new();
        msg.encode(&mut buf);
        let mut slice = buf.as_slice();
        let decoded = SubscribeMessage::decode(&mut slice).unwrap();
        assert_eq!(msg, &decoded);
        assert!(slice.is_empty());
    }

    #[test]
    fn with_filter() {
        let msg = SubscribeMessage {
            request_id: 0,
            required_request_id_delta: 0,
            track_namespace: TrackNamespace {
                fields: vec![b"example".to_vec()],
            },
            track_name: b"video".to_vec(),
            parameters: vec![MessageParameter::SubscriptionFilter(
                SubscriptionFilter::NextGroupStart,
            )],
        };
        roundtrip(&msg);
    }

    #[test]
    fn no_parameters() {
        let msg = SubscribeMessage {
            request_id: 2,
            required_request_id_delta: 0,
            track_namespace: TrackNamespace {
                fields: vec![b"example".to_vec()],
            },
            track_name: b"audio".to_vec(),
            parameters: vec![],
        };
        roundtrip(&msg);
    }

    #[test]
    fn request_id_odd() {
        let msg = SubscribeMessage {
            request_id: 1, // server-initiated
            required_request_id_delta: 0,
            track_namespace: TrackNamespace {
                fields: vec![b"test".to_vec()],
            },
            track_name: b"t".to_vec(),
            parameters: vec![],
        };
        roundtrip(&msg);
    }
}
