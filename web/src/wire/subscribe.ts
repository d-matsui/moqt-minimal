// SUBSCRIBE message (Section 9.7)

import { encodeVarint, decodeVarint } from "./varint.js";
import { encodeMessage, decodeMessage, MSG_SUBSCRIBE } from "./message.js";
import { encodeTrackNamespace, decodeTrackNamespace } from "./track-namespace.js";
import { encodeParameters, decodeParameters } from "./parameter.js";
import type { TrackNamespace } from "./track-namespace.js";
import type { MessageParameter } from "./parameter.js";

export interface SubscribeMessage {
  requestId: number;
  requiredRequestIdDelta: number;
  trackNamespace: TrackNamespace;
  trackName: Uint8Array;
  parameters: MessageParameter[];
}

export function encodeSubscribe(msg: SubscribeMessage): Uint8Array {
  const payload: number[] = [];
  payload.push(...encodeVarint(msg.requestId));
  payload.push(...encodeVarint(msg.requiredRequestIdDelta));
  encodeTrackNamespace(msg.trackNamespace, payload);
  payload.push(...encodeVarint(msg.trackName.length));
  payload.push(...msg.trackName);
  encodeParameters(msg.parameters, payload);
  return encodeMessage(MSG_SUBSCRIBE, new Uint8Array(payload));
}

export function decodeSubscribe(frame: Uint8Array): SubscribeMessage {
  const { msgType, payload } = decodeMessage(frame, 0);
  if (msgType !== MSG_SUBSCRIBE) {
    throw new Error(`expected SUBSCRIBE, got 0x${msgType.toString(16)}`);
  }
  let pos = 0;
  const { value: requestId, bytesRead: r1 } = decodeVarint(payload, pos);
  pos += r1;
  const { value: requiredRequestIdDelta, bytesRead: r2 } = decodeVarint(payload, pos);
  pos += r2;
  const { ns: trackNamespace, bytesRead: r3 } = decodeTrackNamespace(payload, pos);
  pos += r3;
  const { value: nameLen, bytesRead: r4 } = decodeVarint(payload, pos);
  pos += r4;
  const trackName = payload.slice(pos, pos + nameLen);
  pos += nameLen;
  const { params: parameters } = decodeParameters(payload, pos);
  return { requestId, requiredRequestIdDelta, trackNamespace, trackName, parameters };
}
