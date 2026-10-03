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
//
// Звук устроен иначе, чем картинка. Захват — здесь, ради системного
// эхоподавления, а кодирование, приём с восстановлением потерь и сведение — в
// ядре, на Opus 1.6. Сюда возвращается один готовый поток, и его играет
// ворклет в потоке аудиорендера.

import { invoke, Channel } from '@tauri-apps/api/core';

/** Разметка кадра. Должна совпадать с `net::media` в Rust. */
const HEADER = 47;
const VERSION = 3;
const TRACK_AUDIO = 0;
const TRACK_VIDEO = 1;
const TRACK_SCREEN = 2;
const TRACK_SCREEN_AUDIO = 3;
const TRACK_MUSIC = 4;

/**
 * Кодеки картинки. Номер едет в заголовке кадра, строка настраивает кодировщик
 * и декодер.
 *
 * Раньше VP8 был прибит гвоздями с обеих сторон. Он есть везде — и он же
 * единственный, который на маке кодируется процессором: аппаратного VP8 нет ни
 * у Intel, ни у Apple. На демонстрации 1080p это половина ядра под кодирование
 * и картинка, которая рассыпается ровно тогда, когда в неё вглядываются.
 * H.264 кодируется чипом и при том же битрейте держит текст заметно чётче,
 * поэтому его и пробуем первым, а VP8 остаётся последним рубежом.
 */
const VIDEO_CODECS = ['vp8', 'avc1.42E01F', 'avc1.640028', 'vp09.00.10.08'] as const;
/** Порядок предпочтения: сначала аппаратный H.264, VP8 — если больше нечем. */
const CODEC_ORDER = [2, 1, 3, 0];
const CODEC_VP8 = 0;

/** Через сколько молчания дорожка считается погасшей. */
const STALE_AFTER = 1500;

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
/** Модули ворклетов. Лежат в `public/`, отдаются со своего origin. */
const CAPTURE_WORKLET = '/audio-capture-worklet.js';
const PLAYBACK_WORKLET = '/audio-playback-worklet.js';

export type TrackKind = 'audio' | 'video' | 'screen' | 'screen-audio' | 'music';

/**
 * Какие дорожки — звук. Ядро сюда их больше не присылает: их разбирает и
 * сводит оно само. Проверка осталась на случай кадра от старого ядра —
 * видеодекодер на звуке молча настраивался бы на H.264.
 */
const AUDIO_TRACKS = new Set<TrackKind>(['audio', 'screen-audio', 'music']);

/**
 * Как часто проверяем, не уснул ли аудиоконтекст.
 *
 * Событие `statechange` приходит не всегда: у прерванного системой контекста
 * Safari держит нестандартное состояние `interrupted`, о котором не сообщает.
 * Поэтому рядом с подпиской идёт и опрос.
 */
const AWAKE_CHECK = 1000;

/**
 * Держать аудиоконтекст в работе, что бы с ним ни делала система.
 *
 * Это не перестраховка. Захват звука приложения поднимает свою аудиосессию, и
 * система вправе прервать ею нашу: контекст микрофона уходит в приостановку и
 * сам уже не возвращается. Воркер перестаёт отдавать кадры — собеседники
 * перестают слышать человека, причём насовсем, до перезахода в комнату.
 * Лечится это одной строчкой `resume()`, надо только заметить.
 */
function keepAwake(context: AudioContext): () => void {
  const wake = () => {
    if (context.state !== 'running') void context.resume().catch(() => undefined);
  };
  context.addEventListener('statechange', wake);
  const timer = window.setInterval(wake, AWAKE_CHECK);
  return () => {
    context.removeEventListener('statechange', wake);
    window.clearInterval(timer);
  };
}

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
  /**
   * У картинки — номер кодека из `VIDEO_CODECS`.
   *
   * У общего плеера то же поле означает число каналов: стерео даётся не на
   * каждой платформе, а декодер настраивается до первого кадра, и угадывать
   * раскладку ему нечем. У голоса поле не значит ничего.
   */
  codec: number;
  ts: number;
  seq: number;
  data: Uint8Array;
}

