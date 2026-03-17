//! # track_namespace: MOQT track namespace (Section 1.5)
//!
//! A track namespace is a hierarchical name structure used to identify
//! media streams published by a publisher. It consists of 0 to 32 fields,
//! each being a byte sequence of at least 1 byte.
//!
//! Example: `["example", "live"]` — a 2-field namespace. Combined with
//! a Track Name, it forms a Full Track Name that identifies a track.
//!
//! ## Wire format
//! ```text
//! Number of Fields (vi64),
//! [Field Length (vi64), Field Value (..)] ...
//! ```

use anyhow::{Result, ensure};

use super::varint::{decode_varint, encode_varint};

/// Track namespace: an ordered list of 0 to 32 byte-sequence fields.
/// Implements Hash so it can be used as a HashMap key.
/// This is used by the relay server to look up publishers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrackNamespace {
    pub fields: Vec<Vec<u8>>,
}

/// Maximum number of fields per the spec. Limits DoS attack surface.
const MAX_FIELDS: u64 = 32;

/// Encode a track namespace into bytes
///
/// ```text
/// Number of Fields (vi64),
/// [Field Length (vi64), Field Value (..)] ...
/// ```
///
/// An empty namespace (0 fields) is valid and encodes as a single byte `0x00`.
pub fn encode_track_namespace(ns: &TrackNamespace, buf: &mut Vec<u8>) -> Result<()> {
    ensure!(
        ns.fields.len() as u64 <= MAX_FIELDS,
        "too many namespace fields: {} (max {MAX_FIELDS})",
        ns.fields.len()
    );
    encode_varint(ns.fields.len() as u64, buf);
    for field in &ns.fields {
        ensure!(!field.is_empty(), "namespace field must not be empty");
        encode_varint(field.len() as u64, buf);
        buf.extend_from_slice(field);
    }
    Ok(())
}

/// Decode a track namespace from bytes
/// Returns an error if the field count exceeds the limit or a field length is 0.
pub fn decode_track_namespace(buf: &mut &[u8]) -> Result<TrackNamespace> {
    let num_fields = decode_varint(buf)?;
    ensure!(
        num_fields <= MAX_FIELDS,
        "too many namespace fields: {num_fields} (max {MAX_FIELDS})"
    );

    let mut fields = Vec::with_capacity(num_fields as usize);
    for _ in 0..num_fields {
        let field_len = decode_varint(buf)?;
        // Each field must be at least 1 byte per the spec
        ensure!(field_len > 0, "namespace field length must be at least 1");
        // Cast to usize for slice indexing
        let field_len = field_len as usize;
        ensure!(
            buf.len() >= field_len,
            "need {field_len} bytes for namespace field, have {}",
            buf.len()
        );
        fields.push(buf[..field_len].to_vec());
        *buf = &buf[field_len..];
    }

    Ok(TrackNamespace { fields })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(ns: &TrackNamespace) {
        let mut buf = Vec::new();
        encode_track_namespace(ns, &mut buf).unwrap();
        let mut slice = buf.as_slice();
        let decoded = decode_track_namespace(&mut slice).unwrap();
        assert_eq!(ns, &decoded);
        assert!(slice.is_empty(), "all bytes should be consumed");
    }

    // 0 fields
    #[test]
    fn empty_namespace() {
        let ns = TrackNamespace { fields: vec![] };
        roundtrip(&ns);

        let mut buf = Vec::new();
        encode_track_namespace(&ns, &mut buf).unwrap();
        // Number of Fields = 0 (1 byte varint)
        assert_eq!(buf, vec![0x00]);
    }

    // 1 field
    #[test]
    fn single_field() {
        let ns = TrackNamespace {
            fields: vec![b"example".to_vec()],
        };
        roundtrip(&ns);
    }

    // Multiple fields
    #[test]
    fn multiple_fields() {
        let ns = TrackNamespace {
            fields: vec![b"example".to_vec(), b"live".to_vec()],
        };
        roundtrip(&ns);
    }

    // Binary field value
    #[test]
    fn binary_field_value() {
        let ns = TrackNamespace {
            fields: vec![vec![0x00, 0xff, 0x42]],
        };
        roundtrip(&ns);
    }

    // 32 fields is OK
    #[test]
    fn encode_32_fields_ok() {
        let ns = TrackNamespace {
            fields: (0..32).map(|i| vec![b'a' + (i % 26) as u8]).collect(),
        };
        roundtrip(&ns);
    }

    // Encode: 33 fields is an error
    #[test]
    fn encode_too_many_fields_is_error() {
        let ns = TrackNamespace {
            fields: (0..33).map(|i| vec![b'a' + (i % 26) as u8]).collect(),
        };
        let mut buf = Vec::new();
        assert!(encode_track_namespace(&ns, &mut buf).is_err());
    }

    // Encode: empty field is an error
    #[test]
    fn encode_empty_field_is_error() {
        let ns = TrackNamespace {
            fields: vec![vec![]],
        };
        let mut buf = Vec::new();
        assert!(encode_track_namespace(&ns, &mut buf).is_err());
    }

    // Decode: field length 0 is an error
    #[test]
    fn decode_zero_length_field_is_error() {
        // Number of Fields = 1, Field Length = 0
        let data = vec![0x01, 0x00];
        let mut slice = data.as_slice();
        assert!(decode_track_namespace(&mut slice).is_err());
    }

    // Decode: 33 or more fields is an error
    #[test]
    fn decode_too_many_fields_is_error() {
        let mut buf = Vec::new();
        encode_varint(33, &mut buf);
        for _ in 0..33 {
            encode_varint(1, &mut buf); // field length = 1
            buf.push(b'x'); // field value
        }
        let mut slice = buf.as_slice();
        assert!(decode_track_namespace(&mut slice).is_err());
    }

    // Decode: truncated buffer
    #[test]
    fn decode_truncated_is_error() {
        // Number of Fields = 1, Field Length = 5 but only 2 bytes of data
        let data = vec![0x01, 0x05, 0x41, 0x42];
        let mut slice = data.as_slice();
        assert!(decode_track_namespace(&mut slice).is_err());
    }
}
