//! # parameter: Message Parameters (Section 9.3)
//!
//! Control messages such as SUBSCRIBE and SUBSCRIBE_OK carry optional
//! parameters that modify request behavior. Each parameter has a
//! spec-defined type ID and a type-specific value encoding (uint8,
//! varint, Location, or length-prefixed). Parameters are serialized
//! in ascending order by type, using delta encoding for the type ID.
//!
//! The spec defines 10 parameter types. This module fully decodes
//! the 3 types used by this implementation into enum variants:
//! - SUBSCRIPTION_FILTER (0x21): subscription start position
//! - LARGEST_OBJECT (0x09): largest published location
//! - FORWARD (0x10): forwarding state (0 or 1)
//!
//! The remaining 7 spec-defined types are read and skipped based on
//! their wire encoding so the decoder can advance past them without
//! error. Parameters are not forwarded by relays (Section 9.3.1),
//! so discarding unused parameters is correct.
//!
//! An unknown type (not defined in the spec) causes a decode error,
//! per Section 9.3: "An endpoint that receives an unknown Message
//! Parameter MUST close the session with PROTOCOL_VIOLATION."
//!
//! Encode supports the 3 enum variants only. SUBSCRIPTION_FILTER
//! encode handles NextGroupStart only; other filter types are decoded
//! but not encoded because this implementation does not originate them.

use anyhow::{Result, bail, ensure};

use crate::wire::varint::{decode_varint, encode_varint};

// Parameter Type IDs (defined in the spec)
pub const PARAM_DELIVERY_TIMEOUT: u64 = 0x02;
pub const PARAM_AUTHORIZATION_TOKEN: u64 = 0x03;
pub const PARAM_RENDEZVOUS_TIMEOUT: u64 = 0x04;
pub const PARAM_EXPIRES: u64 = 0x08;
pub const PARAM_LARGEST_OBJECT: u64 = 0x09;
pub const PARAM_FORWARD: u64 = 0x10;
pub const PARAM_SUBSCRIBER_PRIORITY: u64 = 0x20;
pub const PARAM_SUBSCRIPTION_FILTER: u64 = 0x21;
pub const PARAM_GROUP_ORDER: u64 = 0x22;
pub const PARAM_NEW_GROUP_REQUEST: u64 = 0x32;

/// Message parameter carried in control messages (Section 9.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageParameter {
    /// SUBSCRIPTION_FILTER (0x21): Subscription filter.
    /// Specifies where the subscriber wants to start receiving data.
    SubscriptionFilter(SubscriptionFilter),
    /// LARGEST_OBJECT (0x09): Largest object location the publisher has.
    /// Expressed as a (Group ID, Object ID) pair.
    LargestObject { group: u64, object: u64 },
    /// FORWARD (0x10): Forwarding state. 0 = don't forward, 1 = forward.
    Forward(u8),
}

/// Subscription filter type (Section 5.1.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionFilter {
    /// 0x1: Start receiving from the beginning of the next group.
    /// The most common filter for live streaming.
    NextGroupStart,
    /// 0x2: Start from the object after the largest object the publisher has.
    LargestObject,
    /// 0x3: Start from an explicitly specified location (open-ended).
    AbsoluteStart { group: u64, object: u64 },
    /// 0x4: Start and end at explicitly specified locations.
    AbsoluteRange {
        group: u64,
        object: u64,
        end_group_delta: u64,
    },
}

