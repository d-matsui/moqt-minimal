//! # parameter: SUBSCRIBE 等のメッセージに付加されるパラメータ
//!
//! SUBSCRIBE や SUBSCRIBE_OK メッセージには、購読の振る舞いを制御する
//! パラメータを付加できる。key_value_pair と同様にデルタエンコーディングを使う。
//!
//! ## 最小実装でサポートするパラメータ
//! - `SUBSCRIPTION_FILTER` (0x21): 購読開始位置のフィルタ（例: 次のグループ先頭から）
//! - `LARGEST_OBJECT` (0x09): パブリッシャーが持つ最大オブジェクト位置
//! - `FORWARD` (0x10): 転送方向の指定

use anyhow::{Result, bail, ensure};

use crate::wire::varint::{decode_varint, encode_varint};

// パラメータタイプ ID（仕様で定義されたもの）
pub const PARAM_DELIVERY_TIMEOUT: u64 = 0x02;
pub const PARAM_AUTHORIZATION_TOKEN: u64 = 0x03;
pub const PARAM_EXPIRES: u64 = 0x08;
pub const PARAM_LARGEST_OBJECT: u64 = 0x09;
pub const PARAM_FORWARD: u64 = 0x10;
pub const PARAM_SUBSCRIBER_PRIORITY: u64 = 0x20;
pub const PARAM_SUBSCRIPTION_FILTER: u64 = 0x21;
pub const PARAM_GROUP_ORDER: u64 = 0x22;

/// 最小実装で扱うメッセージパラメータ。
/// 未知のパラメータは現状エラーにしている（将来的には無視する実装に変更可能）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageParameter {
    /// SUBSCRIPTION_FILTER (0x21): 購読フィルタ。
    /// サブスクライバーが「どこからデータを受け取りたいか」を指定する。
    SubscriptionFilter(SubscriptionFilter),
    /// LARGEST_OBJECT (0x09): パブリッシャーが持つ最新のオブジェクト位置。
    /// (Group ID, Object ID) のペアで表現される。
    LargestObject { group: u64, object: u64 },
    /// FORWARD (0x10): 転送方向（0 または 1）。
    Forward(u8),
}

/// 購読フィルタの種類。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionFilter {
    /// 次のグループの先頭から受信開始する。
    /// ライブ配信で最も一般的なフィルタ。
    NextGroupStart, // 0x1
}

/// メッセージパラメータのリストをエンコードする。
/// パラメータは Type の昇順で並んでいる必要がある。
/// フォーマット: [パラメータ数 (varint)] [各パラメータのデルタType + 値]...
pub fn encode_parameters(params: &[MessageParameter], buf: &mut Vec<u8>) {
    encode_varint(params.len() as u64, buf);
    let mut prev_type: u64 = 0;
    for param in params {
        let type_id = param_type_id(param);
        let delta = type_id - prev_type;
        encode_varint(delta, buf);
        match param {
            MessageParameter::SubscriptionFilter(filter) => {
                // 長さ付きの内部データ（フィルタタイプを varint でエンコード）
                let mut inner = Vec::new();
                match filter {
                    SubscriptionFilter::NextGroupStart => encode_varint(0x1, &mut inner),
                }
                encode_varint(inner.len() as u64, buf);
                buf.extend_from_slice(&inner);
            }
            MessageParameter::LargestObject { group, object } => {
                // Location: Group ID と Object ID の2つの varint を連続して書く
                encode_varint(*group, buf);
                encode_varint(*object, buf);
            }
            MessageParameter::Forward(v) => {
                // 単純な 1 バイト値
                buf.push(*v);
            }
        }
        prev_type = type_id;
    }
}

/// メッセージパラメータのリストをデコードする。
pub fn decode_parameters(buf: &mut &[u8]) -> Result<Vec<MessageParameter>> {
    let count = decode_varint(buf)?;
    let mut params = Vec::with_capacity(count as usize);
    let mut prev_type: u64 = 0;
    for _ in 0..count {
        let delta = decode_varint(buf)?;
        let type_id = prev_type
            .checked_add(delta)
            .ok_or_else(|| anyhow::anyhow!("parameter type overflow"))?;
        let param = match type_id {
            PARAM_SUBSCRIPTION_FILTER => {
                // 長さ付きの内部データを読んでフィルタタイプを取得
                let len = decode_varint(buf)? as usize;
                ensure!(buf.len() >= len, "subscription filter truncated");
                let mut inner = &buf[..len];
                let filter_type = decode_varint(&mut inner)?;
                *buf = &buf[len..];
                match filter_type {
                    0x1 => MessageParameter::SubscriptionFilter(SubscriptionFilter::NextGroupStart),
                    _ => {
                        bail!("unsupported filter type: 0x{filter_type:X}");
                    }
                }
            }
            PARAM_LARGEST_OBJECT => {
                let group = decode_varint(buf)?;
                let object = decode_varint(buf)?;
                MessageParameter::LargestObject { group, object }
            }
            PARAM_FORWARD => {
                ensure!(!buf.is_empty(), "forward parameter truncated");
                let v = buf[0];
                *buf = &buf[1..];
                MessageParameter::Forward(v)
            }
            _ => {
                bail!("unknown parameter type: 0x{type_id:X}");
            }
        };
        params.push(param);
        prev_type = type_id;
    }
    Ok(params)
}

/// パラメータの種類から Type ID を取得する。
fn param_type_id(param: &MessageParameter) -> u64 {
    match param {
        MessageParameter::LargestObject { .. } => PARAM_LARGEST_OBJECT,
        MessageParameter::Forward(_) => PARAM_FORWARD,
        MessageParameter::SubscriptionFilter(_) => PARAM_SUBSCRIPTION_FILTER,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(params: &[MessageParameter]) {
        let mut buf = Vec::new();
        encode_parameters(params, &mut buf);
        let mut slice = buf.as_slice();
        let decoded = decode_parameters(&mut slice).unwrap();
        assert_eq!(params, decoded.as_slice());
        assert!(slice.is_empty());
    }

    #[test]
    fn empty_params() {
        roundtrip(&[]);
    }

    #[test]
    fn subscription_filter_next_group_start() {
        roundtrip(&[MessageParameter::SubscriptionFilter(
            SubscriptionFilter::NextGroupStart,
        )]);
    }

    #[test]
    fn largest_object() {
        roundtrip(&[MessageParameter::LargestObject {
            group: 10,
            object: 5,
        }]);
    }

    #[test]
    fn forward() {
        roundtrip(&[MessageParameter::Forward(1)]);
    }

    #[test]
    fn multiple_params_ascending_order() {
        roundtrip(&[
            MessageParameter::LargestObject {
                group: 3,
                object: 0,
            },
            MessageParameter::Forward(1),
            MessageParameter::SubscriptionFilter(SubscriptionFilter::NextGroupStart),
        ]);
    }
}
