// PUBLISH_NAMESPACE message (Section 9.6)

import { encodeVarint, decodeVarint } from "./varint.js";
import { encodeMessage, decodeMessage, MSG_PUBLISH_NAMESPACE } from "./message.js";
import { encodeTrackNamespace, decodeTrackNamespace } from "./track-namespace.js";
import { encodeParameters, decodeParameters } from "./parameter.js";
import type { TrackNamespace } from "./track-namespace.js";
import type { MessageParameter } from "./parameter.js";

export interface PublishNamespaceMessage {
  requestId: number;
  requiredRequestIdDelta: number;
  trackNamespace: TrackNamespace;
  parameters: MessageParameter[];
}

export function encodePublishNamespace(msg: PublishNamespaceMessage): Uint8Array {
  const payload: number[] = [];
  payload.push(...encodeVarint(msg.requestId));
  payload.push(...encodeVarint(msg.requiredRequestIdDelta));
  encodeTrackNamespace(msg.trackNamespace, payload);
  encodeParameters(msg.parameters, payload);
  return encodeMessage(MSG_PUBLISH_NAMESPACE, new Uint8Array(payload));
}

export function decodePublishNamespace(frame: Uint8Array): PublishNamespaceMessage {
  const { msgType, payload } = decodeMessage(frame, 0);
  if (msgType !== MSG_PUBLISH_NAMESPACE) {
    throw new Error(`expected PUBLISH_NAMESPACE, got 0x${msgType.toString(16)}`);
  }
  let pos = 0;
  const { value: requestId, bytesRead: r1 } = decodeVarint(payload, pos);
  pos += r1;
  const { value: requiredRequestIdDelta, bytesRead: r2 } = decodeVarint(payload, pos);
  pos += r2;
  const { ns: trackNamespace, bytesRead: r3 } = decodeTrackNamespace(payload, pos);
  pos += r3;
  const { params: parameters } = decodeParameters(payload, pos);
  return { requestId, requiredRequestIdDelta, trackNamespace, parameters };
}