/// Encode a list of message parameters.
/// Parameters must be in ascending Type order.
///
/// ```text
/// Number of Parameters (vi64),
/// Parameters (..) ...
///
/// Each parameter (Figure 4):
///   Type Delta (vi64),
///   Value (..)              // encoding varies per parameter type
/// ```
pub fn encode_parameters(params: &[MessageParameter], buf: &mut Vec<u8>) -> Result<()> {
    encode_varint(params.len() as u64, buf);
    let mut prev_type: u64 = 0;
    for param in params {
        let type_id = param_type_id(param);
        // Validate ascending order
        ensure!(
            type_id >= prev_type || prev_type == 0,
            "parameter type_id 0x{type_id:X} is not in ascending order (prev: 0x{prev_type:X})"
        );
        let delta = type_id - prev_type;
        encode_varint(delta, buf);
        match param {
            MessageParameter::SubscriptionFilter(filter) => {
                // SUBSCRIPTION_FILTER (Section 9.3.7) is length-prefixed:
                //   Length (vi64),
                //   Subscription Filter {
                //     Filter Type (vi64),
                //     [Start Location (Group vi64, Object vi64),]  // AbsoluteStart, AbsoluteRange
                //     [End Group Delta (vi64),]                    // AbsoluteRange only
                //   }
                let mut filter_payload = Vec::new();
                match filter {
                    // 0x1: Filter Type only
                    SubscriptionFilter::NextGroupStart => encode_varint(0x1, &mut filter_payload),
                    // Other filter types are decoded but not encoded in this
                    // minimal implementation (only NextGroupStart is used).
                    _ => bail!("only NextGroupStart filter is supported for encoding"),
                }
                encode_varint(filter_payload.len() as u64, buf);
                buf.extend_from_slice(&filter_payload);
            }
            MessageParameter::LargestObject { group, object } => {
                // LARGEST_OBJECT (Section 9.3.9): Location encoding
                //   Group (vi64),
                //   Object (vi64)
                encode_varint(*group, buf);
                encode_varint(*object, buf);
            }
            MessageParameter::Forward(v) => {
                // FORWARD (Section 9.3.10): uint8
                buf.push(*v);
            }
        }
        prev_type = type_id;
    }
    Ok(())
}

