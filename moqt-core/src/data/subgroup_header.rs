use std::io;

use crate::wire::varint::{decode_varint, encode_varint};

/// Subgroup Header Type for minimal implementation: 0x38
/// - bit 0 (PROPERTIES): 0
/// - bit 1-2 (SUBGROUP_ID_MODE): 00 (Subgroup ID = 0, omitted)
/// - bit 3 (END_OF_GROUP): 1
/// - bit 4: 1 (fixed, identifies Subgroup Header)
/// - bit 5 (DEFAULT_PRIORITY): 1
const SUBGROUP_TYPE: u64 = 0x38;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubgroupHeader {
    pub track_alias: u64,
    pub group_id: u64,
}

impl SubgroupHeader {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        encode_varint(SUBGROUP_TYPE, buf);
        encode_varint(self.track_alias, buf);
        encode_varint(self.group_id, buf);
    }

    pub fn decode(buf: &mut &[u8]) -> io::Result<Self> {
        let stream_type = decode_varint(buf)?;
        if stream_type != SUBGROUP_TYPE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected SUBGROUP_HEADER type 0x{SUBGROUP_TYPE:X}, got 0x{stream_type:X}"),
            ));
        }
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
    fn wrong_type() {
        let mut buf = Vec::new();
        encode_varint(0x10, &mut buf); // wrong type
        encode_varint(1, &mut buf);
        encode_varint(0, &mut buf);
        let mut slice = buf.as_slice();
        assert!(SubgroupHeader::decode(&mut slice).is_err());
    }
}
