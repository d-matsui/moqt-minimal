// MoqtSession: high-level MOQT session API over WebTransport.
//
// Handles SETUP exchange and provides methods for:
// - publishNamespace()
// - subscribe()
// - waitForSubscribe()
// - openSubgroup()
// - nextDataStream()

import { StreamReader } from "./stream/stream-reader.js";
import { encodeSetup, decodeSetup, clientSetup } from "./wire/setup.js";
import {
  encodePublishNamespace,
  type PublishNamespaceMessage,
} from "./wire/publish-namespace.js";
import { encodeSubscribe, decodeSubscribe, type SubscribeMessage } from "./wire/subscribe.js";
import {
  encodeSubscribeOk,
  decodeSubscribeOk,
  type SubscribeOkMessage,
} from "./wire/subscribe-ok.js";
import { decodeRequestOk } from "./wire/request-ok.js";
import { encodePublishDone, decodePublishDone, type PublishDoneMessage } from "./wire/publish-done.js";
import { decodeMessage } from "./wire/message.js";
import {
  MSG_SUBSCRIBE,
  MSG_SUBSCRIBE_OK,
  MSG_REQUEST_OK,
  MSG_REQUEST_ERROR,
  MSG_PUBLISH_DONE,
  MSG_PUBLISH_NAMESPACE,
} from "./wire/message.js";
import {
  encodeSubgroupHeader,
  decodeSubgroupHeader,
  type SubgroupHeader,
} from "./wire/subgroup-header.js";
import { encodeObjectHeader, decodeObjectHeader } from "./wire/object.js";
import { trackNamespaceFrom, type TrackNamespace } from "./wire/track-namespace.js";
import {
  subscriptionFilterNextGroupStart,
  type MessageParameter,
} from "./wire/parameter.js";
import { encodeVarint } from "./wire/varint.js";

/** A writable subgroup (data stream). */
export class SubgroupWriter {
  private writer: WritableStreamDefaultWriter<Uint8Array>;

  constructor(writer: WritableStreamDefaultWriter<Uint8Array>) {
    this.writer = writer;
  }

  /** Write an object payload. ObjectHeader is generated internally. */
  async writeObject(payload: Uint8Array): Promise<void> {
    const header = encodeObjectHeader({
      objectIdDelta: 0,
      payloadLength: payload.length,
    });
    const buf = new Uint8Array(header.length + payload.length);
    buf.set(header, 0);
    buf.set(payload, header.length);
    await this.writer.write(buf);
  }

  /** Finish the stream (send FIN). */
  async finish(): Promise<void> {
    await this.writer.close();
  }
}

/** A readable subgroup (data stream). */
export class SubgroupReader {
  readonly header: SubgroupHeader;
  private reader: StreamReader;

  constructor(header: SubgroupHeader, reader: StreamReader) {
    this.header = header;
    this.reader = reader;
  }

  get trackAlias(): number {
    return this.header.trackAlias;
  }

  get groupId(): number {
    return this.header.groupId;
  }

  /** Read the next object payload. Returns null on stream end. */
  async readObject(): Promise<Uint8Array | null> {
    const result = await this.reader.tryReadVarint();
    if (result === null) return null;

    const [objectIdDelta, _deltaBytes] = result;
    const [payloadLength, _lenBytes] = await this.reader.readVarint();

    if (payloadLength === 0) return new Uint8Array(0);
    return await this.reader.readExact(payloadLength);
  }
}

/** An incoming SUBSCRIBE request (publisher side). */
export class SubscribeRequest {
  readonly message: SubscribeMessage;
  private sendWriter: WritableStreamDefaultWriter<Uint8Array>;

  constructor(
    message: SubscribeMessage,
    sendWriter: WritableStreamDefaultWriter<Uint8Array>
  ) {
    this.message = message;
    this.sendWriter = sendWriter;
  }

