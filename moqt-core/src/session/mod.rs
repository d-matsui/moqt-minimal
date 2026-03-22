//! # session: MOQT session management
//!
//! Protocol logic and high-level API for MOQT sessions.
//!
//! - `moqt_session`: MOQT session (SETUP exchange + event dispatch)
//! - `subgroup`: High-level Subgroup reader/writer (hides ObjectHeader)
//! - `subscribe_request`: Incoming SUBSCRIBE request handler
//! - `publish_namespace_request`: Incoming PUBLISH_NAMESPACE request handler
//! - `subscription`: Established subscription state
//! - `request_id`: Request ID allocation (client=even, server=odd)

pub mod moqt_session;
pub mod publish_namespace_request;
pub mod request_id;
pub mod subgroup;
pub mod subscribe_request;
pub mod subscription;
