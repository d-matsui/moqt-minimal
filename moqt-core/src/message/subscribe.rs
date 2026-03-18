//! # subscribe: SUBSCRIBE message (Section 9.8)
//!
//! Sent by a subscriber (via relay) to request subscription to a specific track.
//!
//! ## Request ID
//! - Client-originated requests use even IDs (0, 2, 4, ...)
//! - Server-originated requests use odd IDs (1, 3, 5, ...)
//! - Required Request ID Delta expresses dependencies (always 0 in this minimal implementation)

use anyhow::{Result, ensure};

use super::parameter::{MessageParameter, decode_parameters, encode_parameters};
use super::{MSG_SUBSCRIBE, decode_message, encode_message};
use crate::primitives::track_namespace::{
    TrackNamespace, decode_track_namespace, encode_track_namespace,
};
use crate::primitives::varint::{decode_varint, encode_varint};

/// SUBSCRIBE message. Requests subscription to a specific track.
///
/// ```text
/// Type (vi64) = 0x3,
/// Length (u16),
/// Request ID (vi64),
/// Required Request ID Delta (vi64),
/// Track Namespace (..),
/// Track Name Length (vi64),
/// Track Name (..),
/// Number of Parameters (vi64),
/// Parameters (..) ...
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscribeMessage {
    /// Unique ID for this request. Used to correlate with response messages.
    pub request_id: u64,
    /// ID delta from a dependent prior request. Always 0 in this minimal implementation.
    pub required_request_id_delta: u64,
    /// Track namespace to subscribe to.
    pub track_namespace: TrackNamespace,
    /// Track name. Combined with namespace, uniquely identifies a track.
    pub track_name: Vec<u8>,
    /// Subscription parameters (filter, priority, etc.).
    pub parameters: Vec<MessageParameter>,
}

impl SubscribeMessage {
    pub fn encode(&self, buf: &mut Vec<u8>) -> Result<()> {
        let mut payload = Vec::new();
        encode_varint(self.request_id, &mut payload);
        encode_varint(self.required_request_id_delta, &mut payload);
        encode_track_namespace(&self.track_namespace, &mut payload)?;
        // Track Name is a length-prefixed byte sequence
        encode_varint(self.track_name.len() as u64, &mut payload);
        payload.extend_from_slice(&self.track_name);
        encode_parameters(&self.parameters, &mut payload)?;
        encode_message(MSG_SUBSCRIBE, &payload, buf);
        Ok(())
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let (msg_type, payload) = decode_message(buf)?;
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
        msg.encode(&mut buf).unwrap();
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

    #[test]
    fn wrong_message_type_is_error() {
        let mut buf = Vec::new();
        encode_message(0x07, &[], &mut buf); // REQUEST_OK type, not SUBSCRIBE
        let mut slice = buf.as_slice();
        assert!(SubscribeMessage::decode(&mut slice).is_err());
    }
}
