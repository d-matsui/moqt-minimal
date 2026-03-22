//! # subgroup_header: Subgroup header (Section 10.4.2)
//!
//! Written at the beginning of a QUIC unidirectional stream.
//! Identifies which track (Track Alias) and group (Group ID) the data belongs to.
//!
//! ## draft-17 spec: Subgroup Header Type bit fields
//!
//! The Type field takes the form 0b00X1XXXX (Section 10.4.2).
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
//! Valid Type values (Section 10.4.2):
//!   0x10..0x15 / 0x18..0x1D / 0x30..0x35 / 0x38..0x3D
//! (0b00X1XXXX from which SUBGROUP_ID_MODE = 0b11 (bits 1-2) is excluded as reserved)
//!
//! Optional fields present depending on bits:
//! - Subgroup ID (vi64): present when SUBGROUP_ID_MODE = 0b10
//! - Publisher Priority (u8): present when DEFAULT_PRIORITY = 0
//!
//! ## Encode / Decode
//!
//! The Type byte is dynamically built from the struct fields.
//! All valid bit patterns are supported for both encode and decode.
//! Optional fields (Subgroup ID, Publisher Priority) are included
//! on the wire only when the corresponding struct field is Some.

use anyhow::{Result, ensure};

use crate::wire::varint::{decode_varint, encode_varint};

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
    /// Whether each Object in this subgroup has Properties (bit 0).
    pub has_properties: bool,
    /// Whether this subgroup contains the last Object in the Group (bit 3).
    pub end_of_group: bool,
    /// Explicit Subgroup ID. None means Subgroup ID = 0 (SUBGROUP_ID_MODE=00).
    pub subgroup_id: Option<u64>,
    /// Publisher Priority. None means use default (DEFAULT_PRIORITY=1).
    pub publisher_priority: Option<u8>,
}

impl SubgroupHeader {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        let mut type_byte: u64 = 0x10; // bit 4 always set
        if self.has_properties {
            type_byte |= 0x01; // bit 0
        }
        if self.subgroup_id.is_some() {
            type_byte |= 0x04; // SUBGROUP_ID_MODE = 10 (bit 1-2)
        }
        if self.end_of_group {
            type_byte |= 0x08; // bit 3
        }
        if self.publisher_priority.is_none() {
            type_byte |= 0x20; // DEFAULT_PRIORITY = 1 (bit 5)
        }
        encode_varint(type_byte, buf);
        encode_varint(self.track_alias, buf);
        encode_varint(self.group_id, buf);
        if let Some(id) = self.subgroup_id {
            encode_varint(id, buf);
        }
        if let Some(priority) = self.publisher_priority {
            buf.push(priority);
        }
    }

    /// Decode a SubgroupHeader, accepting all valid Type values from draft-17.
    /// Reads optional fields (Subgroup ID, Publisher Priority) to correctly
    /// advance the buffer, but does not store them in the struct.
    pub fn decode(buf: &mut &[u8]) -> Result<Self> {
        let stream_type = decode_varint(buf)?;
        // Valid Type values per Section 10.4.2
        ensure!(
            matches!(
                stream_type,
                0x10..=0x15 | 0x18..=0x1D | 0x30..=0x35 | 0x38..=0x3D
            ),
            "invalid SUBGROUP_HEADER type 0x{stream_type:X}"
        );

        let has_properties = stream_type & 0x01 != 0;
        let subgroup_id_mode = (stream_type >> 1) & 0x03;
        let end_of_group = stream_type & 0x08 != 0;

        let track_alias = decode_varint(buf)?;
        let group_id = decode_varint(buf)?;

        // Subgroup ID present when SUBGROUP_ID_MODE = 10
        let subgroup_id = if subgroup_id_mode == 0x02 {
            Some(decode_varint(buf)?)
        } else {
            None
        };

        // Publisher Priority present when DEFAULT_PRIORITY = 0
        let publisher_priority = if stream_type & 0x20 == 0 {
            ensure!(!buf.is_empty(), "publisher priority truncated");
            let v = buf[0];
            *buf = &buf[1..];
            Some(v)
        } else {
            None
        };

        Ok(SubgroupHeader {
            track_alias,
            group_id,
            has_properties,
            end_of_group,
            subgroup_id,
            publisher_priority,
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
            end_of_group: true,
            subgroup_id: None,
            publisher_priority: None,
        });
    }

    #[test]
    fn large_ids() {
        roundtrip(&SubgroupHeader {
            track_alias: 42,
            group_id: 1000,
            has_properties: false,
            end_of_group: true,
            subgroup_id: None,
            publisher_priority: None,
        });
    }

    #[test]
    fn with_publisher_priority() {
        // DEFAULT_PRIORITY=0 → priority in header
        roundtrip(&SubgroupHeader {
            track_alias: 1,
            group_id: 0,
            has_properties: false,
            end_of_group: true,
            subgroup_id: None,
            publisher_priority: Some(64),
        });
    }

    #[test]
    fn with_explicit_subgroup_id() {
        // SUBGROUP_ID_MODE=10 → subgroup ID in header
        roundtrip(&SubgroupHeader {
            track_alias: 1,
            group_id: 0,
            has_properties: false,
            end_of_group: true,
            subgroup_id: Some(5),
            publisher_priority: None,
        });
    }

    #[test]
    fn with_all_optional_fields() {
        roundtrip(&SubgroupHeader {
            track_alias: 3,
            group_id: 10,
            has_properties: true,
            end_of_group: false,
            subgroup_id: Some(2),
            publisher_priority: Some(128),
        });
    }

    #[test]
    fn type_byte_default_priority() {
        // end_of_group=1, no subgroup_id, no priority → 0x38
        let header = SubgroupHeader {
            track_alias: 0,
            group_id: 0,
            has_properties: false,
            end_of_group: true,
            subgroup_id: None,
            publisher_priority: None,
        };
        let mut buf = Vec::new();
        header.encode(&mut buf);
        assert_eq!(buf[0], 0x38);
    }

    #[test]
    fn type_byte_with_priority() {
        // end_of_group=1, no subgroup_id, priority present → 0x18
        let header = SubgroupHeader {
            track_alias: 0,
            group_id: 0,
            has_properties: false,
            end_of_group: true,
            subgroup_id: None,
            publisher_priority: Some(0),
        };
        let mut buf = Vec::new();
        header.encode(&mut buf);
        assert_eq!(buf[0], 0x18);
    }

    #[test]
    fn type_byte_with_subgroup_id() {
        // end_of_group=1, subgroup_id present, default priority → 0x3C
        let header = SubgroupHeader {
            track_alias: 0,
            group_id: 0,
            has_properties: false,
            end_of_group: true,
            subgroup_id: Some(0),
            publisher_priority: None,
        };
        let mut buf = Vec::new();
        header.encode(&mut buf);
        assert_eq!(buf[0], 0x3C);
    }

    // 0x10 has DEFAULT_PRIORITY=0, so a Publisher Priority byte is required.
    // This test provides no priority byte, so decode fails with truncation.
    #[test]
    fn truncated_priority_is_error() {
        let mut buf = Vec::new();
        encode_varint(0x10, &mut buf);
        encode_varint(1, &mut buf);
        encode_varint(0, &mut buf);
        let mut slice = buf.as_slice();
        assert!(SubgroupHeader::decode(&mut slice).is_err());
    }
}
