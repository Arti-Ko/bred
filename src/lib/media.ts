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
const TRACK_SCREEN_AUDIO = 3;

/** Через сколько молчания дорожка считается погасшей. */
const STALE_AFTER = 1500;
/** Запас буфера звука. Меньше — рвётся на джиттере, больше — слышна задержка. */
const AUDIO_LEAD = 0.06;
/** Предел отставания. Больше — выгоднее пропустить накопившееся, чем тянуть его. */
const AUDIO_MAX_LAG = 0.4;
/** Как часто просить кодировщик выдать ключевой кадр. */
const KEYFRAME_EVERY = 60;

export type TrackKind = 'audio' | 'video' | 'screen' | 'screen-audio';

/**
 * Закрыть кодировщик или декодер, чем бы это ни кончилось.
 *
 * Повторный `close()` бросает исключение, и оно уносит с собой всё, что должно
 * было выполниться дальше. Из-за этого не срабатывала кнопка «выйти» и не
 * включалась камера: остановка захвата падала на середине.
 */
function shut(codec: { state: string; close(): void } | null | undefined): void {
  try {
    if (codec && codec.state !== 'closed') codec.close();
  } catch {
    // Уже закрыт или закрывается — ровно то, чего мы и хотели.
  }
}

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
    track:
      raw[1] === TRACK_SCREEN
        ? 'screen'
        : raw[1] === TRACK_SCREEN_AUDIO
          ? 'screen-audio'
          : raw[1] === TRACK_VIDEO
            ? 'video'
            : 'audio',
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
  #screenAudioStop: (() => void) | null = null;

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
    // Каждый шаг остановки — независимый: падение одного не должно оставлять
    // человека с включённой камерой и в звонке, из которого не выйти.
    for (const stop of this.#stopFns.splice(0)) {
      try {
        stop();
      } catch {
        // Узел уже отключён — не повод бросать остальное.
      }
    }
    shut(this.#audioEncoder);
    shut(this.#videoEncoder);
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
    this.#audioEncoder = await this.#startAudioTrack(track, TRACK_AUDIO, onError, (stop) =>
      this.#stopFns.push(stop),
    );
  }

  /** Кодирование произвольной звуковой дорожки: микрофона или звука экрана. */
  async #startAudioTrack(
    track: MediaStreamTrack,
    kind: number,
    onError: (message: string) => void,
    keepStop: (stop: () => void) => void,
  ): Promise<AudioEncoder> {
    const encoder = new AudioEncoder({
      output: (chunk) => {
        const data = new Uint8Array(chunk.byteLength);
        chunk.copyTo(data);
        void send(pack(kind, true, chunk.timestamp, data));
      },
      error: (error) => onError(`кодировщик звука: ${error.message}`),
    });
    encoder.configure({
      codec: 'opus',
      sampleRate: 48000,
      numberOfChannels: 1,
      bitrate: 32000,
    });
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

      // AudioData держит память вне кучи JavaScript, и сборщик мусора её не
      // освобождает — только явный close(). Без него звук в звонке утекает
      // непрерывно, десятками мегабайт в минуту.
      const frame = new AudioData({
        format: 'f32-planar',
        sampleRate: 48000,
        numberOfFrames: copy.length,
        numberOfChannels: 1,
        timestamp,
        data: copy,
      });
      encoder.encode(frame);
      frame.close();

      timestamp += Math.round((copy.length / 48000) * 1_000_000);
    };

    source.connect(node);
    // Подключение к выходу обязательно, иначе узел не обрабатывает звук;
    // громкость нулевая, чтобы не слышать самого себя.
    const silence = context.createGain();
    silence.gain.value = 0;
    node.connect(silence);
    silence.connect(context.destination);

    keepStop(() => {
      node.disconnect();
      source.disconnect();
      shut(encoder);
      void context.close();
    });
    return encoder;
  }

  /** Есть ли в текущей демонстрации звук. */
  get screenHasAudio(): boolean {
    return (this.#screenStream?.getAudioTracks().length ?? 0) > 0;
  }

  setScreenAudio(enabled: boolean): void {
    this.#screenStream?.getAudioTracks().forEach((track) => (track.enabled = enabled));
  }

  /** Демонстрация экрана — отдельная дорожка, чтобы шла вместе с камерой. */
  async startScreen(onError: (message: string) => void): Promise<void> {
    // Звук просим сразу: система сама решит, отдавать его или нет. На macOS
    // вебвью его не отдаёт, поэтому наличие дорожки проверяем, а не полагаемся.
    const stream = await navigator.mediaDevices.getDisplayMedia({
      video: { frameRate: 12 },
      audio: true,
    });
    this.#screenStream = stream;
    const track = stream.getVideoTracks()[0];
    // Пользователь может остановить показ кнопкой самой системы, не нашей.
    track.addEventListener('ended', () => this.stopScreen());

    this.#screenEncoder = await this.#videoTrackPump(
      track,
      TRACK_SCREEN,
      // Битрейт выше, чем у камеры: на экране читают текст, и его портит
      // не шум, а нехватка бит.
      { width: 1280, height: 720, bitrate: 2_500_000, framerate: 12 },
      onError,
      (stop) => (this.#screenStop = stop),
    );

    const sound = stream.getAudioTracks()[0];
    if (sound) await this.#startAudioTrack(sound, TRACK_SCREEN_AUDIO, onError, (stop) =>
      this.#screenAudioStop = stop,
    );
  }

  stopScreen(): void {
    for (const stop of [this.#screenStop, this.#screenAudioStop]) {
      try {
        stop?.();
      } catch {
        // см. stop(): шаги независимы
      }
    }
    this.#screenStop = null;
    this.#screenAudioStop = null;
    shut(this.#screenEncoder);
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
      // Очередь длиннее двух кадров означает, что кодировщик не успевает.
      // Пропускаем кадр вместо того, чтобы копить и задержку, и память.
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
  /** По регулятору громкости на каждого: в звонке люди звучат по-разному. */
  #gains = new Map<string, GainNode>();
  #volumes = new Map<string, number>();
  /** Когда последний раз приходил кадр по каждой дорожке. */
  #lastFrame = new Map<string, number>();
  #watch: number | null = null;
  #onSpeaker: (author: string) => void;

  constructor(onSpeaker: (author: string) => void) {
    this.#onSpeaker = onSpeaker;
  }

  /**
   * Громкость конкретного собеседника, где 1 — как есть.
   *
   * Ограничиваем сверху: усиление выше четырёх превращает тихого человека не в
   * громкого, а в хрип пополам с шумом микрофона.
   */
  setVolume(author: string, value: number): void {
    const gain = Math.max(0, Math.min(4, value));
    this.#volumes.set(author, gain);
    const node = this.#gains.get(author);
    if (node && this.#context) {
      // Плавно, а не рывком: скачок усиления слышен щелчком.
      node.gain.setTargetAtTime(gain, this.#context.currentTime, 0.02);
    }
  }

  /** Забыть участника: декодеры на ушедших иначе копятся всю встречу. */
  forget(author: string): void {
    shut(this.#audio.get(author));
    this.#audio.delete(author);
    this.#nextPlay.delete(author);
    this.#gains.get(author)?.disconnect();
    this.#gains.delete(author);
    for (const track of ['video', 'screen']) {
      const key = `${track}:${author}`;
      shut(this.#video.get(key));
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

    // Дорожку никто не «закрывает» отдельным сообщением: человек просто
    // перестаёт слать кадры. Без этого сторожа последний кадр висел бы на
    // экране навсегда — и выключенная камера, и снятая демонстрация.
    this.#watch = window.setInterval(() => this.#dropStale(), 700);
  }

  stop(): void {
    if (this.#watch !== null) {
      window.clearInterval(this.#watch);
      this.#watch = null;
    }
    this.#lastFrame.clear();
    for (const decoder of this.#audio.values()) shut(decoder);
    for (const decoder of this.#video.values()) shut(decoder);
    for (const node of this.#gains.values()) node.disconnect();
    this.#audio.clear();
    this.#video.clear();
    this.#gains.clear();
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
    // Звук экрана микшируется с голосом того же человека — это его звук.
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
    this.#lastFrame.set(key, Date.now());
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
    source.connect(this.#gainFor(author, context));
    // Отыгравший источник обязан отцепиться от графа. Иначе за десятиминутный
    // разговор их накапливается под тридцать тысяч на человека, и каждый
    // держит свой буфер.
    source.onended = () => source.disconnect();

    const earliest = context.currentTime + AUDIO_LEAD;
    let at = Math.max(earliest, this.#nextPlay.get(author) ?? 0);

    // Если очередь убежала вперёд (пришла пачка после затыка сети), догонять её
    // бессмысленно: отставание останется навсегда. Лучше выбросить накопленное
    // и продолжить в реальном времени.
    if (at - context.currentTime > AUDIO_MAX_LAG) {
      at = earliest;
    }
    // Кадр, чьё время уже прошло, играть незачем — только память занимать.
    if (at + buffer.duration < context.currentTime) {
      source.disconnect();
      return;
    }

    source.start(at);
    this.#nextPlay.set(author, at + buffer.duration);

    // Грубый индикатор «говорит»: по факту прихода звука, без анализа громкости.
    this.#onSpeaker(author);
  }

  /**
   * Гасит дорожки, по которым давно ничего не приходило.
   *
   * Камера — просто очищаем холст, под ним проступает аватар. Демонстрация —
   * убираем декодер целиком, чтобы плитка с экраном исчезла, а не висела
   * последним кадром.
   */
  #dropStale(): void {
    const now = Date.now();
    for (const [key, at] of this.#lastFrame) {
      if (now - at < STALE_AFTER) continue;
      this.#lastFrame.delete(key);

      const canvas = this.#canvases.get(key);
      canvas?.getContext('2d')?.clearRect(0, 0, canvas.width, canvas.height);

      if (key.startsWith('screen:')) {
        shut(this.#video.get(key));
        this.#video.delete(key);
        this.#canvases.delete(key);
      }
    }
  }

  /** Регулятор громкости участника, создаётся при первом же кадре звука. */
  #gainFor(author: string, context: AudioContext): GainNode {
    let node = this.#gains.get(author);
    if (!node) {
      node = context.createGain();
      node.gain.value = this.#volumes.get(author) ?? 1;
      node.connect(context.destination);
      this.#gains.set(author, node);
    }
    return node;
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
