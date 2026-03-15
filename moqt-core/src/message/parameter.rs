use std::io;

use crate::wire::varint::{decode_varint, encode_varint};

// Parameter Type IDs
pub const PARAM_DELIVERY_TIMEOUT: u64 = 0x02;
pub const PARAM_AUTHORIZATION_TOKEN: u64 = 0x03;
pub const PARAM_EXPIRES: u64 = 0x08;
pub const PARAM_LARGEST_OBJECT: u64 = 0x09;
pub const PARAM_FORWARD: u64 = 0x10;
pub const PARAM_SUBSCRIBER_PRIORITY: u64 = 0x20;
pub const PARAM_SUBSCRIPTION_FILTER: u64 = 0x21;
pub const PARAM_GROUP_ORDER: u64 = 0x22;

/// Known message parameters for the minimal implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageParameter {
    /// SUBSCRIPTION_FILTER (0x21): length-prefixed, contains Filter Type.
    SubscriptionFilter(SubscriptionFilter),
    /// LARGEST_OBJECT (0x9): Location (Group vi64, Object vi64).
    LargestObject { group: u64, object: u64 },
    /// FORWARD (0x10): uint8, 0 or 1.
    Forward(u8),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionFilter {
    NextGroupStart, // 0x1
}

/// Encode message parameters. Parameters must be in ascending Type order.
pub fn encode_parameters(params: &[MessageParameter], buf: &mut Vec<u8>) {
    encode_varint(params.len() as u64, buf);
    let mut prev_type: u64 = 0;
    for param in params {
        let type_id = param_type_id(param);
        let delta = type_id - prev_type;
        encode_varint(delta, buf);
        match param {
            MessageParameter::SubscriptionFilter(filter) => {
                // length-prefixed
                let mut inner = Vec::new();
                match filter {
                    SubscriptionFilter::NextGroupStart => encode_varint(0x1, &mut inner),
                }
                encode_varint(inner.len() as u64, buf);
                buf.extend_from_slice(&inner);
            }
            MessageParameter::LargestObject { group, object } => {
                // Location: two consecutive varints
                encode_varint(*group, buf);
                encode_varint(*object, buf);
            }
            MessageParameter::Forward(v) => {
                // uint8
                buf.push(*v);
            }
        }
        prev_type = type_id;
    }
}

/// Decode message parameters. Returns the count prefix + parsed parameters.
pub fn decode_parameters(buf: &mut &[u8]) -> io::Result<Vec<MessageParameter>> {
    let count = decode_varint(buf)?;
    let mut params = Vec::with_capacity(count as usize);
    let mut prev_type: u64 = 0;
    for _ in 0..count {
        let delta = decode_varint(buf)?;
        let type_id = prev_type.checked_add(delta).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "parameter type overflow")
        })?;
        let param = match type_id {
            PARAM_SUBSCRIPTION_FILTER => {
                let len = decode_varint(buf)? as usize;
                if buf.len() < len {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "subscription filter truncated",
                    ));
                }
                let mut inner = &buf[..len];
                let filter_type = decode_varint(&mut inner)?;
                *buf = &buf[len..];
                match filter_type {
                    0x1 => MessageParameter::SubscriptionFilter(
                        SubscriptionFilter::NextGroupStart,
                    ),
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("unsupported filter type: 0x{filter_type:X}"),
                        ));
                    }
                }
            }
            PARAM_LARGEST_OBJECT => {
                let group = decode_varint(buf)?;
                let object = decode_varint(buf)?;
                MessageParameter::LargestObject { group, object }
            }
            PARAM_FORWARD => {
                if buf.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "forward parameter truncated",
                    ));
                }
                let v = buf[0];
                *buf = &buf[1..];
                MessageParameter::Forward(v)
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown parameter type: 0x{type_id:X}"),
                ));
            }
        };
        params.push(param);
        prev_type = type_id;
    }
    Ok(params)
}

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
