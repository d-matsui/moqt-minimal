//! # message: MOQT 制御メッセージの定義
//!
//! MOQT セッション上でやり取りされる制御メッセージを定義する。
//! 各メッセージは双方向ストリーム（bidi stream）上で送受信される。
//!
//! ## メッセージフレーム構造
//! ```text
//! [メッセージタイプ (varint)] [ペイロード長 (u16 BE)] [ペイロード]
//! ```
//! ペイロード長は 16-bit ビッグエンディアン固定長。varint ではない点に注意。
//!
//! ## メッセージの流れ（典型例）
//! 1. SETUP: 接続確立時に双方が送り合う
//! 2. PUBLISH_NAMESPACE: パブリッシャーが配信可能な名前空間を登録
//! 3. SUBSCRIBE: サブスクライバーが特定トラックの購読を要求
//! 4. SUBSCRIBE_OK / REQUEST_ERROR: 購読の成功/失敗を応答
//! 5. PUBLISH_DONE: パブリッシャーが配信終了を通知

pub mod parameter;
pub mod publish_done;
pub mod publish_namespace;
pub mod request_error;
pub mod request_ok;
pub mod setup;
pub mod subscribe;
pub mod subscribe_ok;

use anyhow::{Result, ensure};

use crate::wire::varint::{decode_varint, encode_varint};

/// メッセージタイプ ID（仕様 Section 9 で定義）
pub const MSG_SUBSCRIBE: u64 = 0x03;
pub const MSG_SUBSCRIBE_OK: u64 = 0x04;
pub const MSG_REQUEST_ERROR: u64 = 0x05;
pub const MSG_PUBLISH_NAMESPACE: u64 = 0x06;
pub const MSG_REQUEST_OK: u64 = 0x07;
pub const MSG_PUBLISH_DONE: u64 = 0x0B;
/// SETUP メッセージは特殊な Type ID（0x2F00）を持つ。
/// コントロールストリームの識別にも使われる。
pub const MSG_SETUP: u64 = 0x2F00;

/// 制御メッセージフレームをエンコードする。
/// フォーマット: [Type (varint)] [Length (u16 BE)] [Payload]
///
/// Length が u16 固定長なのは、メッセージサイズの上限を明示するため。
/// varint にしないことで、受信側が事前にバッファサイズを確定できる。
pub fn encode_message_frame(msg_type: u64, payload: &[u8], buf: &mut Vec<u8>) {
    encode_varint(msg_type, buf);
    let len = payload.len() as u16;
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(payload);
}

/// 制御メッセージフレームヘッダーをデコードし、(メッセージタイプ, ペイロード) を返す。
pub fn decode_message_header(buf: &mut &[u8]) -> Result<(u64, Vec<u8>)> {
    let msg_type = decode_varint(buf)?;
    ensure!(buf.len() >= 2, "need 2 bytes for message length");
    let len = u16::from_be_bytes([buf[0], buf[1]]) as usize;
    *buf = &buf[2..];
    ensure!(
        buf.len() >= len,
        "need {len} bytes for message payload, have {}",
        buf.len()
    );
    let payload = buf[..len].to_vec();
    *buf = &buf[len..];
    Ok((msg_type, payload))
}
