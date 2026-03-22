// SUBSCRIBE_OK message (Section 9.8)

import { encodeVarint, decodeVarint } from "./varint.js";
import { encodeMessage, decodeMessage, MSG_SUBSCRIBE_OK } from "./message.js";
import { encodeParameters, decodeParameters } from "./parameter.js";
import type { MessageParameter } from "./parameter.js";

export interface SubscribeOkMessage {
  trackAlias: number;
  parameters: MessageParameter[];
  trackPropertiesRaw: Uint8Array;
}

export function encodeSubscribeOk(msg: SubscribeOkMessage): Uint8Array {
  const payload: number[] = [];
  payload.push(...encodeVarint(msg.trackAlias));
  encodeParameters(msg.parameters, payload);
  // Track Properties: length-prefixed raw bytes
  payload.push(...encodeVarint(msg.trackPropertiesRaw.length));
  payload.push(...msg.trackPropertiesRaw);
  return encodeMessage(MSG_SUBSCRIBE_OK, new Uint8Array(payload));
}

export function decodeSubscribeOk(frame: Uint8Array): SubscribeOkMessage {
  const { msgType, payload } = decodeMessage(frame, 0);
  if (msgType !== MSG_SUBSCRIBE_OK) {
    throw new Error(`expected SUBSCRIBE_OK, got 0x${msgType.toString(16)}`);
  }
  let pos = 0;
  const { value: trackAlias, bytesRead: r1 } = decodeVarint(payload, pos);
  pos += r1;
  const { params: parameters, bytesRead: r2 } = decodeParameters(payload, pos);
  pos += r2;
  const { value: propsLen, bytesRead: r3 } = decodeVarint(payload, pos);
  pos += r3;
  const trackPropertiesRaw = payload.slice(pos, pos + propsLen);
  return { trackAlias, parameters, trackPropertiesRaw };
}
