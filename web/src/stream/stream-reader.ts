// StreamReader: buffered reader for WebTransport ReadableStream.
// WebTransport delivers data in arbitrarily-sized chunks, so we need
// to buffer and reassemble across chunk boundaries.

import { decodeVarint, varintByteLength } from "../wire/varint.js";

export class StreamReader {
  private reader: ReadableStreamDefaultReader<Uint8Array>;
  private buffer: Uint8Array = new Uint8Array(0);
  private done = false;

  constructor(readable: ReadableStream<Uint8Array>) {
    this.reader = readable.getReader();
  }

  /** Cancel the underlying stream (sends STOP_SENDING to the peer). */
  async cancel(): Promise<void> {
    await this.reader.cancel();
  }

  /** Read exactly `n` bytes. Throws on EOF before enough bytes. */
  async readExact(n: number): Promise<Uint8Array> {
    while (this.buffer.length < n) {
      if (this.done) {
        throw new Error("stream ended before enough bytes");
      }
      const { value, done } = await this.reader.read();
      if (done) {
        this.done = true;
        if (this.buffer.length < n) {
          throw new Error("stream ended before enough bytes");
        }
        break;
      }
      // Append to buffer
      const newBuf = new Uint8Array(this.buffer.length + value.length);
      newBuf.set(this.buffer, 0);
      newBuf.set(value, this.buffer.length);
      this.buffer = newBuf;
    }

    const result = this.buffer.slice(0, n);
    this.buffer = this.buffer.slice(n);
    return result;
  }

  /** Try to read exactly `n` bytes. Returns null on clean EOF (no partial data). */
  async tryReadExact(n: number): Promise<Uint8Array | null> {
    // If buffer is empty and stream may be done, try one read
    while (this.buffer.length < n) {
      if (this.done) {
        if (this.buffer.length === 0) return null;
        throw new Error("stream ended with partial data");
      }
      const { value, done } = await this.reader.read();
      if (done) {
        this.done = true;
        if (this.buffer.length === 0) return null;
        if (this.buffer.length < n) {
          throw new Error("stream ended with partial data");
        }
        break;
      }
      const newBuf = new Uint8Array(this.buffer.length + value.length);
      newBuf.set(this.buffer, 0);
      newBuf.set(value, this.buffer.length);
      this.buffer = newBuf;
    }

    const result = this.buffer.slice(0, n);
    this.buffer = this.buffer.slice(n);
    return result;
  }

  /** Read a single varint from the stream. Returns [value, rawBytes]. */
  async readVarint(): Promise<[number, Uint8Array]> {
    // Read first byte to determine length
    const first = await this.readExact(1);
    const len = varintByteLength(first[0]);

    if (len === 1) {
      const { value } = decodeVarint(first, 0);
      return [value, first];
    }

    // Read remaining bytes
    const rest = await this.readExact(len - 1);
    const raw = new Uint8Array(len);
    raw[0] = first[0];
    raw.set(rest, 1);

    const { value } = decodeVarint(raw, 0);
    return [value, raw];
  }

  /** Try to read a varint. Returns null on clean EOF. */
  async tryReadVarint(): Promise<[number, Uint8Array] | null> {
    const first = await this.tryReadExact(1);
    if (first === null) return null;

    const len = varintByteLength(first[0]);
    if (len === 1) {
      const { value } = decodeVarint(first, 0);
      return [value, first];
    }

    const rest = await this.readExact(len - 1);
    const raw = new Uint8Array(len);
    raw[0] = first[0];
    raw.set(rest, 1);

    const { value } = decodeVarint(raw, 0);
    return [value, raw];
  }

  /** Read a message frame (Type varint + u16 Length + Payload). */
  async readMessageFrame(): Promise<Uint8Array> {
    const [_type, typeBytes] = await this.readVarint();
    const lenBytes = await this.readExact(2);
    const payloadLen = (lenBytes[0] << 8) | lenBytes[1];
    const payload = await this.readExact(payloadLen);

    const result = new Uint8Array(typeBytes.length + 2 + payloadLen);
    result.set(typeBytes, 0);
    result.set(lenBytes, typeBytes.length);
    result.set(payload, typeBytes.length + 2);
    return result;
  }
}
