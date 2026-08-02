// Медиа-конвейер звонка.
//
// Захват и кодирование живут здесь, в вебвью, а не в Rust — и это осознанно:
// getUserMedia отдаёт поток с платформенным эхоподавлением, шумодавом и
// авторегулировкой усиления. Своё AEC на Rust — это месяцы работы и результат
// хуже системного.
//
// Транспорт при этом свой: закодированные куски уходят в ядро и дальше по
// QUIC-датаграммам через iroh. WebRTC не используется намеренно — он потребовал
// бы STUN/TURN, то есть внешние серверы, а NAT нам уже пробил iroh.

import { invoke, Channel } from '@tauri-apps/api/core';

/** Разметка кадра. Должна совпадать с `net::media` в Rust. */
const HEADER = 43;
const VERSION = 1;
const TRACK_AUDIO = 0;
const TRACK_VIDEO = 1;
const TRACK_SCREEN = 2;

/** Запас буфера звука. Меньше — рвётся на джиттере, больше — слышна задержка. */
const AUDIO_LEAD = 0.06;
/** Предел отставания. Больше — выгоднее пропустить накопившееся, чем тянуть его. */
const AUDIO_MAX_LAG = 0.4;
/** Как часто просить кодировщик выдать ключевой кадр. */
const KEYFRAME_EVERY = 60;

export type TrackKind = 'audio' | 'video' | 'screen';

export interface IncomingFrame {
  author: string;
  track: TrackKind;
  keyframe: boolean;
  ts: number;
  data: Uint8Array;
}

/** Чего не хватает платформе для звонков. Пусто — всё на месте. */
export function missingCapabilities(): string[] {
  const gaps: string[] = [];
  if (typeof globalThis.VideoEncoder === 'undefined') gaps.push('VideoEncoder');
  if (typeof globalThis.AudioEncoder === 'undefined') gaps.push('AudioEncoder');
  if (!navigator.mediaDevices?.getUserMedia) gaps.push('getUserMedia');
  return gaps;
}

function idToHex(bytes: Uint8Array): string {
  // Тот же base32, что и в Rust: идентификаторы должны совпадать посимвольно.
  const alphabet = 'abcdefghijklmnopqrstuvwxyz234567';
  let bits = 0;
  let value = 0;
  let out = '';
  for (const byte of bytes) {
    value = (value << 8) | byte;
    bits += 8;
    while (bits >= 5) {
      out += alphabet[(value >>> (bits - 5)) & 31];
      bits -= 5;
    }
  }
  if (bits > 0) out += alphabet[(value << (5 - bits)) & 31];
  return out;
}

function pack(track: number, keyframe: boolean, ts: number, data: Uint8Array): Uint8Array {
  const out = new Uint8Array(HEADER + data.byteLength);
  const view = new DataView(out.buffer);
  out[0] = VERSION;
  out[1] = track;
  out[2] = keyframe ? 1 : 0;
  // Байты автора ядро игнорирует и подставляет свои — подделать нельзя.
  view.setBigInt64(35, BigInt(Math.round(ts)), true);
  out.set(data, HEADER);
  return out;
}

function unpack(raw: Uint8Array): IncomingFrame | null {
  if (raw.byteLength < HEADER || raw[0] !== VERSION) return null;
  const view = new DataView(raw.buffer, raw.byteOffset, raw.byteLength);
  return {
    author: idToHex(raw.subarray(3, 35)),
    track: raw[1] === TRACK_SCREEN ? 'screen' : raw[1] === TRACK_VIDEO ? 'video' : 'audio',
    keyframe: raw[2] !== 0,
    ts: Number(view.getBigInt64(35, true)),
    data: raw.subarray(HEADER),
  };
}

async function send(frame: Uint8Array): Promise<void> {
  // Сырой ArrayBuffer вместо JSON: видео в base64 стоило бы трети трафика.
  await invoke('send_media', frame);
}

// ── исходящий поток ─────────────────────────────────────────────────────────

export interface CaptureOptions {
  video: boolean;
  onError: (message: string) => void;
}

/**
 * Захват микрофона и (по желанию) камеры с кодированием в Opus и VP8.
 */
export class Capture {
  #stream: MediaStream | null = null;
  #audioEncoder: AudioEncoder | null = null;
  #videoEncoder: VideoEncoder | null = null;
  #stopFns: Array<() => void> = [];
  #videoFrames = 0;
  #screenStream: MediaStream | null = null;
  #screenEncoder: VideoEncoder | null = null;
  #screenStop: (() => void) | null = null;

