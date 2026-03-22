import { describe, it, expect } from "vitest";
import { encodeSetup, decodeSetup, clientSetup } from "./setup.js";
import { encodePublishNamespace, decodePublishNamespace } from "./publish-namespace.js";
import { encodeSubscribe, decodeSubscribe } from "./subscribe.js";
import { encodeSubscribeOk, decodeSubscribeOk } from "./subscribe-ok.js";
import { encodeRequestOk, decodeRequestOk } from "./request-ok.js";
import { encodePublishDone, decodePublishDone } from "./publish-done.js";
import { encodeSubgroupHeader, decodeSubgroupHeader } from "./subgroup-header.js";
import { encodeObjectHeader, decodeObjectHeader } from "./object.js";
import { trackNamespaceFrom } from "./track-namespace.js";
import { subscriptionFilterNextGroupStart } from "./parameter.js";

describe("setup", () => {
  it("roundtrip client setup", () => {
    const msg = clientSetup("/", "localhost");
    const buf = encodeSetup(msg);
    const decoded = decodeSetup(buf);
    expect(decoded.options.length).toBe(2);
  });
});

describe("publish-namespace", () => {
  it("roundtrip", () => {
    const msg = {
      requestId: 0,
      requiredRequestIdDelta: 0,
      trackNamespace: trackNamespaceFrom(["example"]),
      parameters: [],
    };
    const buf = encodePublishNamespace(msg);
    const decoded = decodePublishNamespace(buf);
    expect(decoded.requestId).toBe(0);
    expect(decoded.trackNamespace.fields.length).toBe(1);
    expect(new TextDecoder().decode(decoded.trackNamespace.fields[0])).toBe("example");
  });
});

describe("subscribe", () => {
  it("roundtrip with filter", () => {
    const msg = {
      requestId: 2,
      requiredRequestIdDelta: 0,
      trackNamespace: trackNamespaceFrom(["example"]),
      trackName: new TextEncoder().encode("video"),
      parameters: [subscriptionFilterNextGroupStart()],
    };
    const buf = encodeSubscribe(msg);
    const decoded = decodeSubscribe(buf);
    expect(decoded.requestId).toBe(2);
    expect(new TextDecoder().decode(decoded.trackName)).toBe("video");
    expect(decoded.parameters.length).toBe(1);
  });

  it("roundtrip without parameters", () => {
    const msg = {
      requestId: 0,
      requiredRequestIdDelta: 0,
      trackNamespace: trackNamespaceFrom(["ns"]),
      trackName: new TextEncoder().encode("track"),
      parameters: [],
    };
    const buf = encodeSubscribe(msg);
    const decoded = decodeSubscribe(buf);
    expect(decoded.parameters.length).toBe(0);
  });
});

describe("subscribe-ok", () => {
  it("roundtrip", () => {
    const msg = {
      trackAlias: 1,
      parameters: [],
      trackPropertiesRaw: new Uint8Array(0),
    };
    const buf = encodeSubscribeOk(msg);
    const decoded = decodeSubscribeOk(buf);
    expect(decoded.trackAlias).toBe(1);
    expect(decoded.parameters.length).toBe(0);
    expect(decoded.trackPropertiesRaw.length).toBe(0);
  });
});

describe("request-ok", () => {
  it("roundtrip", () => {
    const msg = { parameters: [] };
    const buf = encodeRequestOk(msg);
    const decoded = decodeRequestOk(buf);
    expect(decoded.parameters.length).toBe(0);
  });
});

describe("publish-done", () => {
  it("roundtrip", () => {
    const msg = { statusCode: 0, streamCount: 5, reasonPhrase: "" };
    const buf = encodePublishDone(msg);
    const decoded = decodePublishDone(buf);
    expect(decoded.statusCode).toBe(0);
    expect(decoded.streamCount).toBe(5);
    expect(decoded.reasonPhrase).toBe("");
  });

  it("roundtrip with reason", () => {
    const msg = { statusCode: 1, streamCount: 0, reasonPhrase: "done" };
    const buf = encodePublishDone(msg);
    const decoded = decodePublishDone(buf);
    expect(decoded.reasonPhrase).toBe("done");
  });
});

describe("subgroup-header", () => {
  it("roundtrip basic (end_of_group, no subgroup_id, default priority)", () => {
    const h = {
      trackAlias: 1,
      groupId: 0,
      hasProperties: false,
      endOfGroup: true,
      subgroupId: null,
      publisherPriority: null,
    };
    const buf = encodeSubgroupHeader(h);
    // Type byte should be 0x38
    expect(buf[0]).toBe(0x38);
    const { header } = decodeSubgroupHeader(buf, 0);
    expect(header.trackAlias).toBe(1);
    expect(header.groupId).toBe(0);
    expect(header.endOfGroup).toBe(true);
    expect(header.subgroupId).toBeNull();
    expect(header.publisherPriority).toBeNull();
  });

  it("roundtrip with subgroup_id", () => {
    const h = {
      trackAlias: 1,
      groupId: 0,
      hasProperties: false,
      endOfGroup: true,
      subgroupId: 5,
      publisherPriority: null,
    };
    const buf = encodeSubgroupHeader(h);
    const { header } = decodeSubgroupHeader(buf, 0);
    expect(header.subgroupId).toBe(5);
  });

  it("roundtrip with publisher priority", () => {
    const h = {
      trackAlias: 1,
      groupId: 0,
      hasProperties: false,
      endOfGroup: true,
      subgroupId: null,
      publisherPriority: 64,
    };
    const buf = encodeSubgroupHeader(h);
    const { header } = decodeSubgroupHeader(buf, 0);
    expect(header.publisherPriority).toBe(64);
  });
});

describe("object-header", () => {
  it("roundtrip", () => {
    const h = { objectIdDelta: 0, payloadLength: 100 };
    const buf = encodeObjectHeader(h);
    const { header } = decodeObjectHeader(buf, 0);
    expect(header.objectIdDelta).toBe(0);
    expect(header.payloadLength).toBe(100);
  });

  it("roundtrip with nonzero delta", () => {
    const h = { objectIdDelta: 3, payloadLength: 50 };
    const buf = encodeObjectHeader(h);
    const { header } = decodeObjectHeader(buf, 0);
    expect(header.objectIdDelta).toBe(3);
    expect(header.payloadLength).toBe(50);
  });
});
