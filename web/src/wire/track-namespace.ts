// Track Namespace: a hierarchical identifier for media tracks.
// Wire format: field_count (varint) + (field_length (varint) + field_bytes)...

import { encodeVarint, decodeVarint } from "./varint.js";

export interface TrackNamespace {
  fields: Uint8Array[];
}

/** Create a TrackNamespace from string fields. */
export function trackNamespaceFrom(fields: string[]): TrackNamespace {
  return { fields: fields.map((f) => new TextEncoder().encode(f)) };
}

/** Encode a TrackNamespace to bytes. */
export function encodeTrackNamespace(ns: TrackNamespace, buf: number[]): void {
  buf.push(...encodeVarint(ns.fields.length));
  for (const field of ns.fields) {
    buf.push(...encodeVarint(field.length));
    buf.push(...field);
  }
}

/** Decode a TrackNamespace from a buffer. */
export function decodeTrackNamespace(
  buf: Uint8Array,
  offset: number
): { ns: TrackNamespace; bytesRead: number } {
  let pos = offset;
  const { value: count, bytesRead: countLen } = decodeVarint(buf, pos);
  pos += countLen;

  const fields: Uint8Array[] = [];
  for (let i = 0; i < count; i++) {
    const { value: fieldLen, bytesRead: fieldLenBytes } = decodeVarint(buf, pos);
    pos += fieldLenBytes;
    if (pos + fieldLen > buf.length) {
      throw new Error("track_namespace: truncated field");
    }
    fields.push(buf.slice(pos, pos + fieldLen));
    pos += fieldLen;
  }

  return { ns: { fields }, bytesRead: pos - offset };
}
