use std::io;

use crate::wire::varint::{decode_varint, encode_varint};

/// Object fields within a Subgroup stream.
///
/// Object ID = previous Object ID + delta + 1 (for non-first objects).
/// For the first object in a subgroup: Object ID = delta.
/// Consecutive IDs (0, 1, 2, ...) have delta = 0 for all objects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectHeader {
    pub object_id_delta: u64,
    pub payload_length: u64,
}

impl ObjectHeader {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        encode_varint(self.object_id_delta, buf);
        encode_varint(self.payload_length, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> io::Result<Self> {
        let object_id_delta = decode_varint(buf)?;
        let payload_length = decode_varint(buf)?;
        Ok(ObjectHeader {
            object_id_delta,
            payload_length,
        })
    }
}

/// Compute the absolute Object ID from the delta.
/// - first object in subgroup: `object_id = delta`
/// - subsequent objects: `object_id = prev_object_id + delta + 1`
pub fn resolve_object_id(prev_object_id: Option<u64>, delta: u64) -> u64 {
    match prev_object_id {
        None => delta,
        Some(prev) => prev + delta + 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(header: &ObjectHeader) {
        let mut buf = Vec::new();
        header.encode(&mut buf);
        let mut slice = buf.as_slice();
        let decoded = ObjectHeader::decode(&mut slice).unwrap();
        assert_eq!(header, &decoded);
        assert!(slice.is_empty());
    }

    #[test]
    fn basic() {
        roundtrip(&ObjectHeader {
            object_id_delta: 0,
            payload_length: 100,
        });
    }

    #[test]
    fn large_payload() {
        roundtrip(&ObjectHeader {
            object_id_delta: 0,
            payload_length: 1_000_000,
        });
    }

    #[test]
    fn nonzero_delta() {
        roundtrip(&ObjectHeader {
            object_id_delta: 5,
            payload_length: 50,
        });
    }

    // Object ID resolution: consecutive IDs (0,1,2,...) with delta=0
    #[test]
    fn resolve_consecutive_ids() {
        assert_eq!(resolve_object_id(None, 0), 0);
        assert_eq!(resolve_object_id(Some(0), 0), 1);
        assert_eq!(resolve_object_id(Some(1), 0), 2);
        assert_eq!(resolve_object_id(Some(2), 0), 3);
    }

    // Object ID resolution: first object with nonzero delta
    #[test]
    fn resolve_first_object_nonzero_delta() {
        assert_eq!(resolve_object_id(None, 5), 5);
    }

    // Object ID resolution: gap (delta > 0 for non-first)
    #[test]
    fn resolve_with_gap() {
        // prev=2, delta=2 → 2 + 2 + 1 = 5 (objects 3 and 4 are in different subgroups or don't exist)
        assert_eq!(resolve_object_id(Some(2), 2), 5);
    }

    // Multiple objects on a stream: encode/decode sequence
    #[test]
    fn multiple_objects_on_stream() {
        let objects = vec![
            (
                ObjectHeader {
                    object_id_delta: 0,
                    payload_length: 3,
                },
                b"abc".to_vec(),
            ),
            (
                ObjectHeader {
                    object_id_delta: 0,
                    payload_length: 3,
                },
                b"def".to_vec(),
            ),
            (
                ObjectHeader {
                    object_id_delta: 0,
                    payload_length: 3,
                },
                b"ghi".to_vec(),
            ),
        ];

        let mut buf = Vec::new();
        for (header, payload) in &objects {
            header.encode(&mut buf);
            buf.extend_from_slice(payload);
        }

        let mut slice = buf.as_slice();
        let mut prev_id: Option<u64> = None;
        for (i, (_, expected_payload)) in objects.iter().enumerate() {
            let header = ObjectHeader::decode(&mut slice).unwrap();
            let id = resolve_object_id(prev_id, header.object_id_delta);
            assert_eq!(id, i as u64);

            let payload = &slice[..header.payload_length as usize];
            slice = &slice[header.payload_length as usize..];
            assert_eq!(payload, expected_payload.as_slice());

            prev_id = Some(id);
        }
        assert!(slice.is_empty());
    }
}
