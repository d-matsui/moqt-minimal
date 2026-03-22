// MOQT message framing: Type (varint) + Length (u16 BE) + Payload
//
// Note: Length is a fixed 16-bit big-endian integer, NOT a varint.

import { encodeVarint, decodeVarint } from "./varint.js";

// Message Type IDs (Section 9)
export const MSG_SUBSCRIBE = 0x03;
export const MSG_SUBSCRIBE_OK = 0x04;
export const MSG_REQUEST_ERROR = 0x05;
export const MSG_PUBLISH_NAMESPACE = 0x06;
export const MSG_REQUEST_OK = 0x07;
export const MSG_PUBLISH_DONE = 0x0b;
export const MSG_SETUP = 0x2f00;

/** Encode a control message: Type (varint) + Length (u16 BE) + Payload. */
export function encodeMessage(msgType: number, payload: Uint8Array): Uint8Array {
  const typeBytes = encodeVarint(msgType);
  const len = payload.length;
  const buf = new Uint8Array(typeBytes.length + 2 + len);
  buf.set(typeBytes, 0);
  buf[typeBytes.length] = (len >> 8) & 0xff;
  buf[typeBytes.length + 1] = len & 0xff;
  buf.set(payload, typeBytes.length + 2);
  return buf;
}

/** Decode a control message from a buffer at the given offset. */
export function decodeMessage(
  buf: Uint8Array,
  offset: number
): { msgType: number; payload: Uint8Array; bytesRead: number } {
  const { value: msgType, bytesRead: typeLen } = decodeVarint(buf, offset);
  const lenOffset = offset + typeLen;

  if (lenOffset + 2 > buf.length) {
    throw new Error("message: need 2 bytes for length");
  }

  const payloadLen = (buf[lenOffset] << 8) | buf[lenOffset + 1];
  const payloadOffset = lenOffset + 2;

  if (payloadOffset + payloadLen > buf.length) {
    throw new Error(
      `message: need ${payloadLen} bytes for payload, have ${buf.length - payloadOffset}`
    );
  }

  const payload = buf.slice(payloadOffset, payloadOffset + payloadLen);
  return {
    msgType,
    payload,
    bytesRead: typeLen + 2 + payloadLen,
  };
}
