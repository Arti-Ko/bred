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
//
// Битрейт задаёт ядро: оно одно видит, сколько кадров не влезло в исходящий
// канал и на скольких собеседников этот канал делится.

import { invoke, Channel } from '@tauri-apps/api/core';

/** Разметка кадра. Должна совпадать с `net::media` в Rust. */
const HEADER = 47;
const VERSION = 2;
const TRACK_AUDIO = 0;
const TRACK_VIDEO = 1;
const TRACK_SCREEN = 2;
const TRACK_SCREEN_AUDIO = 3;

/** Через сколько молчания дорожка считается погасшей. */
const STALE_AFTER = 1500;

/**
 * Границы запаса буфера звука.
 *
 * Раньше запас был один на всех и намертво: 60 мс. На ровном канале это лишняя
 * задержка в разговоре, на дрожащем — недобор, из-за которого звук рвётся.
 * Теперь запас считается по реальному разбросу времени прихода.
 */
const AUDIO_LEAD_MIN = 0.04;
const AUDIO_LEAD_MAX = 0.2;
/** Предел отставания. Больше — выгоднее пропустить накопившееся, чем тянуть его. */
const AUDIO_MAX_LAG = 0.4;
/** Родной размер кадра Opus при 48 кГц — двадцать миллисекунд. */
const AUDIO_BLOCK = 960;
/**
 * Как часто кодировщик обязан выдать ключевой кадр сам по себе.
 *
 * Раньше стояло 60 (2.5 секунды) — и это был единственный способ его получить,
 * поэтому новый участник столько и смотрел в чёрный прямоугольник. Теперь ядро
 * просит ключевой кадр при появлении собеседника, и расписание нужно только как
 * страховка: реже — значит дешевле.
 */
const KEYFRAME_EVERY = 90;
/** Модуль захвата звука. Лежит в `public/`, отдаётся со своего origin. */
const CAPTURE_WORKLET = '/audio-capture-worklet.js';

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
  seq: number;
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
  // Номер кадра тоже назначает ядро: у него один счётчик на всех собеседников.
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
    seq: view.getUint32(43, true),
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

interface VideoConfig {
  width: number;
  height: number;
  bitrate: number;
  framerate: number;
}

/** Стартовые настройки дорожек картинки. Дальше их двигает governor в ядре. */
const CAMERA: VideoConfig = { width: 640, height: 360, bitrate: 500_000, framerate: 24 };
// Битрейт выше, чем у камеры: на экране читают текст, и его портит не шум,
// а нехватка бит.
const SCREEN: VideoConfig = { width: 1280, height: 720, bitrate: 2_500_000, framerate: 12 };

/** Кодировщик картинки вместе со всем, что нужно, чтобы его перенастроить. */
interface VideoLane {
  encoder: VideoEncoder;
  config: VideoConfig;
  /** Выдать ключевой кадр при ближайшей возможности. */
  forced: boolean;
}

/**
 * Захват микрофона и (по желанию) камеры с кодированием в Opus и VP8.
 */