/// Decode a list of message parameters.
/// See encode_parameters for wire format details.
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
                // Length-prefixed: read filter payload and extract filter type
                let len = decode_varint(buf)? as usize;
                ensure!(buf.len() >= len, "subscription filter truncated");
                let mut filter_payload = &buf[..len];
                let filter_type = decode_varint(&mut filter_payload)?;
                *buf = &buf[len..];
                match filter_type {
                    0x1 => MessageParameter::SubscriptionFilter(SubscriptionFilter::NextGroupStart),
                    0x2 => MessageParameter::SubscriptionFilter(SubscriptionFilter::LargestObject),
                    0x3 => {
                        let group = decode_varint(&mut filter_payload)?;
                        let object = decode_varint(&mut filter_payload)?;
                        MessageParameter::SubscriptionFilter(SubscriptionFilter::AbsoluteStart {
                            group,
                            object,
                        })
                    }
                    0x4 => {
                        let group = decode_varint(&mut filter_payload)?;
                        let object = decode_varint(&mut filter_payload)?;
                        let end_group_delta = decode_varint(&mut filter_payload)?;
                        MessageParameter::SubscriptionFilter(SubscriptionFilter::AbsoluteRange {
                            group,
                            object,
                            end_group_delta,
                        })
                    }
                    _ => {
                        bail!("unknown filter type: 0x{filter_type:X}");
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
            // Spec-defined parameters not used by this implementation.
            // Read and skip based on their wire encoding so the decoder
            // can advance past them. These parameters are not forwarded
            // by relays (Section 9.3.1), so discarding is correct.
            PARAM_DELIVERY_TIMEOUT => {
                // varint (Section 9.3.3)
                let _ = decode_varint(buf)?;
                continue;
            }
            PARAM_AUTHORIZATION_TOKEN => {
                // Length-prefixed (Section 9.3.2)
                let len = decode_varint(buf)? as usize;
                ensure!(buf.len() >= len, "authorization token truncated");
                *buf = &buf[len..];
                continue;
            }
            PARAM_RENDEZVOUS_TIMEOUT => {
                // varint (Section 9.3.4)
                let _ = decode_varint(buf)?;
                continue;
            }
            PARAM_EXPIRES => {
                // varint (Section 9.3.8)
                let _ = decode_varint(buf)?;
                continue;
            }
            PARAM_SUBSCRIBER_PRIORITY => {
                // uint8 (Section 9.3.5)
                ensure!(!buf.is_empty(), "subscriber priority truncated");
                *buf = &buf[1..];
                continue;
            }
            PARAM_GROUP_ORDER => {
                // uint8 (Section 9.3.6)
                ensure!(!buf.is_empty(), "group order truncated");
                *buf = &buf[1..];
                continue;
            }
            PARAM_NEW_GROUP_REQUEST => {
                // varint (Section 9.3.11)
                let _ = decode_varint(buf)?;
                continue;
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

/// Get the Type ID for a parameter.
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
        encode_parameters(params, &mut buf).unwrap();
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

    // Decode-only tests for filter types not supported in encode.
    // Byte sequences are constructed manually to verify decode correctness.

    #[test]
    fn decode_filter_largest_object() {
        // 1 param, delta=0x21 (SUBSCRIPTION_FILTER), length=1, filter_type=0x2
        let mut buf = Vec::new();
        encode_varint(1, &mut buf); // count
        encode_varint(PARAM_SUBSCRIPTION_FILTER, &mut buf); // delta type
        encode_varint(1, &mut buf); // filter payload length (1 byte for filter type)
        encode_varint(0x2, &mut buf); // filter type = LargestObject
        let mut slice = buf.as_slice();
        let params = decode_parameters(&mut slice).unwrap();
        assert_eq!(
            params,
            vec![MessageParameter::SubscriptionFilter(
                SubscriptionFilter::LargestObject
            )]
        );
    }

    #[test]
    fn decode_filter_absolute_start() {
        // 1 param, delta=0x21, length=3 (filter_type + group + object), filter=0x3, group=5, object=0
        let mut buf = Vec::new();
        encode_varint(1, &mut buf); // count
        encode_varint(PARAM_SUBSCRIPTION_FILTER, &mut buf); // delta type
        let mut filter_payload = Vec::new();
        encode_varint(0x3, &mut filter_payload); // filter type = AbsoluteStart
        encode_varint(5, &mut filter_payload); // group
        encode_varint(0, &mut filter_payload); // object
        encode_varint(filter_payload.len() as u64, &mut buf); // filter payload length
        buf.extend_from_slice(&filter_payload);
        let mut slice = buf.as_slice();
        let params = decode_parameters(&mut slice).unwrap();
        assert_eq!(
            params,
            vec![MessageParameter::SubscriptionFilter(
                SubscriptionFilter::AbsoluteStart {
                    group: 5,
                    object: 0
                }
            )]
        );
    }

    #[test]
    fn decode_filter_absolute_range() {
        // 1 param, delta=0x21, filter=0x4, group=3, object=0, end_group_delta=10
        let mut buf = Vec::new();
        encode_varint(1, &mut buf); // count
        encode_varint(PARAM_SUBSCRIPTION_FILTER, &mut buf); // delta type
        let mut filter_payload = Vec::new();
        encode_varint(0x4, &mut filter_payload); // filter type = AbsoluteRange
        encode_varint(3, &mut filter_payload); // group
        encode_varint(0, &mut filter_payload); // object
        encode_varint(10, &mut filter_payload); // end_group_delta
        encode_varint(filter_payload.len() as u64, &mut buf); // filter payload length
        buf.extend_from_slice(&filter_payload);
        let mut slice = buf.as_slice();
        let params = decode_parameters(&mut slice).unwrap();
        assert_eq!(
            params,
            vec![MessageParameter::SubscriptionFilter(
                SubscriptionFilter::AbsoluteRange {
                    group: 3,
                    object: 0,
                    end_group_delta: 10,
                },
            )]
        );
    }

    #[test]
    fn unknown_filter_type_is_error() {
        let mut buf = Vec::new();
        encode_varint(1, &mut buf); // count = 1
        encode_varint(PARAM_SUBSCRIPTION_FILTER, &mut buf); // delta type
        let mut filter_payload = Vec::new();
        encode_varint(0xFF, &mut filter_payload); // unknown filter type
        encode_varint(filter_payload.len() as u64, &mut buf); // length
        buf.extend_from_slice(&filter_payload);
        let mut slice = buf.as_slice();
        assert!(decode_parameters(&mut slice).is_err());
    }

    #[test]
    fn encode_non_ascending_order_is_error() {
        let params = vec![
            MessageParameter::SubscriptionFilter(SubscriptionFilter::NextGroupStart),
            MessageParameter::Forward(1), // 0x10 < 0x21, not ascending
        ];
        let mut buf = Vec::new();
        assert!(encode_parameters(&params, &mut buf).is_err());
    }

    #[test]
    fn unknown_param_type_is_error() {
        let mut buf = Vec::new();
        encode_varint(1, &mut buf); // count = 1
        encode_varint(0x99, &mut buf); // unknown type
        encode_varint(0, &mut buf); // dummy value
        let mut slice = buf.as_slice();
        assert!(decode_parameters(&mut slice).is_err());
    }
}
