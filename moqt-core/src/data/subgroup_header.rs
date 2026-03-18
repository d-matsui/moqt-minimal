//! # subgroup_header: Subgroup header (Section 10.2)
//!
//! Written at the beginning of a QUIC unidirectional stream.
//! Identifies which track (Track Alias) and group (Group ID) the data belongs to.
//!
//! ## Subgroup Header Type (0x38) bit fields
//! Bits are numbered from the right (bit 0 = rightmost):
//!
//! ```text
//! Bit 0   PROPERTIES:        0 = no properties, 1 = all objects have properties
//! Bit 1-2 SUBGROUP_ID_MODE:  00 = ID is 0, 01 = ID is first object ID,
//!                            10 = ID in header, 11 = reserved (invalid)
//! Bit 3   END_OF_GROUP:      0 = unknown, 1 = this stream completes the group
//! Bit 4   (fixed):           always 1 (identifies Subgroup Header)
//! Bit 5   DEFAULT_PRIORITY:  0 = priority in header, 1 = use default priority
//! ```
//!
//! This implementation uses 0b00111000 = 0x38:
//! PROPERTIES=0, SUBGROUP_ID_MODE=00, END_OF_GROUP=1, fixed=1, DEFAULT_PRIORITY=1
//!
//! In this minimal implementation, 1 Group = 1 Subgroup = 1 QUIC stream.

use anyhow::{Result, ensure};

use crate::primitives::varint::{decode_varint, encode_varint};

/// Subgroup Header Type used for encoding in this minimal implementation.
/// See module doc for bit field meanings.
const SUBGROUP_TYPE: u64 = 0x38;

/// Subgroup header. Written at the beginning of a QUIC unidirectional stream.
///
/// ```text
/// Subgroup Header Type (vi64),
/// Track Alias (vi64),
/// Group ID (vi64),
/// [Subgroup ID (vi64),]      // if SUBGROUP_ID_MODE = 10
/// [Publisher Priority (u8),] // if DEFAULT_PRIORITY = 0
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubgroupHeader {
    /// Track alias assigned in SUBSCRIBE_OK.
    /// Identifies the track without sending the full namespace each time.
    pub track_alias: u64,
    /// Group ID for this stream. Monotonically increasing.
    pub group_id: u64,
    /// Whether each Object in this subgroup has Properties.
    /// Determined by bit 0 of the header type.
    pub has_properties: bool,
}

impl SubgroupHeader {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        encode_varint(SUBGROUP_TYPE, buf);
        encode_varint(self.track_alias, buf);
        encode_varint(self.group_id, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let stream_type = decode_varint(buf)?;
        // Validate: bit 4 must be 1 (Subgroup Header), and must match
        // the pattern 0b00X1XXXX (0x10..0x1F or 0x30..0x3F)
        ensure!(
            stream_type & 0x10 == 0x10 && (stream_type & !0x3F) == 0,
            "invalid SUBGROUP_HEADER type 0x{stream_type:X}"
        );
        // SUBGROUP_ID_MODE = 11 is reserved
        let subgroup_id_mode = (stream_type >> 1) & 0x03;
        ensure!(
            subgroup_id_mode != 0x03,
            "reserved SUBGROUP_ID_MODE 0b11 in type 0x{stream_type:X}"
        );

        let has_properties = stream_type & 0x01 != 0;

        let track_alias = decode_varint(buf)?;
        let group_id = decode_varint(buf)?;

        // Skip Subgroup ID if SUBGROUP_ID_MODE = 10 (explicit)
        if subgroup_id_mode == 0x02 {
            let _ = decode_varint(buf)?;
        }

        // Skip Publisher Priority if DEFAULT_PRIORITY = 0
        if stream_type & 0x20 == 0 {
            ensure!(!buf.is_empty(), "publisher priority truncated");
            *buf = &buf[1..];
        }

        Ok(SubgroupHeader {
            track_alias,
            group_id,
            has_properties,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(header: &SubgroupHeader) {
        let mut buf = Vec::new();
        header.encode(&mut buf);
        let mut slice = buf.as_slice();
        let decoded = SubgroupHeader::decode(&mut slice).unwrap();
        assert_eq!(header, &decoded);
        assert!(slice.is_empty());
    }

    #[test]
    fn basic() {
        roundtrip(&SubgroupHeader {
            track_alias: 1,
            group_id: 0,
            has_properties: false,
        });
    }

    #[test]
    fn large_ids() {
        roundtrip(&SubgroupHeader {
            track_alias: 42,
            group_id: 1000,
            has_properties: false,
        });
    }

    #[test]
    fn type_on_wire() {
        let header = SubgroupHeader {
            track_alias: 0,
            group_id: 0,
            has_properties: false,
        };
        let mut buf = Vec::new();
        header.encode(&mut buf);
        // 0x38 fits in 1-byte varint
        assert_eq!(buf[0], 0x38);
    }

    #[test]
    fn wrong_type_is_error() {
        let mut buf = Vec::new();
        encode_varint(0x10, &mut buf); // wrong type
        encode_varint(1, &mut buf);
        encode_varint(0, &mut buf);
        let mut slice = buf.as_slice();
        assert!(SubgroupHeader::decode(&mut slice).is_err());
    }
}