export class Capture {
  #stream: MediaStream | null = null;
  #audioEncoder: AudioEncoder | null = null;
  #stopFns: Array<() => void> = [];
  #screenStream: MediaStream | null = null;
  #screenStop: (() => void) | null = null;
  #screenAudioStop: (() => void) | null = null;
  /** Дорожки картинки по виду: камера и демонстрация настраиваются порознь. */
  #lanes = new Map<TrackKind, VideoLane>();

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
    if (options.video) {
      const track = this.#stream?.getVideoTracks()[0];
      if (track) {
        await this.#videoTrackPump(track, 'video', TRACK_VIDEO, CAMERA, options.onError, (stop) =>
          this.#stopFns.push(stop),
        );
      }
    }
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
    this.#audioEncoder = null;
    for (const lane of this.#lanes.values()) shut(lane.encoder);
    this.#lanes.clear();
    this.#stream?.getTracks().forEach((track) => track.stop());
    this.#stream = null;
  }

  setMuted(muted: boolean): void {
    this.#stream?.getAudioTracks().forEach((track) => (track.enabled = !muted));
  }

  /**
   * Выдать ключевой кадр сейчас, не дожидаясь расписания.
   *
   * Ядро просит об этом, когда в звонке появился новый собеседник: его декодер
   * не начнёт работу, пока не увидит ключевой кадр, и до него человек смотрит на
   * чёрный прямоугольник.
   */
  forceKeyframe(): void {
    for (const lane of this.#lanes.values()) lane.forced = true;
  }

  /**
   * Сменить битрейт дорожки на лету.
   *
   * Величину считает ядро: только оно видит, сколько кадров не влезло в
   * исходящий канал, и на скольких собеседников этот канал делится. Здесь —
   * только исполнение.
   */
  setBitrate(track: TrackKind, bps: number): void {
    const lane = this.#lanes.get(track);
    if (!lane || lane.encoder.state !== 'configured') return;
    // Мелкие подвижки игнорируем: каждая перенастройка стоит ключевого кадра.
    if (Math.abs(lane.config.bitrate - bps) < lane.config.bitrate * 0.05) return;

    lane.config = { ...lane.config, bitrate: bps };
    try {
      lane.encoder.configure({ codec: 'vp8', ...lane.config, latencyMode: 'realtime' });
      // После перенастройки нужен ключевой кадр: иначе собеседник ещё несколько
      // секунд декодирует дельты, рассчитанные на прежний битрейт.
      lane.forced = true;
    } catch {
      // Платформа не умеет менять настройки на ходу — продолжаем на прежнем
      // битрейте. Это хуже, но не смертельно.
    }
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

    const context = new AudioContext({ sampleRate: 48000 });
    // Без явного возобновления контекст может остаться приостановленным,
    // и захват молча не даст ни одного кадра.
    if (context.state === 'suspended') await context.resume();
    const source = context.createMediaStreamSource(new MediaStream([track]));

    let timestamp = 0;
    // Именно `Float32Array<ArrayBuffer>`, а не просто `Float32Array`: с версии
    // 5.7 TypeScript различает, чем подложен типизированный массив, и
    // `AudioData` не принимает тот, за которым может стоять `SharedArrayBuffer`.
    const push = (samples: Float32Array<ArrayBuffer>) => {
      if (encoder.state !== 'configured') return;
      // AudioData держит память вне кучи JavaScript, и сборщик мусора её не
      // освобождает — только явный close(). Без него звук в звонке утекает
      // непрерывно, десятками мегабайт в минуту.
      const frame = new AudioData({
        format: 'f32-planar',
        sampleRate: 48000,
        numberOfFrames: samples.length,
        numberOfChannels: 1,
        timestamp,
        data: samples,
      });
      encoder.encode(frame);
      frame.close();
      timestamp += Math.round((samples.length / 48000) * 1_000_000);
    };

    const stop = await this.#pumpAudio(context, source, push);

    keepStop(() => {
      stop();
      source.disconnect();
      shut(encoder);
      void context.close();
    });
    return encoder;
  }

  /**
   * Качать звук из графа наружу. Сначала пробуем AudioWorklet — он живёт в
   * потоке аудиорендера, где перерисовка интерфейса ему не мешает.
   *
   * Запасной путь через ScriptProcessorNode оставлен намеренно: узел устарел и
   * работает в главном потоке, но если платформа не отдаст воркер, звонок
   * должен остаться со звуком, а не остаться без него.
   */
  async #pumpAudio(
    context: AudioContext,
    source: MediaStreamAudioSourceNode,
    push: (samples: Float32Array<ArrayBuffer>) => void,
  ): Promise<() => void> {
    // Подключение к выходу обязательно, иначе граф не тянет звук через узел;
    // громкость нулевая, чтобы не слышать самого себя.
    const silence = context.createGain();
    silence.gain.value = 0;
    silence.connect(context.destination);

    try {
      await context.audioWorklet.addModule(CAPTURE_WORKLET);
      const node = new AudioWorkletNode(context, 'bred-capture', {
        numberOfInputs: 1,
        numberOfOutputs: 1,
        outputChannelCount: [1],
        processorOptions: { blockSize: AUDIO_BLOCK },
      });
      // Воркер передаёт владение буфером, поэтому за массивом стоит обычный
      // `ArrayBuffer`, а не разделяемый.
      node.port.onmessage = (event: MessageEvent<Float32Array<ArrayBuffer>>) =>
        push(event.data);
      source.connect(node);
      node.connect(silence);
      return () => {
        node.port.onmessage = null;
        node.disconnect();
        silence.disconnect();
      };
    } catch {
      // Воркер не поднялся — идём старым путём, но со звуком.
      const node = context.createScriptProcessor(2048, 1, 1);
      node.onaudioprocess = (event) => {
        const channel = event.inputBuffer.getChannelData(0);
        const copy = new Float32Array(channel.length);
        copy.set(channel);
        push(copy);
      };
      source.connect(node);
      node.connect(silence);
      return () => {
        node.onaudioprocess = null;
        node.disconnect();
        silence.disconnect();
      };
    }
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

    await this.#videoTrackPump(
      track,
      'screen',
      TRACK_SCREEN,
      SCREEN,
      onError,
      (stop) => (this.#screenStop = stop),
    );

    const sound = stream.getAudioTracks()[0];
    if (sound) {
      await this.#startAudioTrack(
        sound,
        TRACK_SCREEN_AUDIO,
        onError,
        (stop) => (this.#screenAudioStop = stop),
      );
    }
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
    shut(this.#lanes.get('screen')?.encoder);
    this.#lanes.delete('screen');
    this.#screenStream?.getTracks().forEach((t) => t.stop());
    this.#screenStream = null;
  }

  get sharingScreen(): boolean {
    return this.#screenStream !== null;
  }

  /**
   * Кодирование произвольной видеодорожки. Общий код для камеры и экрана:
   * различаются только разрешение, битрейт и номер дорожки.
   */
  async #videoTrackPump(
    track: MediaStreamTrack,
    lane: TrackKind,
    kind: number,
    config: VideoConfig,
    onError: (message: string) => void,
    keepStop: (stop: () => void) => void,
  ): Promise<void> {
    let frames = 0;
    const encoder = new VideoEncoder({
      output: (chunk) => {
        const data = new Uint8Array(chunk.byteLength);
        chunk.copyTo(data);
        void send(pack(kind, chunk.type === 'key', chunk.timestamp, data));
      },
      error: (error) => onError(`кодировщик картинки: ${error.message}`),
    });
    encoder.configure({ codec: 'vp8', ...config, latencyMode: 'realtime' });

    const entry: VideoLane = { encoder, config, forced: true };
    this.#lanes.set(lane, entry);

    const video = document.createElement('video');
    video.srcObject = new MediaStream([track]);
    video.muted = true;
    await video.play();

    let running = true;
    const pump = () => {
      if (!running || encoder.state !== 'configured') return;
      // Очередь длиннее двух кадров означает, что кодировщик не успевает.
      // Пропускаем кадр вместо того, чтобы копить и задержку, и память.
      if (encoder.encodeQueueSize < 2) {
        const frame = new VideoFrame(video, { timestamp: performance.now() * 1000 });
        frames += 1;
        const keyFrame = entry.forced || frames % KEYFRAME_EVERY === 1;
        entry.forced = false;
        encoder.encode(frame, { keyFrame });
        frame.close();
      }
      schedule();
    };
    // Пока окно на виду — по кадрам отрисовки. Когда свёрнуто — по таймеру:
    // requestAnimationFrame в скрытом окне не вызывается вовсе, и собеседник
    // видел, будто камеру выключили.
    const schedule = () => {
      if (!running) return;
      if (document.hidden) window.setTimeout(pump, 1000 / 12);
      else requestAnimationFrame(pump);
    };
    schedule();

    keepStop(() => {
      running = false;
      video.srcObject = null;
    });
  }
}

