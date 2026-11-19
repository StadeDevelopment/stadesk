const STREAM_START = 0;
const STREAM_PACKET = 1;
const VP9_LEVELS = ["41", "51", "61", "10"];

export type StreamMessage =
  | { kind: "start"; width: number; height: number }
  | { kind: "packet"; keyframe: boolean; timestampMs: number; data: Uint8Array };

export function parseStreamMessage(buffer: ArrayBuffer): StreamMessage | null {
  const view = new DataView(buffer);
  if (view.byteLength < 1) {
    return null;
  }
  const kind = view.getUint8(0);
  if (kind === STREAM_START && view.byteLength === 9) {
    return { kind: "start", width: view.getUint32(1), height: view.getUint32(5) };
  }
  if (kind === STREAM_PACKET && view.byteLength >= 10) {
    const keyframe = view.getUint8(1) === 1;
    const timestampMs = Number(view.getBigInt64(2));
    return { kind: "packet", keyframe, timestampMs, data: new Uint8Array(buffer, 10) };
  }
  return null;
}

export type PlayerStatus = "waiting" | "playing" | "unsupported";

async function supportedConfig(width: number, height: number): Promise<VideoDecoderConfig | null> {
  if (typeof VideoDecoder === "undefined") {
    return null;
  }
  for (const level of VP9_LEVELS) {
    const config: VideoDecoderConfig = {
      codec: `vp09.00.${level}.08`,
      codedWidth: width,
      codedHeight: height,
      optimizeForLatency: true,
    };
    try {
      const result = await VideoDecoder.isConfigSupported(config);
      if (result.supported) {
        return config;
      }
    } catch {
      continue;
    }
  }
  return null;
}

export class StreamPlayer {
  private decoder: VideoDecoder | null = null;
  private config: VideoDecoderConfig | null = null;
  private needsKeyframe = true;
  private generation = 0;

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly onStatus: (status: PlayerStatus) => void,
    private readonly onNeedKeyframe: () => void,
  ) {}

  async start(width: number, height: number): Promise<void> {
    const generation = ++this.generation;
    this.close();
    const config = await supportedConfig(width, height);
    if (generation !== this.generation) {
      return;
    }
    if (!config) {
      this.onStatus("unsupported");
      return;
    }
    this.canvas.width = width;
    this.canvas.height = height;
    this.config = config;
    this.createDecoder();
  }

  private createDecoder(): void {
    if (!this.config) {
      return;
    }
    const context = this.canvas.getContext("2d");
    this.decoder = new VideoDecoder({
      output: (frame) => {
        context?.drawImage(frame, 0, 0, this.canvas.width, this.canvas.height);
        frame.close();
        this.onStatus("playing");
      },
      error: () => {
        this.decoder = null;
        this.needsKeyframe = true;
        this.onNeedKeyframe();
      },
    });
    this.decoder.configure(this.config);
    this.needsKeyframe = true;
  }

  packet(keyframe: boolean, timestampMs: number, data: Uint8Array): void {
    if (!this.config) {
      return;
    }
    if (!this.decoder || this.decoder.state === "closed") {
      this.createDecoder();
    }
    if (this.needsKeyframe && !keyframe) {
      return;
    }
    this.needsKeyframe = false;
    try {
      this.decoder?.decode(
        new EncodedVideoChunk({
          type: keyframe ? "key" : "delta",
          timestamp: timestampMs * 1000,
          data,
        }),
      );
    } catch {
      this.needsKeyframe = true;
      this.onNeedKeyframe();
    }
  }

  close(): void {
    if (this.decoder && this.decoder.state !== "closed") {
      this.decoder.close();
    }
    this.decoder = null;
    this.config = null;
  }
}
