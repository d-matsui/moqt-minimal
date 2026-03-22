// Key-Value-Pair encoding (used in SETUP options).
// Wire format: count (varint) + (type_delta (varint) + value)...
// Even types: value is varint
// Odd types: value is length-prefixed bytes

import { encodeVarint, decodeVarint } from "./varint.js";

export interface KeyValuePair {
  typeId: number;
  value: Uint8Array | number; // number for even types, bytes for odd types
}

/** Encode Key-Value-Pairs with delta encoding on type IDs.
 * No count prefix — pairs are written until the end of the buffer.
 */
export function encodeKeyValuePairs(pairs: KeyValuePair[], buf: number[]): void {
  let prevType = 0;
  for (const pair of pairs) {
    const delta = pair.typeId - prevType;
    buf.push(...encodeVarint(delta));
    prevType = pair.typeId;

    if (pair.typeId % 2 === 0) {
      // Even type: varint value
      buf.push(...encodeVarint(pair.value as number));
    } else {
      // Odd type: length-prefixed bytes
      const bytes = pair.value as Uint8Array;
      buf.push(...encodeVarint(bytes.length));
      buf.push(...bytes);
    }
  }
}

/** Decode Key-Value-Pairs from a buffer until `end` offset.
 * No count prefix — reads until the end of the buffer region.
 */
export function decodeKeyValuePairs(
  buf: Uint8Array,
  offset: number,
  end?: number
): { pairs: KeyValuePair[]; bytesRead: number } {
  let pos = offset;
  const limit = end ?? buf.length;

  const pairs: KeyValuePair[] = [];
  let prevType = 0;
  while (pos < limit) {
    const { value: delta, bytesRead: deltaLen } = decodeVarint(buf, pos);
    pos += deltaLen;
    const typeId = prevType + delta;
    prevType = typeId;

    if (typeId % 2 === 0) {
      const { value, bytesRead: valLen } = decodeVarint(buf, pos);
      pos += valLen;
      pairs.push({ typeId, value });
    } else {
      const { value: len, bytesRead: lenBytes } = decodeVarint(buf, pos);
      pos += lenBytes;
      const value = buf.slice(pos, pos + len);
      pos += len;
      pairs.push({ typeId, value });
    }
  }

  return { pairs, bytesRead: pos - offset };
}
