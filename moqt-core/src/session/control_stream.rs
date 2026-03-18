//! # control_stream: MOQT コントロールストリームの読み書き
//!
//! MOQT では、各ピアが QUIC 単方向ストリームを使ってコントロールメッセージを送受信する。
//! コントロールストリームの最初のメッセージは必ず SETUP。
//!
//! ## QUIC ストリームからの varint 読み取りの課題
//! varint はバイト列から一括でデコードできるが、QUIC ストリームでは
//! データが少しずつ到着する。そのため:
//! 1. まず先頭の1バイトを読む
//! 2. プレフィックスビットから必要な残りバイト数を判定
//! 3. 残りバイトを読む
//! 4. 全バイトを結合して decode_varint でデコード
//!
//! この「ストリーミング varint 読み取り」パターンは、control_stream と
//! relay の両方で使われている。

use anyhow::{Result, bail};

use quinn::{RecvStream, SendStream};

use crate::message::MSG_SETUP;
use crate::primitives::varint::{decode_varint, encode_varint};

/// MOQT コントロールストリームの書き込み側。
/// 各ピアが1本ずつ開く QUIC 単方向ストリームに対応する。
pub struct ControlStreamWriter {
    stream: SendStream,
}

impl ControlStreamWriter {
    pub fn new(stream: SendStream) -> Self {
        Self { stream }
    }

    /// コントロールストリームに SETUP メッセージを書き込む。
    /// ストリームタイプ（0x2F00）→ ペイロード長 → ペイロード の順に書く。
    ///
    /// コントロールストリームの先頭には、ストリームの種類を示すために
    /// SETUP の Type ID (0x2F00) を varint として書き込む必要がある。
    pub async fn write_setup(&mut self, msg: &crate::message::setup::SetupMessage) -> Result<()> {
        // ストリームタイプ = SETUP (0x2F00) を最初に書く
        let mut buf = Vec::new();
        encode_varint(MSG_SETUP, &mut buf);

        // SETUP メッセージのペイロード（Setup Options を Key-Value-Pair でエンコード）
        let mut payload = Vec::new();
        use crate::primitives::key_value_pair::{KeyValuePair, KvValue, encode_key_value_pairs};
        let kvs: Vec<KeyValuePair> = msg
            .setup_options
            .iter()
            .map(|opt| match opt {
                crate::message::setup::SetupOption::Path(v) => KeyValuePair {
                    type_id: 0x01,
                    value: KvValue::Bytes(v.clone()),
                },
                crate::message::setup::SetupOption::Authority(v) => KeyValuePair {
                    type_id: 0x05,
                    value: KvValue::Bytes(v.clone()),
                },
                crate::message::setup::SetupOption::Unknown { type_id, value } => KeyValuePair {
                    type_id: *type_id,
                    value: value.clone(),
                },
            })
            .collect();
        encode_key_value_pairs(&kvs, &mut payload)?;

        // ペイロード長を u16 BE で書き、続けてペイロードを書く
        let len = payload.len() as u16;
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(&payload);

        self.stream.write_all(&buf).await?;
        Ok(())
    }

    /// フレーム済みの制御メッセージ（Type + Length + Payload）をそのまま書き込む。
    pub async fn write_raw(&mut self, data: &[u8]) -> Result<()> {
        self.stream.write_all(data).await?;
        Ok(())
    }
}

/// MOQT コントロールストリームの読み取り側。
pub struct ControlStreamReader {
    stream: RecvStream,
}

impl ControlStreamReader {
    pub fn new(stream: RecvStream) -> Self {
        Self { stream }
    }

    /// コントロールストリームから SETUP メッセージを読み取る。
    /// ストリームタイプ (0x2F00) + SETUP メッセージのバイト列を期待する。
    pub async fn read_setup(&mut self) -> Result<crate::message::setup::SetupMessage> {
        let buf = self.read_message_bytes().await?;
        let mut slice = buf.as_slice();
        crate::message::setup::SetupMessage::decode(&mut slice)
    }

    /// コントロールストリームから1つの制御メッセージを読み取り、
    /// フレーム全体（Type + Length + Payload）のバイト列を返す。
    ///
    /// QUIC ストリームからは少しずつデータが届くため、以下の手順で読む:
    /// 1. varint でメッセージタイプを読む
    /// 2. 2バイトのペイロード長を読む
    /// 3. ペイロード長分のデータを読む
    /// 4. 全てを結合して返す
    pub async fn read_message_bytes(&mut self) -> Result<Vec<u8>> {
        // メッセージタイプ（varint）を読む
        let (_type_val, type_bytes) = self.read_varint().await?;

        // ペイロード長（2バイト、ビッグエンディアン u16）を読む
        let mut len_bytes = [0u8; 2];
        self.stream.read_exact(&mut len_bytes).await?;
        let payload_len = u16::from_be_bytes(len_bytes) as usize;

        // ペイロード本体を読む
        let mut payload = vec![0u8; payload_len];
        if payload_len > 0 {
            self.stream.read_exact(&mut payload).await?;
        }

        // 読んだ全バイトを結合して返す（上位で decode するため）
        let mut result = Vec::with_capacity(type_bytes.len() + 2 + payload_len);
        result.extend_from_slice(&type_bytes);
        result.extend_from_slice(&len_bytes);
        result.extend_from_slice(&payload);
        Ok(result)
    }

    /// QUIC ストリームから varint を1つ読み取る。
    /// (デコードされた値, 生のバイト列) を返す。
    ///
    /// ストリームからは1バイトずつしか読めない場合があるため、
    /// まず先頭1バイトでバイト長を判定してから残りを読む。
    async fn read_varint(&mut self) -> Result<(u64, Vec<u8>)> {
        // 先頭1バイトを読んでバイト長を判定
        let mut first = [0u8; 1];
        self.stream.read_exact(&mut first).await?;

        let byte = first[0];
        // varint のプレフィックスビットから総バイト数を決定
        // （encode_varint / decode_varint と同じロジック）
        let total_len = if byte & 0x80 == 0 {
            1
        } else if byte & 0xc0 == 0x80 {
            2
        } else if byte & 0xe0 == 0xc0 {
            3
        } else if byte & 0xf0 == 0xe0 {
            4
        } else if byte & 0xf8 == 0xf0 {
            5
        } else if byte & 0xfc == 0xf8 {
            6
        } else if byte == 0xfc {
            bail!("invalid varint code point 0xFC");
        } else if byte == 0xfe {
            8
        } else {
            9
        };

        // 残りのバイトを読む
        let mut raw = vec![0u8; total_len];
        raw[0] = byte;
        if total_len > 1 {
            self.stream.read_exact(&mut raw[1..]).await?;
        }

        // 完全なバイト列から値をデコード
        let mut slice = raw.as_slice();
        let value = decode_varint(&mut slice)?;
        Ok((value, raw))
    }
}