/**
 * Чего не хватает платформе для звонков. Пусто — всё на месте.
 *
 * Кодировщик звука вебвью больше не нужен: голос кодирует ядро. Без
 * WebCodecs звонок остаётся голосовым — пропадает только картинка.
 */
export function missingCapabilities(): string[] {
  const gaps: string[] = [];
  if (!navigator.mediaDevices?.getUserMedia) gaps.push('getUserMedia');
  if (typeof globalThis.AudioWorkletNode === 'undefined') gaps.push('AudioWorklet');
  return gaps;
}

/** Есть ли чем кодировать и показывать картинку. */
export function videoSupported(): boolean {
  return typeof globalThis.VideoEncoder !== 'undefined' && typeof globalThis.VideoDecoder !== 'undefined';
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

function pack(
  track: number,
  keyframe: boolean,
  codec: number,
  ts: number,
  data: Uint8Array,
): Uint8Array {
  const out = new Uint8Array(HEADER + data.byteLength);
  const view = new DataView(out.buffer);
  out[0] = VERSION;
  out[1] = track;
  // Ключевой кадр в нулевом бите, кодек — в трёх следующих.
  out[2] = (keyframe ? 1 : 0) | ((codec & 7) << 1);
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
          : raw[1] === TRACK_MUSIC
            ? 'music'
            : raw[1] === TRACK_VIDEO
              ? 'video'
              : 'audio',
    keyframe: (raw[2] & 1) !== 0,
    codec: (raw[2] >> 1) & 7,
    ts: Number(view.getBigInt64(35, true)),
    seq: view.getUint32(43, true),
    data: raw.subarray(HEADER),
  };
}

async function send(frame: Uint8Array): Promise<void> {
  // Сырой ArrayBuffer вместо JSON: видео в base64 стоило бы трети трафика.
  await invoke('send_media', frame);
}

/**
 * Блок своего звука — в ядро, кодировщику: `[0]` дорожка, дальше int16 LE.
 *
 * Int16 вместо float — вдвое меньше байт через мост, а точности микрофону
 * после эхоподавления хватает с запасом.
 */
function sendPcm(track: number, samples: Float32Array): void {
  const out = new Uint8Array(1 + samples.length * 2);
  out[0] = track;
  const view = new DataView(out.buffer);
  for (let i = 0; i < samples.length; i += 1) {
    const clamped = Math.max(-1, Math.min(1, samples[i]));
    view.setInt16(1 + i * 2, Math.round(clamped * 32767), true);
  }
  void invoke('send_pcm', out).catch(() => undefined);
}

// ── исходящий поток ─────────────────────────────────────────────────────────

export interface CaptureOptions {
  video: boolean;
  onError: (message: string) => void;
  /**
   * Насколько громко звучит собственный микрофон, 0..1.
   *
   * Нужен ровно для одного: показать человеку, что звук идёт от него. Своего
   * голоса в звонке не слышно — эхоподавление на то и стоит, — поэтому без
   * индикатора «меня слышно?» проверяется только вопросом вслух.
   */
  onLevel?: (level: number) => void;
}

interface VideoConfig {
  width: number;
  height: number;
  bitrate: number;
  framerate: number;
}

/** Стартовые настройки дорожек картинки. Дальше их двигает governor в ядре. */
const CAMERA: VideoConfig = { width: 640, height: 360, bitrate: 500_000, framerate: 24 };

/**
 * Потолок демонстрации.
 *
 * 1080p, а не «сколько дал экран»: на маке getDisplayMedia отдаёт retina-кадр
 * вдвое больше самого экрана, и кодировать его — это вчетверо больше пикселей
 * ради разницы, которой не видно. Ниже 1080p опускаться тоже нельзя: 720p,
 * растянутые на весь монитор, — это ровно то мыло, из-за которого демонстрацию
 * и разворачивать не хотелось.
 */
