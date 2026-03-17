//! # setup: SETUP message (Section 9.4)
//!
//! The first message exchanged between client and server to establish a MOQT session.
//! Contains connection parameters (PATH, AUTHORITY, etc.) as Setup Options.
//!
//! ## Protocol flow
//! 1. Client -> Server: SETUP (with PATH="/", AUTHORITY="localhost", etc.)
//! 2. Server -> Client: SETUP (typically with empty Options)
//!
//! ## Wire format
//! SETUP has a special Type ID of 0x2F00, which is also used as the
//! control stream type. The payload is a list of Key-Value-Pairs.

use anyhow::{Result, ensure};

use super::{MSG_SETUP, decode_message, encode_message};
use crate::wire::key_value_pair::{
    KeyValuePair, KvValue, decode_key_value_pairs, encode_key_value_pairs,
};

/// Known Setup Option Type IDs
const OPTION_PATH: u64 = 0x01;
const OPTION_AUTHORITY: u64 = 0x05;

/// SETUP message. Exchanged by both sides during session establishment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupMessage {
    pub setup_options: Vec<SetupOption>,
}

/// Setup Option types.
/// PATH and AUTHORITY are standard options defined in the spec.
/// Unknown options are preserved for forward compatibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupOption {
    /// Path of the MoQ URI (Option Type 0x01).
    Path(Vec<u8>),
    /// Authority component of the MoQ URI (Option Type 0x05).
    Authority(Vec<u8>),
    /// Unknown option. The spec requires receivers to ignore unrecognized options,
    /// so we preserve them without error.
    Unknown { type_id: u64, value: KvValue },
}

impl SetupMessage {
    /// Encode a SETUP message.
    /// Converts Setup Options to Key-Value-Pairs and wraps them
    /// in a control message (Type + Length + Payload).
    pub fn encode(&self, buf: &mut Vec<u8>) -> Result<()> {
        // Convert SetupOptions to KeyValuePairs
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
        encode_key_value_pairs(&kvs, &mut payload)?;
        encode_message(MSG_SETUP, &payload, buf);
        Ok(())
    }

    /// Decode a SETUP message from bytes.
    /// Returns an error if the message type is not MSG_SETUP.
    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let (msg_type, payload) = decode_message(buf)?;
        ensure!(
            msg_type == MSG_SETUP,
            "expected SETUP (0x{MSG_SETUP:X}), got 0x{msg_type:X}"
        );

        let mut payload_slice = payload.as_slice();
        let kvs = decode_key_value_pairs(&mut payload_slice)?;

        // Convert known Type IDs to SetupOption variants,
        // preserve unknown ones as Unknown
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
        msg.encode(&mut buf).unwrap();
        let mut slice = buf.as_slice();
        let decoded = SetupMessage::decode(&mut slice).unwrap();
        assert_eq!(msg, &decoded);
        assert!(slice.is_empty());
    }

    // Client SETUP with PATH and AUTHORITY
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

    // Server SETUP with empty options
    #[test]
    fn server_setup_empty_options() {
        let msg = SetupMessage {
            setup_options: vec![],
        };
        roundtrip(&msg);
    }

    // Unknown Setup Option is preserved through roundtrip
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

    // Type ID 0x2F00 is correctly encoded on wire
    #[test]
    fn message_type_on_wire() {
        let msg = SetupMessage {
            setup_options: vec![],
        };
        let mut buf = Vec::new();
        msg.encode(&mut buf).unwrap();
        // 0x2F00 as varint: 2 bytes (prefix 10), value = 0x2F00
        // 1st byte: 0x80 | (0x2F00 >> 8) = 0x80 | 0x2F = 0xAF
        // 2nd byte: 0x00
        assert_eq!(buf[0], 0xAF);
        assert_eq!(buf[1], 0x00);
    }

    // Wrong message type is an error
    #[test]
    fn wrong_message_type_is_error() {
        let mut buf = Vec::new();
        encode_message(0x03, &[], &mut buf); // SUBSCRIBE type, not SETUP
        let mut slice = buf.as_slice();
        assert!(SetupMessage::decode(&mut slice).is_err());
    }
}