  get stream(): MediaStream | null {
    return this.#stream;
  }

  async start(options: CaptureOptions): Promise<void> {
    this.#stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
      },
      video: options.video ? { width: 640, height: 360, frameRate: 24 } : false,
    });

    await this.#startAudio(options.onError);
    if (options.video) await this.#startVideo(options.onError);
  }

  stop(): void {
    this.stopScreen();
    for (const stop of this.#stopFns.splice(0)) stop();
    this.#audioEncoder?.close();
    this.#videoEncoder?.close();
    this.#audioEncoder = null;
    this.#videoEncoder = null;
    this.#stream?.getTracks().forEach((track) => track.stop());
    this.#stream = null;
  }

  setMuted(muted: boolean): void {
    this.#stream?.getAudioTracks().forEach((track) => (track.enabled = !muted));
  }

  async #startAudio(onError: (message: string) => void): Promise<void> {
    const track = this.#stream?.getAudioTracks()[0];
    if (!track) return;

    const encoder = new AudioEncoder({
      output: (chunk) => {
        const data = new Uint8Array(chunk.byteLength);
        chunk.copyTo(data);
        void send(pack(TRACK_AUDIO, true, chunk.timestamp, data));
      },
      error: (error) => onError(`кодировщик звука: ${error.message}`),
    });
    encoder.configure({
      codec: 'opus',
      sampleRate: 48000,
      numberOfChannels: 1,
      bitrate: 32000,
    });
    this.#audioEncoder = encoder;

    // Путь через WebAudio, а не MediaStreamTrackProcessor: последний есть
    // только в Chromium, а окно на macOS — WKWebView.
    const context = new AudioContext({ sampleRate: 48000 });
    // Без явного возобновления контекст может остаться приостановленным,
    // и захват молча не даст ни одного кадра.
    if (context.state === 'suspended') await context.resume();
    const source = context.createMediaStreamSource(new MediaStream([track]));
    const node = context.createScriptProcessor(2048, 1, 1);
    let timestamp = 0;

    node.onaudioprocess = (event) => {
      if (encoder.state !== 'configured') return;
      const channel = event.inputBuffer.getChannelData(0);
      const copy = new Float32Array(channel.length);
      copy.set(channel);
      encoder.encode(
        new AudioData({
          format: 'f32-planar',
          sampleRate: 48000,
          numberOfFrames: copy.length,
          numberOfChannels: 1,
          timestamp,
          data: copy,
        }),
      );
      timestamp += Math.round((copy.length / 48000) * 1_000_000);
    };

    source.connect(node);
    // Подключение к выходу обязательно, иначе узел не обрабатывает звук;
    // громкость нулевая, чтобы не слышать самого себя.
    const silence = context.createGain();
    silence.gain.value = 0;
    node.connect(silence);
    silence.connect(context.destination);

    this.#stopFns.push(() => {
      node.disconnect();
      source.disconnect();
      void context.close();
    });
  }

  /** Демонстрация экрана — отдельная дорожка, чтобы шла вместе с камерой. */
  async startScreen(onError: (message: string) => void): Promise<void> {
    const stream = await navigator.mediaDevices.getDisplayMedia({
      video: { frameRate: 15 },
      audio: false,
    });
    this.#screenStream = stream;
    const track = stream.getVideoTracks()[0];
    // Пользователь может остановить показ кнопкой самой системы, не нашей.
    track.addEventListener('ended', () => this.stopScreen());

    this.#screenEncoder = await this.#videoTrackPump(
      track,
      TRACK_SCREEN,
      { width: 1280, height: 720, bitrate: 1_200_000, framerate: 15 },
      onError,
      (stop) => (this.#screenStop = stop),
    );
  }

  stopScreen(): void {
    this.#screenStop?.();
    this.#screenStop = null;
    this.#screenEncoder?.close();
    this.#screenEncoder = null;
    this.#screenStream?.getTracks().forEach((t) => t.stop());
    this.#screenStream = null;
  }

  get sharingScreen(): boolean {
    return this.#screenStream !== null;
  }

  async #startVideo(onError: (message: string) => void): Promise<void> {
    const track = this.#stream?.getVideoTracks()[0];
    if (!track) return;

    const encoder = new VideoEncoder({
      output: (chunk) => {
        const data = new Uint8Array(chunk.byteLength);
        chunk.copyTo(data);
        void send(pack(TRACK_VIDEO, chunk.type === 'key', chunk.timestamp, data));
      },
      error: (error) => onError(`кодировщик видео: ${error.message}`),
    });
    encoder.configure({
      codec: 'vp8',
      width: 640,
      height: 360,
      bitrate: 500_000,
      framerate: 24,
      latencyMode: 'realtime',
    });
    this.#videoEncoder = encoder;

    const video = document.createElement('video');
    video.srcObject = new MediaStream([track]);
    video.muted = true;
    await video.play();

    let running = true;
    const pump = () => {
      if (!running || encoder.state !== 'configured') return;
      // Очередь длиннее двух кадров означает, что мы не успеваем: пропускаем
      // кадр вместо того, чтобы копить задержку.
      if (encoder.encodeQueueSize < 2) {
        const frame = new VideoFrame(video, { timestamp: performance.now() * 1000 });
        this.#videoFrames += 1;
        encoder.encode(frame, { keyFrame: this.#videoFrames % KEYFRAME_EVERY === 1 });
        frame.close();
      }
      requestAnimationFrame(pump);
    };
    requestAnimationFrame(pump);

    this.#stopFns.push(() => {
      running = false;
      video.srcObject = null;
    });
  }

  /**
   * Кодирование произвольной видеодорожки. Общий код для камеры и экрана:
   * различаются только разрешение, битрейт и номер дорожки.
   */
  async #videoTrackPump(
    track: MediaStreamTrack,
    kind: number,
    config: { width: number; height: number; bitrate: number; framerate: number },
    onError: (message: string) => void,
    keepStop: (stop: () => void) => void,
  ): Promise<VideoEncoder> {
    let frames = 0;
    const encoder = new VideoEncoder({
      output: (chunk) => {
        const data = new Uint8Array(chunk.byteLength);
        chunk.copyTo(data);
        void send(pack(kind, chunk.type === 'key', chunk.timestamp, data));
      },
      error: (error) => onError(`кодировщик экрана: ${error.message}`),
    });
    encoder.configure({ codec: 'vp8', ...config, latencyMode: 'realtime' });

    const video = document.createElement('video');
    video.srcObject = new MediaStream([track]);
    video.muted = true;
    await video.play();

    let running = true;
    const pump = () => {
      if (!running || encoder.state !== 'configured') return;
      if (encoder.encodeQueueSize < 2) {
        const frame = new VideoFrame(video, { timestamp: performance.now() * 1000 });
        frames += 1;
        encoder.encode(frame, { keyFrame: frames % KEYFRAME_EVERY === 1 });
        frame.close();
      }
      requestAnimationFrame(pump);
    };
    requestAnimationFrame(pump);

    keepStop(() => {
      running = false;
      video.srcObject = null;
    });
    return encoder;
  }
}

