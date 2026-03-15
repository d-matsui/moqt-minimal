use anyhow::{Result, ensure};

use super::{MSG_REQUEST_OK, decode_message_header, encode_message_frame};
use crate::wire::varint::{decode_varint, encode_varint};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestOkMessage {
    // Parameters omitted in minimal implementation (count = 0)
}

impl RequestOkMessage {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        let mut payload = Vec::new();
        encode_varint(0, &mut payload); // Number of Parameters = 0
        encode_message_frame(MSG_REQUEST_OK, &payload, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let (msg_type, payload) = decode_message_header(buf)?;
        ensure!(
            msg_type == MSG_REQUEST_OK,
            "expected REQUEST_OK (0x{MSG_REQUEST_OK:X}), got 0x{msg_type:X}"
        );
        let mut p = payload.as_slice();
        let num_params = decode_varint(&mut p)?;
        ensure!(
            num_params == 0,
            "parameters not supported in minimal implementation"
        );
        Ok(RequestOkMessage {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let msg = RequestOkMessage {};
        let mut buf = Vec::new();
        msg.encode(&mut buf);
        let mut slice = buf.as_slice();
        let decoded = RequestOkMessage::decode(&mut slice).unwrap();
        assert_eq!(msg, decoded);
        assert!(slice.is_empty());
    }
}
