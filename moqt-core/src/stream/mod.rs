//! # stream: QUIC stream framing for MOQT
//!
//! Reader/writer wrappers for the three MOQT stream types over QUIC.
//! These handle framing (reading/writing bytes on a QUIC stream) but have
//! no knowledge of session state or protocol logic.
//!
//! - `control`: Control stream (unidirectional) — SETUP / GOAWAY
//! - `request`: Request stream (bidirectional) — SUBSCRIBE, PUBLISH_NAMESPACE, etc.
//! - `data`: Data stream (unidirectional) — SubgroupHeader + Objects
//! - `utils`: Shared low-level readers (varint, message frame)

pub mod control;
pub mod data;
pub mod request;
pub mod utils;
