import { describe, it, expect } from "vitest";
import { encodeMessage, decodeMessage, MSG_SUBSCRIBE, MSG_SETUP } from "./message.js";

describe("message framing", () => {
  it("roundtrip with empty payload", () => {
    const buf = encodeMessage(MSG_SUBSCRIBE, new Uint8Array(0));
    const { msgType, payload, bytesRead } = decodeMessage(buf, 0);
    expect(msgType).toBe(MSG_SUBSCRIBE);
    expect(payload.length).toBe(0);
    expect(bytesRead).toBe(buf.length);
  });

  it("roundtrip with payload", () => {
    const data = new Uint8Array([0x01, 0x02, 0x03]);
    const buf = encodeMessage(MSG_SUBSCRIBE, data);
    const { msgType, payload, bytesRead } = decodeMessage(buf, 0);
    expect(msgType).toBe(MSG_SUBSCRIBE);
    expect(Array.from(payload)).toEqual([0x01, 0x02, 0x03]);
    expect(bytesRead).toBe(buf.length);
  });

  it("length field is u16 big-endian (not varint)", () => {
    const data = new Uint8Array(300); // > 255, needs 2 bytes
    const buf = encodeMessage(MSG_SUBSCRIBE, data);
    // After the type varint (1 byte for 0x03), next 2 bytes should be 0x01 0x2C (300)
    expect(buf[1]).toBe(0x01);
    expect(buf[2]).toBe(0x2c);
  });

  it("SETUP type 0x2F00 uses 2-byte varint", () => {
    const buf = encodeMessage(MSG_SETUP, new Uint8Array(0));
    // 0x2F00 encoded as varint: 0xAF 0x00 (2 bytes, prefix 10)
    // 0x2F00 = 12032, fits in 2-byte varint (max 16383)
    const { msgType } = decodeMessage(buf, 0);
    expect(msgType).toBe(MSG_SETUP);
  });

  it("throws on truncated length", () => {
    // Type varint (1 byte) + only 1 byte of length (need 2)
    const buf = new Uint8Array([0x03, 0x00]);
    expect(() => decodeMessage(buf, 0)).toThrow();
  });

  it("throws on truncated payload", () => {
    // Type + length=5 + only 3 bytes of payload
    const buf = new Uint8Array([0x03, 0x00, 0x05, 0x01, 0x02, 0x03]);
    expect(() => decodeMessage(buf, 0)).toThrow();
  });
});