const SCREEN_MAX_WIDTH = 1920;
const SCREEN_MAX_HEIGHT = 1080;
/** Плавность демонстрации. Двенадцать кадров хватало на слайды и рвало прокрутку. */
const SCREEN_FPS = 30;
/**
 * Бит на пиксель в секунду. Экран сжимается лучше камеры — он почти весь
 * неподвижен, — но платит за резкость: мыло на лице незаметно, мыло на букве
 * делает её нечитаемой.
 */
const SCREEN_BITS_PER_PIXEL = 0.07;
/** Границы битрейта демонстрации. Верхняя совпадает с потолком governor'а в ядре. */
const SCREEN_BITRATE_MIN = 1_500_000;
const SCREEN_BITRATE_MAX = 8_000_000;


/** Кодировщик картинки вместе со всем, что нужно, чтобы его перенастроить. */
interface VideoLane {
  encoder: VideoEncoder;
  config: VideoConfig;
  /** Номер кодека, которым настроен кодировщик, — он же едет в заголовке кадра. */
  codec: number;
  /** Выдать ключевой кадр при ближайшей возможности. */
  forced: boolean;
}

/**
 * Настройки кодировщика под выбранный кодек.
 *
 * H.264 просим отдавать в annex-b: в этом формате заголовки последовательности
 * едут внутри каждого ключевого кадра. Формат по умолчанию отдаёт их отдельным
 * описанием, один раз, — а у нас поток без начала: собеседник подключается
 * посреди разговора и такого описания уже не увидит.
 */
function encoderConfig(codec: number, config: VideoConfig): VideoEncoderConfig {
  const name = VIDEO_CODECS[codec] ?? VIDEO_CODECS[CODEC_VP8];
  return {
    codec: name,
    ...config,
    latencyMode: 'realtime',
    ...(name.startsWith('avc1') ? { avc: { format: 'annexb' as const } } : {}),
  };
}

/**
 * Первый кодек из списка предпочтений, который движок согласен взять.
 *
 * Спрашиваем именно с теми настройками, с какими будем кодировать: поддержка
 * кодека сама по себе ничего не обещает — 1080p может не влезть в профиль,
 * который движок готов дать.
 */
async function pickCodec(config: VideoConfig): Promise<number> {
  for (const codec of CODEC_ORDER) {
    try {
      const probe = await VideoEncoder.isConfigSupported(encoderConfig(codec, config));
      if (probe.supported) return codec;
    } catch {
      // Движок не знает такой строки кодека — просто пробуем следующую.
    }
  }
  return CODEC_VP8;
}

/**
 * Настройки демонстрации по тому, что реально отдала система.
 *
 * Считать их заранее нельзя: экраны бывают от ноутбучных до 5K, и одна и та же
 * константа для всех означает либо мыло, либо кодирование вчетверо большего
 * кадра впустую.
 */
function screenConfig(track: MediaStreamTrack): VideoConfig {
  const settings = track.getSettings();
  const sourceWidth = settings.width ?? SCREEN_MAX_WIDTH;
  const sourceHeight = settings.height ?? SCREEN_MAX_HEIGHT;
  const scale = Math.min(1, SCREEN_MAX_WIDTH / sourceWidth, SCREEN_MAX_HEIGHT / sourceHeight);
  // Стороны чётные: кодеки работают с блоками, нечётная сторона либо
  // отвергается, либо молча округляется — и картинка едет со сдвигом.
  const width = Math.max(2, Math.round((sourceWidth * scale) / 2) * 2);
  const height = Math.max(2, Math.round((sourceHeight * scale) / 2) * 2);
  const framerate = Math.min(SCREEN_FPS, Math.round(settings.frameRate ?? SCREEN_FPS) || SCREEN_FPS);
  const bitrate = Math.min(
    SCREEN_BITRATE_MAX,
    Math.max(SCREEN_BITRATE_MIN, Math.round(width * height * framerate * SCREEN_BITS_PER_PIXEL)),
  );
  return { width, height, bitrate, framerate };
}

