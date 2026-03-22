//! # data: MOQT data stream reader/writer
//!
//! Provides `DataStreamReader` and `DataStreamWriter` for reading and writing
//! SubgroupHeader + Object sequences on QUIC unidirectional streams.
//!
//! This mirrors the `ControlStreamReader` / `ControlStreamWriter` pattern
//! used for control messages, ensuring that wire format knowledge lives in
//! `moqt-core` (via `SubgroupHeader::decode()` / `ObjectHeader`) rather than
//! being duplicated in each binary.

use anyhow::Result;
use quinn::{RecvStream, SendStream};

use crate::data::object::ObjectHeader;
use crate::data::subgroup_header::SubgroupHeader;
use crate::stream::utils::read_varint;

/// Reads SubgroupHeader and Objects from a QUIC unidirectional data stream.
pub struct DataStreamReader {
    stream: RecvStream,
    has_properties: bool,
}

impl DataStreamReader {
    pub fn new(stream: RecvStream) -> Self {
        Self {
            stream,
            has_properties: false,
        }
    }

    /// Read and validate a SubgroupHeader from the stream.
    /// Returns (parsed header, raw bytes) so callers can forward raw bytes
    /// while also inspecting the parsed structure.
    ///
    /// Reading flow:
    /// 1. Read stream_type, track_alias, group_id varints
    /// 2. Inspect stream_type bits to read optional fields
    ///    (Subgroup ID if SUBGROUP_ID_MODE=10, Publisher Priority if DEFAULT_PRIORITY=0)
    /// 3. Pass accumulated raw bytes to `SubgroupHeader::decode()` for validation
    pub async fn read_subgroup_header(&mut self) -> Result<(SubgroupHeader, Vec<u8>)> {
        let mut raw = Vec::new();

        // stream_type varint
        let (stream_type, type_bytes) = read_varint(&mut self.stream).await?;
        raw.extend_from_slice(&type_bytes);

        // track_alias varint
        let (_track_alias, alias_bytes) = read_varint(&mut self.stream).await?;
        raw.extend_from_slice(&alias_bytes);

        // group_id varint
        let (_group_id, group_bytes) = read_varint(&mut self.stream).await?;
        raw.extend_from_slice(&group_bytes);

        // Optional Subgroup ID (SUBGROUP_ID_MODE bits 1-2 == 0b10)
        let subgroup_id_mode = (stream_type >> 1) & 0x03;
        if subgroup_id_mode == 0x02 {
            let (_subgroup_id, subgroup_bytes) = read_varint(&mut self.stream).await?;
            raw.extend_from_slice(&subgroup_bytes);
        }

        // Optional Publisher Priority (DEFAULT_PRIORITY bit 5 == 0)
        if stream_type & 0x20 == 0 {
            let mut priority = [0u8; 1];
            self.stream.read_exact(&mut priority).await?;
            raw.push(priority[0]);
        }

        // Validate with SubgroupHeader::decode()
        let mut slice = raw.as_slice();
        let header = SubgroupHeader::decode(&mut slice)?;

        self.has_properties = header.has_properties;

        Ok((header, raw))
    }

    /// Read one Object from the stream.
    /// Returns `Ok(None)` on stream FIN (no more objects).
    /// Returns `Ok(Some((header, payload, raw_header_bytes)))` on success.
    /// The raw_header_bytes contain the encoded ObjectHeader (and properties if present),
    /// suitable for pass-through forwarding.
    pub async fn read_object(&mut self) -> Result<Option<(ObjectHeader, Vec<u8>, Vec<u8>)>> {
        let mut header_bytes = Vec::new();

        // Read object_id_delta; EOF means end of stream
        let (object_id_delta, delta_bytes) = match read_varint(&mut self.stream).await {
            Ok(v) => v,
            Err(e) => {
                let is_eof = e.downcast_ref::<quinn::ReadExactError>().is_some()
                    || e.to_string().contains("stream ended");
                if is_eof {
                    return Ok(None);
                }
                return Err(e);
            }
        };
        header_bytes.extend_from_slice(&delta_bytes);

        // Read payload_length
        let (payload_length, len_bytes) = read_varint(&mut self.stream).await?;
        header_bytes.extend_from_slice(&len_bytes);

        // Skip Object Properties if PROPERTIES bit was set in SubgroupHeader
        if self.has_properties {
            let (props_len, props_len_bytes) = read_varint(&mut self.stream).await?;
            header_bytes.extend_from_slice(&props_len_bytes);
            if props_len > 0 {
                let mut props = vec![0u8; props_len as usize];
                self.stream.read_exact(&mut props).await?;
                header_bytes.extend_from_slice(&props);
            }
        }

        // Read payload
        let mut payload = vec![0u8; payload_length as usize];
        if payload_length > 0 {
            self.stream.read_exact(&mut payload).await?;
        }

        let header = ObjectHeader {
            object_id_delta,
            payload_length,
        };

        Ok(Some((header, payload, header_bytes)))
    }
}

/// Writes SubgroupHeader and Objects to a QUIC unidirectional data stream.
pub struct DataStreamWriter {
    stream: SendStream,
}

impl DataStreamWriter {
    pub fn new(stream: SendStream) -> Self {
        Self { stream }
    }

    /// Write a SubgroupHeader to the stream.
    pub async fn write_subgroup_header(&mut self, header: &SubgroupHeader) -> Result<()> {
        let mut buf = Vec::new();
        header.encode(&mut buf);
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    /// Write an Object (header + payload) to the stream.
    pub async fn write_object(&mut self, header: &ObjectHeader, payload: &[u8]) -> Result<()> {
        let mut buf = Vec::new();
        header.encode(&mut buf);
        buf.extend_from_slice(payload);
        self.stream.write_all(&buf).await?;
        Ok(())
    }

    /// Write raw bytes to the stream (for pass-through forwarding).
    pub async fn write_raw(&mut self, data: &[u8]) -> Result<()> {
        self.stream.write_all(data).await?;
        Ok(())
    }

    /// Signal the end of the stream (send FIN).
    pub fn finish(&mut self) -> Result<()> {
        self.stream.finish()?;
        Ok(())
    }
}
