// MOQT Browser Subscriber
// Receives video via WebTransport from the relay, decodes with WebCodecs VP8,
// and renders to canvas.

import { MoqtSession, SubgroupReader } from "./session.js";

const $ = (id: string) => document.getElementById(id)!;

let session: MoqtSession | null = null;
let decoder: VideoDecoder | null = null;
let canvas: HTMLCanvasElement;
let ctx: CanvasRenderingContext2D;

async function start() {
  const url = ($("relay-url") as HTMLInputElement).value;
  const namespace = ($("namespace") as HTMLInputElement).value;
  const trackName = ($("track-name") as HTMLInputElement).value;

  canvas = $("canvas") as HTMLCanvasElement;
  ctx = canvas.getContext("2d")!;

  log("Connecting...");
  session = await MoqtSession.connect(url);
  log("Connected. SETUP complete.");

  log("Subscribing...");
  const subscription = await session.subscribe([namespace], trackName);
  log(`Subscribed (alias=${subscription.trackAlias}).`);

  // Set up WebCodecs VideoDecoder (VP8)
  decoder = new VideoDecoder({
    output: (frame) => {
      // Resize canvas to match frame
      if (canvas.width !== frame.displayWidth || canvas.height !== frame.displayHeight) {
        canvas.width = frame.displayWidth;
        canvas.height = frame.displayHeight;
      }
      ctx.drawImage(frame, 0, 0);
      frame.close();
    },
    error: (e) => log(`Decoder error: ${e.message}`),
  });

  decoder.configure({ codec: "vp8" });
  log("Receiving...");

  ($("start-btn") as HTMLButtonElement).disabled = true;
  ($("stop-btn") as HTMLButtonElement).disabled = false;

  receiveLoop();
}

// The currently active group reader. When a new group arrives,
// the old one is abandoned (its remaining objects are discarded).
let activeGroupId = -1;
let activeAbort: AbortController | null = null;

async function receiveLoop() {
  if (!session || !decoder) return;

  try {
    while (session) {
      const event = await session.nextEvent();
      if (event.type !== "dataStream") continue;

      const group = event.reader;

      // Skip if this group is older than the active one
      if (group.groupId <= activeGroupId) continue;

      // Cancel the previous group reader
      if (activeAbort) {
        activeAbort.abort();
      }

      activeGroupId = group.groupId;
      const abort = new AbortController();
      activeAbort = abort;

      // Process this group in the background
      processGroup(group, abort.signal);
    }
  } catch (e) {
    log(`Receive ended: ${e}`);
  }
}

async function processGroup(group: SubgroupReader, signal: AbortSignal) {
  log(`Group ${group.groupId} (alias=${group.trackAlias})`);

  // Flush decoder to clear any frames from previous group
  if (decoder && decoder.state === "configured") {
    await decoder.flush();
  }

  let isFirstObject = true;
  let objectCount = 0;

  try {
    while (!signal.aborted) {
      const payload = await group.readObject();
      if (payload === null) break;

      if (signal.aborted) break;

      if (decoder && decoder.state === "configured") {
        const type = isFirstObject ? "key" : "delta";
        isFirstObject = false;

        const chunk = new EncodedVideoChunk({
          type,
          timestamp: performance.now() * 1000,
          data: payload,
        });

        decoder.decode(chunk);
      }
      objectCount++;
    }
  } catch (e) {
    if (!signal.aborted) {
      log(`Group ${group.groupId} error: ${e}`);
    }
  } finally {
    if (signal.aborted) {
      // New group arrived, cancel this stream (sends STOP_SENDING)
      await group.cancel().catch(() => {});
    }
  }

  if (!signal.aborted) {
    log(`  Group ${group.groupId}: ${objectCount} objects`);
  }
}

async function stop() {
  if (decoder) {
    await decoder.flush();
    decoder.close();
    decoder = null;
  }
  if (session) {
    session.close();
    session = null;
  }
  ($("start-btn") as HTMLButtonElement).disabled = false;
  ($("stop-btn") as HTMLButtonElement).disabled = true;
  log("Stopped.");
}

function log(msg: string) {
  const el = $("log");
  el.textContent += msg + "\n";
  el.scrollTop = el.scrollHeight;
  console.log(msg);
}

document.addEventListener("DOMContentLoaded", () => {
  $("start-btn").addEventListener("click", () => start().catch((e) => log(`ERROR: ${e}`)));
  $("stop-btn").addEventListener("click", () => stop().catch((e) => log(`ERROR: ${e}`)));
});