// ── входящий поток ──────────────────────────────────────────────────────────

/**
 * Подбор запаса буфера под реальный разброс времени прихода.
 *
 * Фиксированные 60 мс были компромиссом, который не подходил никому: на ровном
 * канале это лишняя задержка в разговоре, на дрожащем — недобор, из-за которого
 * звук рвётся. Разброс считаем так же, как RFC 3550 считает джиттер, и держим
 * запас чуть выше него.
 */
class Jitter {
  #lead = AUDIO_LEAD_MIN;
  #spread = 0;
  #last = 0;

  /** Отметить приход кадра длительностью `frameMs`. */
  observe(frameMs: number): void {
    const now = performance.now();
    if (this.#last > 0) {
      const deviation = Math.abs(now - this.#last - frameMs);
      // Скользящее среднее: одиночный выброс не должен раздувать буфер.
      this.#spread += (deviation - this.#spread) / 16;
    }
    this.#last = now;
  }

  /** Не дождались кадра вовремя — запас заведомо мал, поднимаем сразу. */
  underrun(): void {
    this.#lead = Math.min(AUDIO_LEAD_MAX, this.#lead * 1.5 + 0.01);
  }

  get lead(): number {
    const target = Math.min(
      AUDIO_LEAD_MAX,
      Math.max(AUDIO_LEAD_MIN, (this.#spread * 2.5 + 20) / 1000),
    );
    // Вверх — сразу: недобор слышно немедленно. Вниз — медленно: поспешное
    // снижение возвращает бульканье, ради избавления от которого всё и делалось.
    this.#lead = target > this.#lead ? target : this.#lead + (target - this.#lead) * 0.05;
    return this.#lead;
  }
}

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
  /** Подбор буфера и учёт потерь — на каждого собеседника свой. */
  #jitter = new Map<string, Jitter>();
  #lastSeq = new Map<string, number>();
  #lost = 0;
  #watch: number | null = null;
  #onSpeaker: (author: string) => void;

  constructor(onSpeaker: (author: string) => void) {
    this.#onSpeaker = onSpeaker;
  }

  /** Сколько кадров звука недосчитались — грубая мера качества канала. */
  get lostFrames(): number {
    return this.#lost;
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
    this.#jitter.delete(author);
    this.#gains.get(author)?.disconnect();
    this.#gains.delete(author);
    for (const track of ['audio', 'screen-audio']) this.#lastSeq.delete(`${track}:${author}`);
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
    this.#jitter.clear();
    this.#lastSeq.clear();
    void this.#context?.close();
    this.#context = null;
  }

  #handle(frame: IncomingFrame, onError: (message: string) => void): void {
    if (frame.track === 'audio' || frame.track === 'screen-audio') {
      this.#handleAudio(frame, onError);
    } else {
      this.#handleVideo(frame, onError);
    }
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

    // Пропуск в номерах — это потерянная датаграмма. Считаем её: по потерям
    // видно, что буфер пора растить, а не гадать по одному только разбросу.
    const key = `${frame.track}:${frame.author}`;
    const previous = this.#lastSeq.get(key);
    if (previous !== undefined && frame.seq > previous + 1) {
      this.#lost += frame.seq - previous - 1;
      this.#jitterFor(frame.author).underrun();
    }
    if (previous === undefined || frame.seq > previous) this.#lastSeq.set(key, frame.seq);

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
        error: (error) => onError(`декодер картинки: ${error.message}`),
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

  #jitterFor(author: string): Jitter {
    let jitter = this.#jitter.get(author);
    if (!jitter) {
      jitter = new Jitter();
      this.#jitter.set(author, jitter);
    }
    return jitter;
  }

  /** Планирование звука с запасом, подобранным под реальный разброс прихода. */
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

    const jitter = this.#jitterFor(author);
    jitter.observe(buffer.duration * 1000);

    const source = context.createBufferSource();
    source.buffer = buffer;
    source.connect(this.#gainFor(author, context));
    // Отыгравший источник обязан отцепиться от графа. Иначе за десятиминутный
    // разговор их накапливается под тридцать тысяч на человека, и каждый
    // держит свой буфер.
    source.onended = () => source.disconnect();

    const queued = this.#nextPlay.get(author) ?? 0;
    // Очередь опустела — значит запаса не хватило. Этот кадр играть уже поздно,
    // но буфер после такого обязан подрасти.
    if (queued > 0 && queued < context.currentTime) jitter.underrun();

    const earliest = context.currentTime + jitter.lead;
    let at = Math.max(earliest, queued);

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
