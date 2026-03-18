//! # data: MOQT data stream types (Section 10)
//!
//! Media data is sent over QUIC unidirectional streams (uni streams).
//! Each stream has the following structure:
//!
//! ```text
//! SubgroupHeader     <- once at the beginning of the stream
//! ObjectHeader + Payload  <- zero or more objects follow
//! ObjectHeader + Payload
//! ...
//! FIN                <- stream end
//! ```
//!
//! ## Relationship between Group, Subgroup, and Object
//! - Group: A unit of independently decodable media data (e.g. a video segment starting with a keyframe)
//! - Object: An individual data unit within a Group (e.g. a single video frame)
//! - Subgroup: A subdivision within a Group (1 Group = 1 Subgroup = 1 stream in this minimal implementation)

pub mod object;
pub mod subgroup_header;