  /** Accept the subscription with the given track alias. */
  async accept(trackAlias: number): Promise<void> {
    const frame = encodeSubscribeOk({
      trackAlias,
      parameters: [],
      trackPropertiesRaw: new Uint8Array(0),
    });
    await this.sendWriter.write(frame);
  }

  /** Send PUBLISH_DONE to signal end of publishing. */
  async sendPublishDone(streamCount: number): Promise<void> {
    const frame = encodePublishDone({
      statusCode: 0,
      streamCount,
      reasonPhrase: "",
    });
    await this.sendWriter.write(frame);
  }
}

/** An established subscription (subscriber side). */
export class Subscription {
  readonly trackAlias: number;
  private recvReader: StreamReader;

  constructor(trackAlias: number, recvReader: StreamReader) {
    this.trackAlias = trackAlias;
    this.recvReader = recvReader;
  }

  /** Wait for PUBLISH_DONE from the publisher. */
  async recvPublishDone(): Promise<PublishDoneMessage> {
    const frame = await this.recvReader.readMessageFrame();
    return decodePublishDone(frame);
  }
}

export type SessionEvent =
  | { type: "subscribe"; request: SubscribeRequest }
  | { type: "dataStream"; reader: SubgroupReader };

/** A MOQT session over WebTransport. */
export class MoqtSession {
  private transport: WebTransport;
  private nextRequestId = 0;

  private constructor(transport: WebTransport) {
    this.transport = transport;
  }

  /** Connect to a relay and perform SETUP exchange. */
  static async connect(url: string): Promise<MoqtSession> {
    const transport = new WebTransport(url);
    await transport.ready;

    const session = new MoqtSession(transport);
    await session.setupExchange();
    return session;
  }

  private async setupExchange(): Promise<void> {
    // Send SETUP on a new unidirectional stream
    const sendStream = await this.transport.createUnidirectionalStream();
    const writer = sendStream.getWriter();
    const setupMsg = clientSetup("/", "localhost");
    await writer.write(encodeSetup(setupMsg));
    // Don't close the control stream (must stay open per spec)

    // Read relay's SETUP from an incoming unidirectional stream
    const uniReader = this.transport.incomingUnidirectionalStreams.getReader();
    const { value: recvStream } = await uniReader.read();
    uniReader.releaseLock();
    if (!recvStream) throw new Error("no incoming control stream");

    const reader = new StreamReader(recvStream);
    const frame = await reader.readMessageFrame();
    const _serverSetup = decodeSetup(frame);
  }

  private allocateRequestId(): number {
    const id = this.nextRequestId;
    this.nextRequestId += 2; // even IDs for client
    return id;
  }

  /** Register a namespace with the relay. */
  async publishNamespace(namespace: string[]): Promise<void> {
    const bidi = await this.transport.createBidirectionalStream();
    const writer = bidi.writable.getWriter();
    const reader = new StreamReader(bidi.readable);

    const msg: PublishNamespaceMessage = {
      requestId: this.allocateRequestId(),
      requiredRequestIdDelta: 0,
      trackNamespace: trackNamespaceFrom(namespace),
      parameters: [],
    };
    await writer.write(encodePublishNamespace(msg));

    const frame = await reader.readMessageFrame();
    const { msgType } = decodeMessage(frame, 0);
    if (msgType === MSG_REQUEST_ERROR) {
      throw new Error("PUBLISH_NAMESPACE rejected");
    }
    if (msgType !== MSG_REQUEST_OK) {
      throw new Error(`unexpected response: 0x${msgType.toString(16)}`);
    }
  }

