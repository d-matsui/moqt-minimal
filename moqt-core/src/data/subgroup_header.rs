//! # subgroup_header: Subgroup header (Section 10.2)
//!
//! Written at the beginning of a QUIC unidirectional stream.
//! Identifies which track (Track Alias) and group (Group ID) the data belongs to.
//!
//! ## Subgroup Header Type (0x38) bit fields
//! ```text
//! Bit 0   (PROPERTIES):       0 — no properties
//! Bit 1-2 (SUBGROUP_ID_MODE): 00 — Subgroup ID = 0 (omitted)
//! Bit 3   (END_OF_GROUP):     1 — this stream completes the group
//! Bit 4   (fixed):            1 — indicates this is a Subgroup Header
//! Bit 5   (DEFAULT_PRIORITY): 1 — use default priority
//! ```
//! → 0b00111000 = 0x38
//!
//! In this minimal implementation, 1 Group = 1 Subgroup = 1 QUIC stream.

use anyhow::{Result, ensure};

use crate::primitives::varint::{decode_varint, encode_varint};

/// Subgroup Header Type used in this minimal implementation.
/// See module doc for bit field meanings.
const SUBGROUP_TYPE: u64 = 0x38;

/// Subgroup header. Written at the beginning of a QUIC unidirectional stream.
///
/// ```text
/// Subgroup Header Type (vi64) = 0x38,
/// Track Alias (vi64),
/// Group ID (vi64)
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubgroupHeader {
    /// Track alias assigned in SUBSCRIBE_OK.
    /// Identifies the track without sending the full namespace each time.
    pub track_alias: u64,
    /// Group ID for this stream. Monotonically increasing.
    pub group_id: u64,
}

impl SubgroupHeader {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        encode_varint(SUBGROUP_TYPE, buf);
        encode_varint(self.track_alias, buf);
        encode_varint(self.group_id, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let stream_type = decode_varint(buf)?;
        ensure!(
            stream_type == SUBGROUP_TYPE,
            "expected SUBGROUP_HEADER type 0x{SUBGROUP_TYPE:X}, got 0x{stream_type:X}"
        );
        let track_alias = decode_varint(buf)?;
        let group_id = decode_varint(buf)?;
        Ok(SubgroupHeader {
            track_alias,
            group_id,
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
        });
    }

    #[test]
    fn large_ids() {
        roundtrip(&SubgroupHeader {
            track_alias: 42,
            group_id: 1000,
        });
    }

    #[test]
    fn type_on_wire() {
        let header = SubgroupHeader {
            track_alias: 0,
            group_id: 0,
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
