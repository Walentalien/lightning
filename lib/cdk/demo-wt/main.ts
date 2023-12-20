import * as schema from "../handshake/schema.ts";
import { FleekTransport } from "../handshake/transports/webtransport.ts";

const video = document.querySelector("video")!;

document.getElementById("start")!.onclick = async () => {
  await startSession();
};

// Temporary internal hash for big buck bunny
const bbb_blake3 = new Uint8Array([
  16,
  101,
  178,
  253,
  130,
  145,
  238,
  45,
  55,
  180,
  144,
  250,
  71,
  121,
  27,
  31,
  201,
  144,
  67,
  224,
  179,
  36,
  52,
  86,
  242,
  33,
  164,
  55,
  27,
  140,
  43,
  209,
]);

const transport = new FleekTransport("127.0.0.1");
let sourceBuffer: SourceBuffer | undefined;
const queue: Uint8Array[] = [];

function appendBuffer(buffer: Uint8Array) {
  if (sourceBuffer!.updating || queue.length != 0) {
    queue.push(buffer);
  } else {
    sourceBuffer!.appendBuffer(buffer);
  }
}

// Todo: Handle errors and log.
const startSession = async () => {
  // connect and handshake
  const stream = await transport.connect();
  await stream.handshakePrimary(0);

  // Send a request for the CID.
  const buffer = new Uint8Array(33);
  buffer[0] = 0; // Blake3 Origin
  buffer.set(bbb_blake3, 1); // UID
  await stream.send({
    tag: schema.Request.Tag.ServicePayload,
    bytes: buffer,
  });

  // Read the number of blocks we should receive back from the first frame.
  const frame = await stream.recv();
  if (!frame || frame.tag !== schema.Response.Tag.ServicePayload) {
    console.error("invalid frame: ", frame);
    return;
  }
  const view = new DataView(frame.bytes.buffer);
  const blockCount = view.getUint32(0, false);

  // Read each block from the stream
  for (let i = 0; i < blockCount; i++) {
    const frame = await stream.recv();
    if (!frame || frame.tag !== schema.Response.Tag.ServicePayload) {
      console.error("invalid tag");
      return;
    }

    appendBuffer(frame.bytes);
  }

  transport.close();
};

// --------------------------------------------
// MediaSource
// Need to be specific for Blink regarding codecs
// ./mp4info frag_bunny.mp4 | grep Codec
const mimeCodec = 'video/mp4; codecs="avc1.42E01E, mp4a.40.2"';

if ("MediaSource" in window && MediaSource.isTypeSupported(mimeCodec)) {
  const mediaSource = new MediaSource();
  video.src = URL.createObjectURL(mediaSource);
  mediaSource.addEventListener("sourceopen", sourceOpen);
} else {
  console.error("Unsupported MIME type or codec: ", mimeCodec);
}

function sourceOpen(this: MediaSource) {
  console.log(this.readyState); // open
  sourceBuffer = this.addSourceBuffer(mimeCodec);

  sourceBuffer!.addEventListener("updateend", function(_) {
    if (queue.length != 0) {
      sourceBuffer!.appendBuffer(queue.shift()!);
    }
  });
}
