import { describe, it, expect } from "vitest";
import { encodeVarint, decodeVarint } from "./varint.js";

function roundtrip(value: number): void {
  const buf = encodeVarint(value);
  const { value: decoded, bytesRead } = decodeVarint(buf, 0);
  expect(decoded).toBe(value);
  expect(bytesRead).toBe(buf.length);
}

describe("varint", () => {
  describe("roundtrip", () => {
    it("1-byte values (0..=127)", () => {
      roundtrip(0);
      roundtrip(1);
      roundtrip(37);
      roundtrip(127);
    });

    it("2-byte values (128..=16383)", () => {
      roundtrip(128);
      roundtrip(15293);
      roundtrip(16383);
    });

    it("3-byte values", () => {
      roundtrip(16384);
      roundtrip(2097151);
    });

    it("4-byte values", () => {
      roundtrip(2097152);
      roundtrip(494_878_333);
      roundtrip(268_435_455);
    });

    it("5-byte values", () => {
      roundtrip(268_435_456);
      roundtrip(34_359_738_367);
    });

    it("6-byte values", () => {
      roundtrip(34_359_738_368);
      roundtrip(2_893_212_287_960);
      roundtrip(4_398_046_511_103);
    });
  });

  describe("spec byte sequences", () => {
    it("37 -> [0x25]", () => {
      const buf = encodeVarint(37);
      expect(Array.from(buf)).toEqual([0x25]);
    });

    it("15293 -> [0xbb, 0xbd]", () => {
      const buf = encodeVarint(15293);
      expect(Array.from(buf)).toEqual([0xbb, 0xbd]);
    });

    it("494878333 encodes to 5 bytes (minimal)", () => {
      const buf = encodeVarint(494_878_333);
      expect(buf.length).toBe(5);
      roundtrip(494_878_333);
    });

    it("2893212287960 -> [0xfa, 0xa1, 0xa0, 0xe4, 0x03, 0xd8]", () => {
      const buf = encodeVarint(2_893_212_287_960);
      expect(Array.from(buf)).toEqual([0xfa, 0xa1, 0xa0, 0xe4, 0x03, 0xd8]);
    });
  });

  describe("non-minimal decode", () => {
    it("decodes non-minimal 37 encoded as 2 bytes [0x80, 0x25]", () => {
      const buf = new Uint8Array([0x80, 0x25]);
      const { value } = decodeVarint(buf, 0);
      expect(value).toBe(37);
    });
  });

  describe("errors", () => {
    it("throws on empty buffer", () => {
      expect(() => decodeVarint(new Uint8Array(0), 0)).toThrow();
    });

    it("throws on truncated buffer", () => {
      // 2-byte varint but only 1 byte provided
      expect(() => decodeVarint(new Uint8Array([0x80]), 0)).toThrow();
    });

    it("throws on invalid code point 0xFC", () => {
      expect(() =>
        decodeVarint(new Uint8Array([0xfc, 0, 0, 0, 0, 0, 0, 0]), 0)
      ).toThrow();
    });

    it("throws on invalid code point 0xFD", () => {
      expect(() =>
        decodeVarint(new Uint8Array([0xfd, 0, 0, 0, 0, 0, 0, 0]), 0)
      ).toThrow();
    });
  });

  describe("minimal encoding length", () => {
    it("uses 1 byte for 127", () => {
      expect(encodeVarint(127).length).toBe(1);
    });

    it("uses 2 bytes for 128", () => {
      expect(encodeVarint(128).length).toBe(2);
    });

    it("uses 2 bytes for 16383", () => {
      expect(encodeVarint(16383).length).toBe(2);
    });

    it("uses 3 bytes for 16384", () => {
      expect(encodeVarint(16384).length).toBe(3);
    });
  });
});
