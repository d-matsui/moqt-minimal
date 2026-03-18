//! # primitives: MOQT protocol primitives
//!
//! Defines the basic data types exchanged as byte sequences in the MOQT protocol.
//! These are used as building blocks by higher-level message types.
//!
//! - `varint`: Variable-length integer encoding/decoding (used throughout the protocol)
//! - `key_value_pair`: Key-value pairs used in SETUP messages, etc.
//! - `reason_phrase`: Length-prefixed UTF-8 string for error reasons, etc.
//! - `track_namespace`: Track namespace (identifies media published by a publisher)

pub mod key_value_pair;
pub mod reason_phrase;
pub mod track_namespace;
pub mod varint;
