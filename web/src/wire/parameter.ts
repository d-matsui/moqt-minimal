// Message Parameters (Section 9.3.1)
// Used in SUBSCRIBE, SUBSCRIBE_OK, etc.
// Wire format: count (varint) + (type_delta (varint) + value)...

import { encodeVarint, decodeVarint } from "./varint.js";

// Parameter type IDs
const PARAM_AUTHORIZATION = 0x02;
const PARAM_SUBSCRIPTION_FILTER = 0x04;
const PARAM_LARGEST_OBJECT = 0x05;
const PARAM_FORWARD = 0x06;

// Subscription filter values
export const FILTER_NEXT_GROUP_START = 0x02;

export interface MessageParameter {
  typeId: number;
  value: number | Uint8Array;
}

/** Create a NextGroupStart subscription filter parameter. */
export function subscriptionFilterNextGroupStart(): MessageParameter {
  return { typeId: PARAM_SUBSCRIPTION_FILTER, value: FILTER_NEXT_GROUP_START };
}

/** Encode message parameters. */
export function encodeParameters(params: MessageParameter[], buf: number[]): void {
  buf.push(...encodeVarint(params.length));
  let prevType = 0;
  for (const param of params) {
    const delta = param.typeId - prevType;
    buf.push(...encodeVarint(delta));
    prevType = param.typeId;

    if (typeof param.value === "number") {
      // Varint value
      buf.push(...encodeVarint(param.value));
    } else {
      // Length-prefixed bytes
      buf.push(...encodeVarint(param.value.length));
      buf.push(...param.value);
    }
  }
}

/** Decode message parameters. */
export function decodeParameters(
  buf: Uint8Array,
  offset: number
): { params: MessageParameter[]; bytesRead: number } {
  let pos = offset;
  const { value: count, bytesRead: countLen } = decodeVarint(buf, pos);
  pos += countLen;

  const params: MessageParameter[] = [];
  let prevType = 0;
  for (let i = 0; i < count; i++) {
    const { value: delta, bytesRead: deltaLen } = decodeVarint(buf, pos);
    pos += deltaLen;
    const typeId = prevType + delta;
    prevType = typeId;

    // Parameter value encoding depends on the type
    if (typeId === PARAM_SUBSCRIPTION_FILTER || typeId === PARAM_LARGEST_OBJECT || typeId === PARAM_FORWARD) {
      const { value, bytesRead: valLen } = decodeVarint(buf, pos);
      pos += valLen;
      params.push({ typeId, value });
    } else if (typeId === PARAM_AUTHORIZATION) {
      const { value: len, bytesRead: lenBytes } = decodeVarint(buf, pos);
      pos += lenBytes;
      const value = buf.slice(pos, pos + len);
      pos += len;
      params.push({ typeId, value });
    } else {
      // Unknown parameter: skip by reading length-prefixed bytes
      const { value: len, bytesRead: lenBytes } = decodeVarint(buf, pos);
      pos += lenBytes;
      pos += len;
    }
  }

  return { params, bytesRead: pos - offset };
}