  /** Subscribe to a track. */
  async subscribe(
    namespace: string[],
    trackName: string,
    params: MessageParameter[] = [subscriptionFilterNextGroupStart()]
  ): Promise<Subscription> {
    const bidi = await this.transport.createBidirectionalStream();
    const writer = bidi.writable.getWriter();
    const reader = new StreamReader(bidi.readable);

    const msg: SubscribeMessage = {
      requestId: this.allocateRequestId(),
      requiredRequestIdDelta: 0,
      trackNamespace: trackNamespaceFrom(namespace),
      trackName: new TextEncoder().encode(trackName),
      parameters: params,
    };
    await writer.write(encodeSubscribe(msg));

    const frame = await reader.readMessageFrame();
    const { msgType } = decodeMessage(frame, 0);
    if (msgType === MSG_REQUEST_ERROR) {
      throw new Error("SUBSCRIBE rejected");
    }
    if (msgType !== MSG_SUBSCRIBE_OK) {
      throw new Error(`unexpected response: 0x${msgType.toString(16)}`);
    }
    const ok = decodeSubscribeOk(frame);
    return new Subscription(ok.trackAlias, reader);
  }

  /** Wait for the next incoming event (SUBSCRIBE or data stream). */
  async nextEvent(): Promise<SessionEvent> {
    // Race between incoming bidi and uni streams
    const bidiReader = this.transport.incomingBidirectionalStreams.getReader();
    const uniReader = this.transport.incomingUnidirectionalStreams.getReader();

    try {
      const result = await Promise.race([
        bidiReader.read().then((r) => ({ kind: "bidi" as const, ...r })),
        uniReader.read().then((r) => ({ kind: "uni" as const, ...r })),
      ]);

      if (result.kind === "bidi" && result.value) {
        bidiReader.releaseLock();
        uniReader.releaseLock();

        const stream = result.value;
        const reader = new StreamReader(stream.readable);
        const frame = await reader.readMessageFrame();
        const { msgType } = decodeMessage(frame, 0);

        if (msgType === MSG_SUBSCRIBE) {
          const sub = decodeSubscribe(frame);
          const writer = stream.writable.getWriter();
          return { type: "subscribe", request: new SubscribeRequest(sub, writer) };
        }
        throw new Error(`unexpected bidi message: 0x${msgType.toString(16)}`);
      }

      if (result.kind === "uni" && result.value) {
        bidiReader.releaseLock();
        uniReader.releaseLock();

        const reader = new StreamReader(result.value);
        // Read SubgroupHeader
        const [streamType, typeBytes] = await reader.readVarint();
        const [trackAlias, aliasBytes] = await reader.readVarint();
        const [groupId, groupBytes] = await reader.readVarint();

        // Reconstruct raw bytes for decoding
        const raw: number[] = [...typeBytes, ...aliasBytes, ...groupBytes];

        // Optional Subgroup ID
        const subgroupIdMode = (streamType >> 1) & 0x03;
        if (subgroupIdMode === 0x02) {
          const [_sid, sidBytes] = await reader.readVarint();
          raw.push(...sidBytes);
        }

        // Optional Publisher Priority
        if ((streamType & 0x20) === 0) {
          const priorityByte = await reader.readExact(1);
          raw.push(priorityByte[0]);
        }

        const { header } = decodeSubgroupHeader(new Uint8Array(raw), 0);
        return { type: "dataStream", reader: new SubgroupReader(header, reader) };
      }

      throw new Error("no incoming stream");
    } finally {
      // Ensure locks are released if not already
      try { bidiReader.releaseLock(); } catch {}
      try { uniReader.releaseLock(); } catch {}
    }
  }

  /** Open a subgroup for writing objects. */
  async openSubgroup(
    trackAlias: number,
    groupId: number,
    subgroupId: number
  ): Promise<SubgroupWriter> {
    const stream = await this.transport.createUnidirectionalStream();
    const writer = stream.getWriter();

    const header = encodeSubgroupHeader({
      trackAlias,
      groupId,
      hasProperties: false,
      endOfGroup: true,
      subgroupId,
      publisherPriority: null,
    });
    await writer.write(header);

    return new SubgroupWriter(writer);
  }

  /** Close the session. */
  close(): void {
    this.transport.close();
  }
}
