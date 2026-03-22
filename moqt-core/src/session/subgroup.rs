//! # subgroup: High-level Subgroup reader/writer
//!
//! Wraps `DataStreamWriter` / `DataStreamReader` to hide wire-level details
//! (`ObjectHeader`, `SubgroupHeader` fields) from application code.
//!
//! - `SubgroupWriter`: send objects by payload only (ObjectHeader auto-generated)
//! - `SubgroupReader`: receive objects as payloads, with subgroup metadata accessors
//!
//! For low-level access (e.g. relay pass-through), `SubgroupReader` also exposes
//! `header()` and `read_object_raw()`.

use anyhow::Result;

use crate::stream::data::{DataStreamReader, DataStreamWriter};
use crate::wire::object::ObjectHeader;
use crate::wire::subgroup_header::SubgroupHeader;

/// Writes objects to a subgroup (unidirectional data stream).
///
/// Hides `ObjectHeader` construction — callers pass only the payload.
/// The stream is automatically finished (FIN sent) when dropped.
pub struct SubgroupWriter {
    writer: DataStreamWriter,
    finished: bool,
}

impl SubgroupWriter {
    pub(crate) fn new(writer: DataStreamWriter) -> Self {
        Self {
            writer,
            finished: false,
        }
    }

    /// Write an object payload to this subgroup.
    /// `ObjectHeader` is generated internally (object_id_delta=0, payload_length from payload).
    pub async fn write_object(&mut self, payload: &[u8]) -> Result<()> {
        let header = ObjectHeader {
            object_id_delta: 0,
            payload_length: payload.len() as u64,
        };
        self.writer.write_object(&header, payload).await
    }

    /// Explicitly finish the stream (send FIN).
    /// If not called, the stream is finished on drop.
    pub fn finish(&mut self) -> Result<()> {
        if !self.finished {
            self.finished = true;
            self.writer.finish()?;
        }
        Ok(())
    }
}

impl Drop for SubgroupWriter {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.writer.finish();
        }
    }
}

/// Reads objects from a subgroup (unidirectional data stream).
///
/// Provides high-level access (payload only) for clients,
/// and low-level access (raw bytes) for relay pass-through.
pub struct SubgroupReader {
    header: SubgroupHeader,
    reader: DataStreamReader,
}

impl SubgroupReader {
    pub(crate) fn new(header: SubgroupHeader, reader: DataStreamReader) -> Self {
        Self { header, reader }
    }

    /// The track alias from the SubgroupHeader.
    pub fn track_alias(&self) -> u64 {
        self.header.track_alias
    }

    /// The group ID from the SubgroupHeader.
    pub fn group_id(&self) -> u64 {
        self.header.group_id
    }

    /// Read the next object payload.
    /// Returns `Ok(None)` when the stream ends (no more objects).
    pub async fn read_object(&mut self) -> Result<Option<Vec<u8>>> {
        match self.reader.read_object().await? {
            Some((_, payload, _)) => Ok(Some(payload)),
            None => Ok(None),
        }
    }

    /// Get the full SubgroupHeader (for relay pass-through).
    pub fn header(&self) -> &SubgroupHeader {
        &self.header
    }

    /// Read the next object with raw bytes (for relay pass-through).
    /// Returns `(ObjectHeader, payload, raw_header_bytes)`.
    pub async fn read_object_raw(&mut self) -> Result<Option<(ObjectHeader, Vec<u8>, Vec<u8>)>> {
        self.reader.read_object().await
    }
}
