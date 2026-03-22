// MOQT Variable-Length Integer encoding/decoding (Section 1.4.1)
//
// Prefix bits of the first byte determine the byte length:
//   0         -> 1 byte  (7-bit value, max 127)
//   10        -> 2 bytes (14-bit value, max 16,383)
//   110       -> 3 bytes (21-bit value, max 2,097,151)
//   1110      -> 4 bytes (28-bit value, max 268,435,455)
//   11110     -> 5 bytes (35-bit value, max 34,359,738,367)
//   111110    -> 6 bytes (42-bit value, max 4,398,046,511,103)
//   1111110x  -> INVALID (0xFC, 0xFD)
//   11111110  -> 8 bytes (56-bit value)
//   11111111  -> 9 bytes (64-bit value)
//
// Note: No 7-byte encoding. JavaScript Number is safe up to 2^53,
// which covers all values up to the 6-byte tier. 8-byte and 9-byte
// tiers would need BigInt for full range, but in practice MOQT
// values stay within Number.MAX_SAFE_INTEGER.

// Byte length tiers: [maxValue, byteCount, prefixBits, prefixMask]
const TIERS: Array<[number, number, number, number]> = [
  [0x7f, 1, 0x00, 0x80],             // prefix 0
  [0x3fff, 2, 0x80, 0xc0],           // prefix 10
  [0x1fffff, 3, 0xc0, 0xe0],         // prefix 110
  [0x0fffffff, 4, 0xe0, 0xf0],       // prefix 1110
  [0x07ffffffff, 5, 0xf0, 0xf8],     // prefix 11110
  [0x03ffffffffff, 6, 0xf8, 0xfc],   // prefix 111110
];

/** Encode a number as a MOQT variable-length integer (minimal encoding). */
export function encodeVarint(value: number): Uint8Array {
  // Find the smallest tier that fits
  for (const [maxVal, byteCount, prefix] of TIERS) {
    if (value <= maxVal) {
      const buf = new Uint8Array(byteCount);
      // Write value as big-endian, then OR prefix into first byte
      for (let i = byteCount - 1; i >= 0; i--) {
        buf[i] = value & 0xff;
        value = Math.floor(value / 256);
      }
      buf[0] |= prefix;
      return buf;
    }
  }

  // 8-byte tier (56-bit value, max 72,057,594,037,927,935)
  if (value <= 0x00ffffffffffffff) {
    const buf = new Uint8Array(8);
    buf[0] = 0xfe;
    for (let i = 7; i >= 1; i--) {
      buf[i] = value & 0xff;
      value = Math.floor(value / 256);
    }
    return buf;
  }

  // 9-byte tier (full 64-bit) - unlikely in practice
  const buf = new Uint8Array(9);
  buf[0] = 0xff;
  for (let i = 8; i >= 1; i--) {
    buf[i] = value & 0xff;
    value = Math.floor(value / 256);
  }
  return buf;
}

/** Determine byte length from the first byte of a varint. */
export function varintByteLength(firstByte: number): number {
  if ((firstByte & 0x80) === 0) return 1;
  if ((firstByte & 0xc0) === 0x80) return 2;
  if ((firstByte & 0xe0) === 0xc0) return 3;
  if ((firstByte & 0xf0) === 0xe0) return 4;
  if ((firstByte & 0xf8) === 0xf0) return 5;
  if ((firstByte & 0xfc) === 0xf8) return 6;
  if (firstByte === 0xfc || firstByte === 0xfd) {
    throw new Error(`invalid varint code point 0x${firstByte.toString(16).toUpperCase()}`);
  }
  if (firstByte === 0xfe) return 8;
  return 9; // 0xff
}

/** Decode a MOQT variable-length integer from a buffer at the given offset. */
export function decodeVarint(
  buf: Uint8Array,
  offset: number
): { value: number; bytesRead: number } {
  if (offset >= buf.length) {
    throw new Error("varint: empty buffer");
  }

  const firstByte = buf[offset];
  const len = varintByteLength(firstByte);

  if (offset + len > buf.length) {
    throw new Error(`varint: need ${len} bytes, have ${buf.length - offset}`);
  }

  // Value masks per byte length (strips prefix bits)
  const masks: Record<number, number> = {
    1: 0x7f,
    2: 0x3fff,
    3: 0x1fffff,
    4: 0x0fffffff,
    5: 0x07ffffffff,
    6: 0x03ffffffffff,
    8: 0x00ffffffffffffff,
    9: Number.MAX_SAFE_INTEGER, // best we can do without BigInt
  };

  // Combine bytes as big-endian
  let value = 0;
  for (let i = 0; i < len; i++) {
    value = value * 256 + buf[offset + i];
  }

  // Strip prefix bits
  const mask = masks[len];
  if (mask !== undefined) {
    // For values that fit in Number, use bitwise-safe masking
    // Math.floor handles the floating-point intermediate
    value = len <= 4 ? (value & mask) : value % (mask + 1);
  }

  return { value, bytesRead: len };
}
