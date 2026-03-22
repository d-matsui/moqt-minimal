// ObjectHeader (Section 10.4.3)
// Written before each object's payload in a data stream.
// Wire format: Object ID Delta (varint) + Payload Length (varint)

import { encodeVarint, decodeVarint } from "./varint.js";

export interface ObjectHeader {
  objectIdDelta: number;
  payloadLength: number;
}

/** Encode an ObjectHeader to bytes. */
export function encodeObjectHeader(h: ObjectHeader): Uint8Array {
  const buf: number[] = [];
  buf.push(...encodeVarint(h.objectIdDelta));
  buf.push(...encodeVarint(h.payloadLength));
  return new Uint8Array(buf);
}

/** Decode an ObjectHeader from a buffer. */
export function decodeObjectHeader(
  buf: Uint8Array,
  offset: number
): { header: ObjectHeader; bytesRead: number } {
  let pos = offset;
  const { value: objectIdDelta, bytesRead: r1 } = decodeVarint(buf, pos);
  pos += r1;
  const { value: payloadLength, bytesRead: r2 } = decodeVarint(buf, pos);
  pos += r2;
  return {
    header: { objectIdDelta, payloadLength },
    bytesRead: pos - offset,
  };
}