// ── входящий поток ──────────────────────────────────────────────────────────

/**
 * Приём, декодирование и воспроизведение чужих потоков.
 * По декодеру на дорожку каждого участника.
 */
export class Playback {
  #audio = new Map<string, AudioDecoder>();
  #video = new Map<string, VideoDecoder>();
  #canvases = new Map<string, HTMLCanvasElement>();
  #context: AudioContext | null = null;
  #nextPlay = new Map<string, number>();
  #onSpeaker: (author: string) => void;

  constructor(onSpeaker: (author: string) => void) {
    this.#onSpeaker = onSpeaker;
  }

  /** Забыть участника: декодеры на ушедших иначе копятся всю встречу. */
  forget(author: string): void {
    this.#audio.get(author)?.close();
    this.#audio.delete(author);
    this.#nextPlay.delete(author);
    for (const track of ['video', 'screen']) {
      const key = `${track}:${author}`;
      this.#video.get(key)?.close();
      this.#video.delete(key);
      this.#canvases.delete(key);
    }
  }

  /** Куда рисовать поток участника. Камера и экран — разные холсты. */
  attachCanvas(author: string, canvas: HTMLCanvasElement | null, track: TrackKind = 'video'): void {
    const key = `${track}:${author}`;
    if (canvas) this.#canvases.set(key, canvas);
    else this.#canvases.delete(key);
  }