/** Среднеквадратичная громкость блока — по ней и видно, говорит человек или молчит. */
function loudness(samples: Float32Array): number {
  let sum = 0;
  for (const sample of samples) sum += sample * sample;
  return Math.sqrt(sum / Math.max(1, samples.length));
}

/**
 * Захват микрофона и (по желанию) камеры с кодированием в Opus и VP8.
 */
export class Capture {
  #stream: MediaStream | null = null;
  #stopFns: Array<() => void> = [];
  /**
   * Камера — своим потоком, отдельно от микрофона.
   *
   * Раньше камеру включали пересбором всего захвата: на секунду пропадал
   * голос, а заодно молча обрывалась идущая демонстрация экрана.
   */
  #camera: MediaStream | null = null;
  #cameraStop: (() => void) | null = null;
  #screenStream: MediaStream | null = null;
  #screenStop: (() => void) | null = null;
  #screenAudioStop: (() => void) | null = null;
  /** Куда сообщать громкость микрофона. Экрана это не касается: он не «говорит». */
  #onLevel: ((level: number) => void) | null = null;
  /** Дорожки картинки по виду: камера и демонстрация настраиваются порознь. */
  #lanes = new Map<TrackKind, VideoLane>();

  /** Своя камера — для плитки «вы». Без камеры пусто. */
  get stream(): MediaStream | null {
    return this.#camera;
  }

