// SubgroupHeader (Section 10.4.2)
// Written at the beginning of a QUIC unidirectional data stream.
//
// Type byte bit fields:
//   Bit 0:   PROPERTIES (0=no, 1=yes)
//   Bit 1-2: SUBGROUP_ID_MODE (00=0, 01=first_obj, 10=in_header, 11=reserved)
//   Bit 3:   END_OF_GROUP (0=unknown, 1=completes group)
//   Bit 4:   always 1
//   Bit 5:   DEFAULT_PRIORITY (0=in header, 1=use default)

import { encodeVarint, decodeVarint } from "./varint.js";

export interface SubgroupHeader {
  trackAlias: number;
  groupId: number;
  hasProperties: boolean;
  endOfGroup: boolean;
  subgroupId: number | null;
  publisherPriority: number | null;
}

/** Encode a SubgroupHeader to bytes. */
export function encodeSubgroupHeader(h: SubgroupHeader): Uint8Array {
  let typeByte = 0x10; // bit 4 always set
  if (h.hasProperties) typeByte |= 0x01;
  if (h.subgroupId !== null) typeByte |= 0x04; // SUBGROUP_ID_MODE = 10
  if (h.endOfGroup) typeByte |= 0x08;
  if (h.publisherPriority === null) typeByte |= 0x20; // DEFAULT_PRIORITY = 1

  const buf: number[] = [];
  buf.push(...encodeVarint(typeByte));
  buf.push(...encodeVarint(h.trackAlias));
  buf.push(...encodeVarint(h.groupId));
  if (h.subgroupId !== null) {
    buf.push(...encodeVarint(h.subgroupId));
  }
  if (h.publisherPriority !== null) {
    buf.push(h.publisherPriority & 0xff);
  }
  return new Uint8Array(buf);
}

/** Decode a SubgroupHeader from a buffer. */
export function decodeSubgroupHeader(
  buf: Uint8Array,
  offset: number
): { header: SubgroupHeader; bytesRead: number } {
  let pos = offset;
  const { value: streamType, bytesRead: r1 } = decodeVarint(buf, pos);
  pos += r1;

  const hasProperties = (streamType & 0x01) !== 0;
  const subgroupIdMode = (streamType >> 1) & 0x03;
  const endOfGroup = (streamType & 0x08) !== 0;

  const { value: trackAlias, bytesRead: r2 } = decodeVarint(buf, pos);
  pos += r2;
  const { value: groupId, bytesRead: r3 } = decodeVarint(buf, pos);
  pos += r3;

  let subgroupId: number | null = null;
  if (subgroupIdMode === 0x02) {
    const { value, bytesRead } = decodeVarint(buf, pos);
    pos += bytesRead;
    subgroupId = value;
  }

  let publisherPriority: number | null = null;
  if ((streamType & 0x20) === 0) {
    if (pos >= buf.length) throw new Error("subgroup_header: publisher priority truncated");
    publisherPriority = buf[pos];
    pos += 1;
  }

  return {
    header: { trackAlias, groupId, hasProperties, endOfGroup, subgroupId, publisherPriority },
    bytesRead: pos - offset,
  };
}
