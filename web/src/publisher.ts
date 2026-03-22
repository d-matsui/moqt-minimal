// MOQT Browser Publisher
// Captures video via getUserMedia, encodes with WebCodecs VP8,
// and sends as MOQT objects via WebTransport.

import { MoqtSession, SubgroupWriter } from "./session.js";

const $ = (id: string) => document.getElementById(id)!;

let session: MoqtSession | null = null;
let encoder: VideoEncoder | null = null;
let currentGroup: SubgroupWriter | null = null;
let groupId = 0;
let streamCount = 0;
let groupStarted = false;
let mediaStream: MediaStream | null = null;

async function start() {
  const url = ($ ("relay-url") as HTMLInputElement).value;
  const namespace = ($ ("namespace") as HTMLInputElement).value;
  const trackName = ($ ("track-name") as HTMLInputElement).value;

  log("Connecting...");
  session = await MoqtSession.connect(url);
  log("Connected. SETUP complete.");

  log("Registering namespace...");
  await session.publishNamespace([namespace]);
  log("PUBLISH_NAMESPACE registered.");

  log("Waiting for SUBSCRIBE...");
  const event = await session.nextEvent();
  if (event.type !== "subscribe") {
    log("ERROR: expected SUBSCRIBE");
    return;
  }
  log(`Received SUBSCRIBE for: ${new TextDecoder().decode(event.request.message.trackName)}`);

  await event.request.accept(1);
  log("Sent SUBSCRIBE_OK (alias=1).");

  // Start capture
  mediaStream = await navigator.mediaDevices.getUserMedia({
    video: { width: 640, height: 480, frameRate: 30 },
    audio: false,
  });

  const videoTrack = mediaStream.getVideoTracks()[0];
  const settings = videoTrack.getSettings();
  const preview = $("preview") as HTMLVideoElement;
  preview.srcObject = mediaStream;

  // Set up WebCodecs VideoEncoder (VP8)
  encoder = new VideoEncoder({
    output: async (chunk) => {
      await handleEncodedChunk(chunk);
    },
    error: (e) => log(`Encoder error: ${e.message}`),
  });

  encoder.configure({
    codec: "vp8",
    width: settings.width || 640,
    height: settings.height || 480,
    framerate: 30,
    bitrate: 1_000_000,
  });

  // Read frames from video track
  const processor = new MediaStreamTrackProcessor({ track: videoTrack });
  const frameReader = processor.readable.getReader();

  readFrames(frameReader);
  ($("start-btn") as HTMLButtonElement).disabled = true;
  ($("stop-btn") as HTMLButtonElement).disabled = false;
  log("Publishing...");
}

async function readFrames(
  reader: ReadableStreamDefaultReader<VideoFrame>
) {
  while (true) {
    const { value: frame, done } = await reader.read();
    if (done || !encoder) break;
    if (encoder.encodeQueueSize > 3) {
      frame.close();
      continue;
    }
    const isKeyframe = groupId === 0 && !groupStarted;
    encoder.encode(frame, { keyFrame: isKeyframe || groupId % 30 === 0 });
    frame.close();
  }
}

async function handleEncodedChunk(chunk: EncodedVideoChunk) {
  if (!session) return;

  // Keyframe starts a new group
  if (chunk.type === "key" && groupStarted) {
    if (currentGroup) {
      await currentGroup.finish();
      streamCount++;
      log(`Sent group ${groupId}`);
    }
    groupId++;
  }

  // Open new subgroup if needed
  if (!currentGroup || (chunk.type === "key" && groupStarted)) {
    currentGroup = await session.openSubgroup(1, groupId, 0);
    groupStarted = true;
  }

  // Write frame as one MOQT object
  const data = new Uint8Array(chunk.byteLength);
  chunk.copyTo(data);
  await currentGroup.writeObject(data);
}

async function stop() {
  if (encoder) {
    await encoder.flush();
    encoder.close();
    encoder = null;
  }

  if (currentGroup) {
    await currentGroup.finish();
    streamCount++;
    log(`Sent group ${groupId}`);
  }

  if (mediaStream) {
    mediaStream.getTracks().forEach((t) => t.stop());
    mediaStream = null;
  }

  if (session) {
    // TODO: send PUBLISH_DONE via the subscribe request bidi stream
    session.close();
    session = null;
  }

  ($("start-btn") as HTMLButtonElement).disabled = false;
  ($("stop-btn") as HTMLButtonElement).disabled = true;
  log(`Stopped. Sent ${streamCount} groups.`);
}

function log(msg: string) {
  const el = $("log");
  el.textContent += msg + "\n";
  el.scrollTop = el.scrollHeight;
  console.log(msg);
}

// Wire up buttons
document.addEventListener("DOMContentLoaded", () => {
  $("start-btn").addEventListener("click", () => start().catch((e) => log(`ERROR: ${e}`)));
  $("stop-btn").addEventListener("click", () => stop().catch((e) => log(`ERROR: ${e}`)));
});
