use anyhow::{Result, ensure};

use super::{MSG_SETUP, decode_message_header, encode_message_frame};
use crate::wire::key_value_pair::{
    KeyValuePair, KvValue, decode_key_value_pairs, encode_key_value_pairs,
};

const OPTION_PATH: u64 = 0x01;
const OPTION_AUTHORITY: u64 = 0x05;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupMessage {
    pub setup_options: Vec<SetupOption>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupOption {
    Path(Vec<u8>),
    Authority(Vec<u8>),
    Unknown { type_id: u64, value: KvValue },
}

impl SetupMessage {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        let kvs: Vec<KeyValuePair> = self
            .setup_options
            .iter()
            .map(|opt| match opt {
                SetupOption::Path(v) => KeyValuePair {
                    type_id: OPTION_PATH,
                    value: KvValue::Bytes(v.clone()),
                },
                SetupOption::Authority(v) => KeyValuePair {
                    type_id: OPTION_AUTHORITY,
                    value: KvValue::Bytes(v.clone()),
                },
                SetupOption::Unknown { type_id, value } => KeyValuePair {
                    type_id: *type_id,
                    value: value.clone(),
                },
            })
            .collect();

        let mut payload = Vec::new();
        encode_key_value_pairs(&kvs, &mut payload);
        encode_message_frame(MSG_SETUP, &payload, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let (msg_type, payload) = decode_message_header(buf)?;
        ensure!(
            msg_type == MSG_SETUP,
            "expected SETUP (0x{MSG_SETUP:X}), got 0x{msg_type:X}"
        );

        let mut payload_slice = payload.as_slice();
        let kvs = decode_key_value_pairs(&mut payload_slice)?;

        let setup_options = kvs
            .into_iter()
            .map(|kv| match kv.type_id {
                OPTION_PATH => match kv.value {
                    KvValue::Bytes(v) => SetupOption::Path(v),
                    _ => SetupOption::Unknown {
                        type_id: kv.type_id,
                        value: kv.value,
                    },
                },
                OPTION_AUTHORITY => match kv.value {
                    KvValue::Bytes(v) => SetupOption::Authority(v),
                    _ => SetupOption::Unknown {
                        type_id: kv.type_id,
                        value: kv.value,
                    },
                },
                _ => SetupOption::Unknown {
                    type_id: kv.type_id,
                    value: kv.value,
                },
            })
            .collect();

        Ok(SetupMessage { setup_options })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(msg: &SetupMessage) {
        let mut buf = Vec::new();
        msg.encode(&mut buf);
        let mut slice = buf.as_slice();
        let decoded = SetupMessage::decode(&mut slice).unwrap();
        assert_eq!(msg, &decoded);
        assert!(slice.is_empty());
    }

    // 2.1: client の SETUP（PATH, AUTHORITY 付き）
    #[test]
    fn client_setup_with_path_and_authority() {
        let msg = SetupMessage {
            setup_options: vec![
                SetupOption::Path(b"/".to_vec()),
                SetupOption::Authority(b"localhost".to_vec()),
            ],
        };
        roundtrip(&msg);
    }

    // 2.1: server の SETUP（Setup Options 空）
    #[test]
    fn server_setup_empty_options() {
        let msg = SetupMessage {
            setup_options: vec![],
        };
        roundtrip(&msg);
    }

    // 2.1: 未知の Setup Option を無視してパースが継続できる
    #[test]
    fn unknown_option_preserved() {
        let msg = SetupMessage {
            setup_options: vec![
                SetupOption::Path(b"/".to_vec()),
                SetupOption::Unknown {
                    type_id: 0x07,
                    value: KvValue::Bytes(b"my-impl/1.0".to_vec()),
                },
            ],
        };
        roundtrip(&msg);
    }

    // メッセージフレーム: Type = 0x2F00 がワイヤ上で正しくエンコードされる
    #[test]
    fn message_type_on_wire() {
        let msg = SetupMessage {
            setup_options: vec![],
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf);
        // 0x2F00 as varint: 2 bytes (10 prefix), value = 0x2F00
        // First byte: 0x80 | (0x2F00 >> 8) = 0x80 | 0x2F = 0xAF
        // Second byte: 0x00
        assert_eq!(buf[0], 0xAF);
        assert_eq!(buf[1], 0x00);
    }

    // デコード: 間違った message type
    #[test]
    fn wrong_message_type() {
        let mut buf = Vec::new();
        encode_message_frame(0x03, &[], &mut buf); // SUBSCRIBE type, not SETUP
        let mut slice = buf.as_slice();
        assert!(SetupMessage::decode(&mut slice).is_err());
    }
}
