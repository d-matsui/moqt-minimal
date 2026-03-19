//! # session: MOQT session management
//!
//! Utilities for managing MOQT sessions over QUIC connections.
//!
//! - `moqt_session`: MOQT session (SETUP exchange + connection wrapper)
//! - `control_stream`: Control stream (unidirectional) — SETUP / GOAWAY
//! - `request_stream`: Request stream (bidirectional) — SUBSCRIBE, PUBLISH_NAMESPACE, etc.
//! - `data_stream`: Data stream (unidirectional) — SubgroupHeader + Objects
//! - `stream_utils`: Shared low-level readers (varint, message frame)
//! - `request_id`: Request ID allocation (client=even, server=odd)
//! - `quic_config`: QUIC/TLS configuration helpers

pub mod control_stream;
pub mod data_stream;
pub mod moqt_session;
pub mod quic_config;
pub mod request_id;
pub mod request_stream;
pub mod stream_utils;
