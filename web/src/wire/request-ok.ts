// REQUEST_OK message (Section 9.9)

import { encodeMessage, decodeMessage, MSG_REQUEST_OK } from "./message.js";
import { encodeParameters, decodeParameters } from "./parameter.js";
import type { MessageParameter } from "./parameter.js";

export interface RequestOkMessage {
  parameters: MessageParameter[];
}

export function encodeRequestOk(msg: RequestOkMessage): Uint8Array {
  const payload: number[] = [];
  encodeParameters(msg.parameters, payload);
  return encodeMessage(MSG_REQUEST_OK, new Uint8Array(payload));
}

export function decodeRequestOk(frame: Uint8Array): RequestOkMessage {
  const { msgType, payload } = decodeMessage(frame, 0);
  if (msgType !== MSG_REQUEST_OK) {
    throw new Error(`expected REQUEST_OK, got 0x${msgType.toString(16)}`);
  }
  const { params: parameters } = decodeParameters(payload, 0);
  return { parameters };
}