  async start(options: CaptureOptions): Promise<void> {
    this.#onLevel = options.onLevel ?? null;
    this.#stream = await navigator.mediaDevices.getUserMedia({
      audio: {
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
      },
      video: false,
    });

    await this.#startAudio(options.onError);
    if (options.video) await this.setCamera(true, options.onError);
  }

  /** Включить или выключить камеру, не трогая микрофон и демонстрацию. */
  async setCamera(on: boolean, onError: (message: string) => void): Promise<void> {
    if (!on) {
      try {
        this.#cameraStop?.();
      } catch {
        // уже остановлена
      }
      this.#cameraStop = null;
      shut(this.#lanes.get('video')?.encoder);
      this.#lanes.delete('video');
      this.#camera?.getTracks().forEach((track) => track.stop());
      this.#camera = null;
      return;
    }
    if (this.#camera) return;
    const camera = await navigator.mediaDevices.getUserMedia({
      video: { width: 640, height: 360, frameRate: 24 },
    });
    this.#camera = camera;
    const track = camera.getVideoTracks()[0];
    if (!track) return;
    await this.#videoTrackPump(track, 'video', TRACK_VIDEO, CAMERA, onError, (stop) => (this.#cameraStop = stop));
  }

  stop(): void {
    this.stopScreen();
    void this.setCamera(false, () => undefined);
    // Каждый шаг остановки — независимый: падение одного не должно оставлять
    // человека с включённой камерой и в звонке, из которого не выйти.
    for (const stop of this.#stopFns.splice(0)) {
      try {
        stop();
      } catch {
        // Узел уже отключён — не повод бросать остальное.
      }
    }
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
      lane.encoder.configure(encoderConfig(lane.codec, lane.config));
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
    await this.#startAudioTrack(
      track,
      TRACK_AUDIO,
      onError,
      (stop) => this.#stopFns.push(stop),
      (level) => this.#onLevel?.(level),
    );
  }

  /**
   * Захват произвольной звуковой дорожки — микрофона или звука экрана.
   *
   * Кодирует ядро: здесь только снимаем отсчёты по двадцать миллисекунд и
   * отдаём их. Раньше здесь стоял AudioEncoder из WebCodecs на 32 кбит/с без
   * какой-либо защиты от потерь.
   */
  async #startAudioTrack(
    track: MediaStreamTrack,
    kind: number,
    onError: (message: string) => void,
    keepStop: (stop: () => void) => void,
    onLevel?: (level: number) => void,
  ): Promise<void> {
    const context = new AudioContext({ sampleRate: 48000 });
    // Остановка регистрируется до первого ожидания: человек мог войти и сразу
    // выйти, пока грузится ворклет, — и тогда контекст микрофона и его сторож
    // жили бы вечно, а сторож ещё и будил бы закрытый захват.
    let stopped = false;
    let awake: (() => void) | null = null;
    let stopPump: (() => void) | null = null;
    let source: MediaStreamAudioSourceNode | null = null;
    keepStop(() => {
      stopped = true;
      awake?.();
      stopPump?.();
      source?.disconnect();
      void context.close().catch(() => undefined);
    });

    // Без явного возобновления контекст может остаться приостановленным,
    // и захват молча не даст ни одного кадра.
    if (context.state === 'suspended') await context.resume().catch(() => undefined);
    if (stopped) return;
    // И дальше следим за ним всю жизнь захвата: усыпить контекст система может
    // и потом — например, когда мы же поднимем захват звука приложения.
    awake = keepAwake(context);
    // А если система оборвала саму дорожку, `resume()` уже не поможет — и это
    // ровно тот случай, когда молчание хуже всего: человек говорит, его не
    // слышат, и никто не понимает почему.
    track.addEventListener('ended', () =>
      onError('система отключила микрофон — перезайдите в звонок'),
    );
    source = context.createMediaStreamSource(new MediaStream([track]));

    // Именно `Float32Array<ArrayBuffer>`, а не просто `Float32Array`: с версии
    // 5.7 TypeScript различает, чем подложен типизированный массив.
    const push = (samples: Float32Array<ArrayBuffer>) => {
      // Громкость считаем отдельно от отправки: индикатор обязан работать и
      // тогда, когда до ядра блок не дошёл.
      onLevel?.(loudness(samples));
      sendPcm(kind, samples);
    };

    const pump = await this.#pumpAudio(context, source, push);
    if (stopped) pump();
    else stopPump = pump;
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

  /** Своя демонстрация — для превью у себя же: видно, что показ идёт. */
  get screenStream(): MediaStream | null {
    return this.#screenStream;
  }

  /** Есть ли в текущей демонстрации звук. */
  get screenHasAudio(): boolean {
    return (this.#screenStream?.getAudioTracks().length ?? 0) > 0;
  }

  setScreenAudio(enabled: boolean): void {
    this.#screenStream?.getAudioTracks().forEach((track) => (track.enabled = enabled));
  }

  /**
   * Демонстрация экрана — отдельная дорожка, чтобы шла вместе с камерой.
   *
   * `onEnded` — показ остановили мимо нас, системной кнопкой «Остановить».
   * Раньше об этом никто не узнавал, и кнопка в звонке так и горела «экран вкл».
   */
  async startScreen(onError: (message: string) => void, onEnded?: () => void): Promise<void> {
    // Звук просим сразу: система сама решит, отдавать его или нет. На macOS
    // вебвью его не отдаёт, поэтому наличие дорожки проверяем, а не полагаемся.
    const stream = await navigator.mediaDevices.getDisplayMedia({
      video: {
        frameRate: { ideal: SCREEN_FPS },
        width: { max: SCREEN_MAX_WIDTH },
        height: { max: SCREEN_MAX_HEIGHT },
      },
      audio: true,
    });
    this.#screenStream = stream;
    const track = stream.getVideoTracks()[0];
    // Подсказка источнику: на экране важнее резкость, чем плавность. Без неё
    // система при нехватке ресурсов режет разрешение — то самое, ради которого
    // демонстрацию и смотрят.
    track.contentHint = 'detail';
    // Пользователь может остановить показ кнопкой самой системы, не нашей.
    track.addEventListener('ended', () => {
      if (this.#screenStream !== stream) return; // уже остановили сами
      this.stopScreen();
      onEnded?.();
    });

    await this.#videoTrackPump(
      track,
      'screen',
      TRACK_SCREEN,
      screenConfig(track),
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
    let codec = await pickCodec(config);
    const encoder = new VideoEncoder({
      output: (chunk) => {
        const data = new Uint8Array(chunk.byteLength);
        chunk.copyTo(data);
        void send(pack(kind, chunk.type === 'key', codec, chunk.timestamp, data));
      },
      error: (error) => onError(`кодировщик картинки: ${error.message}`),
    });
    try {
      encoder.configure(encoderConfig(codec, config));
    } catch {
      // Движок сказал, что кодек поддержан, и отказался его настраивать. Так
      // бывает: поддержка проверяется по строке кодека, а упирается в профиль
      // или в разрешение. Отступаем на VP8 — он медленнее и хуже, но он есть
      // везде, а звонок без картинки хуже картинки похуже.
      codec = CODEC_VP8;
      encoder.configure(encoderConfig(codec, config));
    }

    const entry: VideoLane = { encoder, config, codec, forced: true };
    this.#lanes.set(lane, entry);

    const video = document.createElement('video');
    video.srcObject = new MediaStream([track]);
    video.muted = true;
    await video.play();

    let running = true;
    let encodedAt = 0;
    const pump = () => {
      if (!running || encoder.state !== 'configured') return;
      // Кодировщику задан свой темп, а requestAnimationFrame зовёт нас со
      // скоростью монитора — на маке это до ста двадцати раз в секунду. Лишние
      // кадры не добавляют плавности: битрейт один на всех, и каждый кадр сверх
      // темпа отбирает биты у остальных, то есть делает картинку хуже, а не
      // лучше. Миллисекунда допуска — на дрожание самого таймера.
      const now = performance.now();
      if (now - encodedAt < 1000 / entry.config.framerate - 1) {
        schedule();
        return;
      }
      encodedAt = now;
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
      if (document.hidden) window.setTimeout(pump, 1000 / entry.config.framerate);
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
 * Звук звонка: один сведённый ядром поток, который играет ворклет.
 *
 * Свой аудиоконтекст, а не общий с захватом: у захвата он живёт и умирает
 * вместе с микрофоном, а слышать собеседников надо и с выключенным.
 */
export class VoicePlayer {
  #context: AudioContext | null = null;
  #node: AudioWorkletNode | null = null;
  #awake: (() => void) | null = null;
  /** Остановили, пока поднимались: дальше не идём, иначе узел и подписка повиснут. */
  #stopped = false;

  async start(onError: (message: string) => void): Promise<void> {
    const context = new AudioContext({ sampleRate: 48000, latencyHint: 'interactive' });
    this.#context = context;
    // Тот же сторож, что и у захвата: прерванный системой контекст сам не
    // возвращается, и собеседников переставало быть слышно до перезахода.
    this.#awake = keepAwake(context);
    try {
      await context.audioWorklet.addModule(PLAYBACK_WORKLET);
    } catch (error) {
      if (!this.#stopped) {
        onError(`воспроизведение звука: ${error instanceof Error ? error.message : String(error)}`);
      }
      return;
    }
    if (this.#stopped) return;
    const node = new AudioWorkletNode(context, 'bred-playback', {
      numberOfInputs: 0,
      numberOfOutputs: 1,
      outputChannelCount: [2],
    });
    node.connect(context.destination);
    this.#node = node;
    if (context.state === 'suspended') void context.resume().catch(() => undefined);

    const channel = new Channel<ArrayBuffer | number[]>();
    // Кадр уходит в ворклет без копии: владение буфером передаётся. Крупные
    // сообщения канала Tauri отдаёт буфером, мелкие и запасной путь IPC —
    // могут и массивом чисел; принимаем оба.
    channel.onmessage = (payload) => {
      const buffer = payload instanceof ArrayBuffer ? payload : new Uint8Array(payload).buffer;
      this.#node?.port.postMessage(buffer, [buffer]);
    };
    await invoke('voice_stream', { channel });
  }

  stop(): void {
    this.#stopped = true;
    this.#awake?.();
    this.#awake = null;
    this.#node?.disconnect();
    this.#node = null;
    void this.#context?.close().catch(() => undefined);
    this.#context = null;
  }
}

/**
 * Приём и показ чужой картинки. По декодеру на дорожку каждого участника.
 *
 * Звука здесь больше нет: его принимает, восстанавливает и сводит ядро, а
 * играет [`VoicePlayer`].
 */
export class Playback {
  #video = new Map<string, VideoDecoder>();
  /** Каким кодеком настроен декодер дорожки: сменился — надо пересобирать. */
  #codecs = new Map<string, number>();
  #canvases = new Map<string, HTMLCanvasElement>();
  /** Когда последний раз приходил кадр по каждой дорожке. */
  #lastFrame = new Map<string, number>();
  #watch: number | null = null;

  /** Забыть участника: декодеры на ушедших иначе копятся всю встречу. */
  forget(author: string): void {
    for (const track of ['video', 'screen']) {
      const key = `${track}:${author}`;
      shut(this.#video.get(key));
      this.#video.delete(key);
      this.#codecs.delete(key);
      this.#canvases.delete(key);
      this.#lastFrame.delete(key);
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
      if (frame && !AUDIO_TRACKS.has(frame.track)) this.#handleVideo(frame, onError);
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
    for (const decoder of this.#video.values()) shut(decoder);
    this.#video.clear();
    this.#codecs.clear();
  }

  /** Кто из участников сейчас показывает экран. */
  sharingScreen(): string[] {
    return [...this.#video.keys()]
      .filter((key) => key.startsWith('screen:'))
      .map((key) => key.slice('screen:'.length));
  }

  #handleVideo(frame: IncomingFrame, onError: (message: string) => void): void {
    // Ключ с дорожкой: у одного участника камера и экран идут одновременно.
    const key = `${frame.track}:${frame.author}`;
    this.#lastFrame.set(key, Date.now());
    let decoder = this.#video.get(key);
    // Декодер, на котором случилась ошибка, закрыт навсегда и молча
    // отказывается от всего, что ему дают. Раньше на этом картинка человека
    // замирала до перезахода в звонок; теперь его пересобираем с ближайшего
    // ключевого кадра. То же — если собеседник сменил кодек, например, включив
    // демонстрацию, под которую нашёлся аппаратный H.264.
    if (decoder && (decoder.state === 'closed' || this.#codecs.get(key) !== frame.codec)) {
      if (!frame.keyframe) return;
      shut(decoder);
      this.#video.delete(key);
      decoder = undefined;
    }
    if (!decoder) {
      // До первого ключевого кадра декодер запускать бессмысленно.
      if (!frame.keyframe) return;
      decoder = new VideoDecoder({
        output: (image) => this.#draw(key, image),
        error: (error) => onError(`декодер картинки: ${error.message}`),
      });
      try {
        decoder.configure({
          codec: VIDEO_CODECS[frame.codec] ?? VIDEO_CODECS[CODEC_VP8],
          optimizeForLatency: true,
        });
      } catch (error) {
        shut(decoder);
        onError(`декодер картинки: ${error instanceof Error ? error.message : String(error)}`);
        return;
      }
      this.#video.set(key, decoder);
      this.#codecs.set(key, frame.codec);
    }
    if (decoder.state !== 'configured') return;
    try {
      decoder.decode(
        new EncodedVideoChunk({
          type: frame.keyframe ? 'key' : 'delta',
          timestamp: frame.ts,
          data: frame.data,
        }),
      );
    } catch {
      // Испорченный кадр: декодер закроется, и следующий ключевой его пересоберёт.
    }
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
        this.#codecs.delete(key);
        this.#canvases.delete(key);
      }
    }
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
