use std::io;

use super::{decode_message_header, encode_message_frame, MSG_PUBLISH_NAMESPACE};
use crate::wire::track_namespace::{
    decode_track_namespace, encode_track_namespace, TrackNamespace,
};
use crate::wire::varint::{decode_varint, encode_varint};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishNamespaceMessage {
    pub request_id: u64,
    pub required_request_id_delta: u64,
    pub track_namespace: TrackNamespace,
    // Parameters omitted in minimal implementation (count = 0)
}

impl PublishNamespaceMessage {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        let mut payload = Vec::new();
        encode_varint(self.request_id, &mut payload);
        encode_varint(self.required_request_id_delta, &mut payload);
        encode_track_namespace(&self.track_namespace, &mut payload);
        encode_varint(0, &mut payload); // Number of Parameters = 0
        encode_message_frame(MSG_PUBLISH_NAMESPACE, &payload, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> io::Result<Self> {
        let (msg_type, payload) = decode_message_header(buf)?;
        if msg_type != MSG_PUBLISH_NAMESPACE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "expected PUBLISH_NAMESPACE (0x{MSG_PUBLISH_NAMESPACE:X}), got 0x{msg_type:X}"
                ),
            ));
        }
        let mut p = payload.as_slice();
        let request_id = decode_varint(&mut p)?;
        let required_request_id_delta = decode_varint(&mut p)?;
        let track_namespace = decode_track_namespace(&mut p)?;
        let num_params = decode_varint(&mut p)?;
        if num_params != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "parameters not supported in minimal implementation",
            ));
        }
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
        msg.encode(&mut buf);
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
        msg.encode(&mut buf);
        let mut slice = buf.as_slice();
        let decoded = PublishNamespaceMessage::decode(&mut slice).unwrap();
        assert_eq!(msg, decoded);
    }
}