  /** Подписка на поток кадров из ядра. */
  async listen(onError: (message: string) => void): Promise<void> {
    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (payload) => {
      const frame = unpack(new Uint8Array(payload));
      if (frame) this.#handle(frame, onError);
    };
    await invoke('media_stream', { channel });
  }

  stop(): void {
    for (const decoder of this.#audio.values()) decoder.close();
    for (const decoder of this.#video.values()) decoder.close();
    this.#audio.clear();
    this.#video.clear();
    this.#nextPlay.clear();
    void this.#context?.close();
    this.#context = null;
  }

  #handle(frame: IncomingFrame, onError: (message: string) => void): void {
    if (frame.track === 'audio') this.#handleAudio(frame, onError);
    else this.#handleVideo(frame, onError);
  }

  /** Кто из участников сейчас показывает экран. */
  sharingScreen(): string[] {
    return [...this.#video.keys()]
      .filter((key) => key.startsWith('screen:'))
      .map((key) => key.slice('screen:'.length));
  }

  #handleAudio(frame: IncomingFrame, onError: (message: string) => void): void {
    let decoder = this.#audio.get(frame.author);
    if (!decoder) {
      decoder = new AudioDecoder({
        output: (data) => this.#play(frame.author, data),
        error: (error) => onError(`декодер звука: ${error.message}`),
      });
      decoder.configure({ codec: 'opus', sampleRate: 48000, numberOfChannels: 1 });
      this.#audio.set(frame.author, decoder);
    }
    if (decoder.state !== 'configured') return;
    decoder.decode(
      new EncodedAudioChunk({
        type: 'key',
        timestamp: frame.ts,
        data: frame.data,
      }),
    );
  }

  #handleVideo(frame: IncomingFrame, onError: (message: string) => void): void {
    // Ключ с дорожкой: у одного участника камера и экран идут одновременно.
    const key = `${frame.track}:${frame.author}`;
    let decoder = this.#video.get(key);
    if (!decoder) {
      // До первого ключевого кадра декодер запускать бессмысленно.
      if (!frame.keyframe) return;
      decoder = new VideoDecoder({
        output: (image) => this.#draw(key, image),
        error: (error) => onError(`декодер видео: ${error.message}`),
      });
      decoder.configure({ codec: 'vp8', optimizeForLatency: true });
      this.#video.set(key, decoder);
    }
    if (decoder.state !== 'configured') return;
    decoder.decode(
      new EncodedVideoChunk({
        type: frame.keyframe ? 'key' : 'delta',
        timestamp: frame.ts,
        data: frame.data,
      }),
    );
  }

  /** Планирование звука с небольшим запасом — иначе рвётся на джиттере сети. */
  #play(author: string, data: AudioData): void {
    this.#context ??= new AudioContext({ sampleRate: 48000 });
    const context = this.#context;
    if (context.state === 'suspended') void context.resume();

    const frames = data.numberOfFrames;
    const buffer = context.createBuffer(1, frames, data.sampleRate);
    const channel = new Float32Array(frames);
    data.copyTo(channel, { planeIndex: 0, format: 'f32-planar' });
    buffer.copyToChannel(channel, 0);
    data.close();

    const source = context.createBufferSource();
    source.buffer = buffer;
    source.connect(context.destination);

    const earliest = context.currentTime + AUDIO_LEAD;
    let at = Math.max(earliest, this.#nextPlay.get(author) ?? 0);

    // Если очередь убежала вперёд (пришла пачка после затыка сети), догонять её
    // бессмысленно: отставание останется навсегда. Лучше выбросить накопленное
    // и продолжить в реальном времени.
    if (at - context.currentTime > AUDIO_MAX_LAG) {
      at = earliest;
    }

    source.start(at);
    this.#nextPlay.set(author, at + buffer.duration);

    // Грубый индикатор «говорит»: по факту прихода звука, без анализа громкости.
    this.#onSpeaker(author);
  }

  #draw(key: string, image: VideoFrame): void {
    const canvas = this.#canvases.get(key);
    if (!canvas) {
      image.close();
      return;
    }
    if (canvas.width !== image.displayWidth) canvas.width = image.displayWidth;
    if (canvas.height !== image.displayHeight) canvas.height = image.displayHeight;
    canvas.getContext('2d')?.drawImage(image, 0, 0);
    image.close();
  }
}
