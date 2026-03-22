// PUBLISH_DONE message (Section 9.12)

import { encodeVarint, decodeVarint } from "./varint.js";
import { encodeMessage, decodeMessage, MSG_PUBLISH_DONE } from "./message.js";

export interface PublishDoneMessage {
  statusCode: number;
  streamCount: number;
  reasonPhrase: string;
}

export function encodePublishDone(msg: PublishDoneMessage): Uint8Array {
  const payload: number[] = [];
  payload.push(...encodeVarint(msg.statusCode));
  payload.push(...encodeVarint(msg.streamCount));
  const reasonBytes = new TextEncoder().encode(msg.reasonPhrase);
  payload.push(...encodeVarint(reasonBytes.length));
  payload.push(...reasonBytes);
  return encodeMessage(MSG_PUBLISH_DONE, new Uint8Array(payload));
}

export function decodePublishDone(frame: Uint8Array): PublishDoneMessage {
  const { msgType, payload } = decodeMessage(frame, 0);
  if (msgType !== MSG_PUBLISH_DONE) {
    throw new Error(`expected PUBLISH_DONE, got 0x${msgType.toString(16)}`);
  }
  let pos = 0;
  const { value: statusCode, bytesRead: r1 } = decodeVarint(payload, pos);
  pos += r1;
  const { value: streamCount, bytesRead: r2 } = decodeVarint(payload, pos);
  pos += r2;
  const { value: reasonLen, bytesRead: r3 } = decodeVarint(payload, pos);
  pos += r3;
  const reasonPhrase = new TextDecoder().decode(payload.slice(pos, pos + reasonLen));
  return { statusCode, streamCount, reasonPhrase };
}
