// SETUP message (Section 9.4)
// Type ID: 0x2F00
// Body: setup_options (Key-Value-Pairs)

import { encodeMessage, decodeMessage, MSG_SETUP } from "./message.js";
import { encodeKeyValuePairs, decodeKeyValuePairs } from "./key-value-pair.js";
import type { KeyValuePair } from "./key-value-pair.js";

// Setup option type IDs (must match Rust: setup.rs)
export const SETUP_PATH = 0x01;       // odd = bytes
export const SETUP_AUTHORITY = 0x05;   // odd = bytes

export interface SetupMessage {
  options: KeyValuePair[];
}

/** Create a client SETUP message with PATH and AUTHORITY. */
export function clientSetup(path: string, authority: string): SetupMessage {
  return {
    options: [
      { typeId: SETUP_PATH, value: new TextEncoder().encode(path) },
      { typeId: SETUP_AUTHORITY, value: new TextEncoder().encode(authority) },
    ],
  };
}

/** Encode a SETUP message (full frame with type + length + payload). */
export function encodeSetup(msg: SetupMessage): Uint8Array {
  const payload: number[] = [];
  encodeKeyValuePairs(msg.options, payload);
  return encodeMessage(MSG_SETUP, new Uint8Array(payload));
}

/** Decode a SETUP message from a full frame. */
export function decodeSetup(frame: Uint8Array): SetupMessage {
  const { msgType, payload } = decodeMessage(frame, 0);
  if (msgType !== MSG_SETUP) {
    throw new Error(`expected SETUP (0x2F00), got 0x${msgType.toString(16)}`);
  }
  const { pairs } = decodeKeyValuePairs(payload, 0);
  return { options: pairs };
}
