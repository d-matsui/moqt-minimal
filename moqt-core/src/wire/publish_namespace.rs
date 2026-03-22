//! # publish_namespace: PUBLISH_NAMESPACE message (Section 9.17)
//!
//! A publisher sends this message to the relay to declare that it will
//! publish media under a given namespace. The relay uses this information
//! to route SUBSCRIBEs to the appropriate publisher.
//!
//! ## Protocol flow
//! 1. Publisher -> Relay: PUBLISH_NAMESPACE
//! 2. Relay -> Publisher: REQUEST_OK (accepted) or REQUEST_ERROR (rejected)

use anyhow::{Result, ensure};

use super::{MSG_PUBLISH_NAMESPACE, decode_message, encode_message};
use crate::wire::parameter::{decode_parameters, encode_parameters};
use crate::wire::track_namespace::{
    TrackNamespace, decode_track_namespace, encode_track_namespace,
};
use crate::wire::varint::{decode_varint, encode_varint};

/// PUBLISH_NAMESPACE message. Registers a namespace that the publisher will publish to.
///
/// ```text
/// Type (vi64) = 0x6,
/// Length (u16),
/// Request ID (vi64),
/// Required Request ID Delta (vi64),
/// Track Namespace (..),
/// Number of Parameters (vi64),
/// Parameters (..) ...
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishNamespaceMessage {
    pub request_id: u64,
    /// ID delta from a dependent prior request. Always 0 in this minimal implementation.
    pub required_request_id_delta: u64,
    /// The namespace to register.
    pub track_namespace: TrackNamespace,
    // Parameters (Section 9.3): AUTHORIZATION_TOKEN (0x03) can appear here.
    // This implementation always encodes with count = 0; on decode,
    // parameter handling is delegated to decode_parameters.
}

impl PublishNamespaceMessage {
    pub fn encode(&self, buf: &mut Vec<u8>) -> Result<()> {
        let mut payload = Vec::new();
        encode_varint(self.request_id, &mut payload);
        encode_varint(self.required_request_id_delta, &mut payload);
        encode_track_namespace(&self.track_namespace, &mut payload)?;
        encode_parameters(&[], &mut payload)?;
        encode_message(MSG_PUBLISH_NAMESPACE, &payload, buf);
        Ok(())
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let (msg_type, payload) = decode_message(buf)?;
        ensure!(
            msg_type == MSG_PUBLISH_NAMESPACE,
            "expected PUBLISH_NAMESPACE (0x{MSG_PUBLISH_NAMESPACE:X}), got 0x{msg_type:X}"
        );
        let mut p = payload.as_slice();
        let request_id = decode_varint(&mut p)?;
        let required_request_id_delta = decode_varint(&mut p)?;
        let track_namespace = decode_track_namespace(&mut p)?;
        let _params = decode_parameters(&mut p)?;
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

    #[test]
    fn decode_with_skipped_parameters() {
        // A PUBLISH_NAMESPACE with an AUTHORIZATION_TOKEN parameter.
        // decode_parameters skips unknown-to-us params, so decode should succeed.
        use crate::wire::varint::encode_varint;
        let mut payload = Vec::new();
        encode_varint(0, &mut payload); // request_id
        encode_varint(0, &mut payload); // required_request_id_delta
        encode_varint(1, &mut payload); // namespace field count
        encode_varint(3, &mut payload); // field length
        payload.extend_from_slice(b"app"); // field value
        encode_varint(1, &mut payload); // num_params = 1
        encode_varint(0x03, &mut payload); // delta = AUTHORIZATION_TOKEN type
        encode_varint(4, &mut payload); // token length
        payload.extend_from_slice(b"test"); // token value
        let mut buf = Vec::new();
        encode_message(MSG_PUBLISH_NAMESPACE, &payload, &mut buf);
        let mut slice = buf.as_slice();
        let decoded = PublishNamespaceMessage::decode(&mut slice).unwrap();
        assert_eq!(decoded.track_namespace.fields, vec![b"app".to_vec()]);
    }

    #[test]
    fn wrong_message_type_is_error() {
        let mut buf = Vec::new();
        encode_message(0x03, &[], &mut buf); // SUBSCRIBE type, not PUBLISH_NAMESPACE
        let mut slice = buf.as_slice();
        assert!(PublishNamespaceMessage::decode(&mut slice).is_err());
    }
}
