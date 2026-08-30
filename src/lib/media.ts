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

/**
 * Границы запаса буфера звука.
 *
 * Раньше запас был один на всех и намертво: 60 мс. На ровном канале это лишняя
 * задержка в разговоре, на дрожащем — недобор, из-за которого звук рвётся.
 * Теперь запас считается по реальному разбросу времени прихода.
 */
const AUDIO_LEAD_MIN = 0.04;
/**
 * Запас буфера для музыки — втрое больше разговорного.
 *
 * В разговоре лишняя десятая доля секунды мешает: люди перебивают друг друга.
 * В музыке она не значит ничего, а вот щелчок на месте недостающего блока
 * слышен всем и сразу.
 */
const MUSIC_LEAD_MIN = 0.12;
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
/**
 * Битрейт общего плеера.
 *
 * Вчетверо выше голосового: голос на 32 кбит/с разборчив, а музыка на них
 * превращается в телефонный звонок из подвала. И два канала вместо одного —
 * сведение стерео в моно слышно на первой же гитаре.
 */
const MUSIC_BITRATE = 128_000;

/** Модуль захвата звука. Лежит в `public/`, отдаётся со своего origin. */
const CAPTURE_WORKLET = '/audio-capture-worklet.js';

export type TrackKind = 'audio' | 'video' | 'screen' | 'screen-audio' | 'music';

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

/** Тише этого — тишина, а не речь. Порог по среднеквадратичной громкости. */
const SPEAKING_RMS = 0.012;

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
  #audioEncoder: AudioEncoder | null = null;
  #stopFns: Array<() => void> = [];
  #screenStream: MediaStream | null = null;
  #screenStop: (() => void) | null = null;
  #screenAudioStop: (() => void) | null = null;
  /** Куда сообщать громкость микрофона. Экрана это не касается: он не «говорит». */
  #onLevel: ((level: number) => void) | null = null;
  /** Дорожки картинки по виду: камера и демонстрация настраиваются порознь. */
  #lanes = new Map<TrackKind, VideoLane>();

  get stream(): MediaStream | null {
    return this.#stream;
  }

  async start(options: CaptureOptions): Promise<void> {
    this.#onLevel = options.onLevel ?? null;
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
    this.#audioEncoder = await this.#startAudioTrack(
      track,
      TRACK_AUDIO,
      onError,
      (stop) => this.#stopFns.push(stop),
      (level) => this.#onLevel?.(level),
    );
  }

  /** Кодирование произвольной звуковой дорожки: микрофона или звука экрана. */
  async #startAudioTrack(
    track: MediaStreamTrack,
    kind: number,
    onError: (message: string) => void,
    keepStop: (stop: () => void) => void,
    onLevel?: (level: number) => void,
  ): Promise<AudioEncoder> {
    const encoder = new AudioEncoder({
      output: (chunk) => {
        const data = new Uint8Array(chunk.byteLength);
        chunk.copyTo(data);
        void send(pack(kind, true, CODEC_VP8, chunk.timestamp, data));
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
      // Громкость считаем до кодирования и независимо от него: индикатор
      // обязан работать и тогда, когда кодировщик отвалился.
      onLevel?.(loudness(samples));
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
    track.addEventListener('ended', () => this.stopScreen());

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

/**
 * Общий плеер: отсчёты приходят из ядра, а кодируются здесь.
 *
 * Захват нативный — вебвью на маке системный звук не отдаёт вовсе, — но
 * кодирование осталось на своём месте, рядом с микрофонным. Ядру от этого
 * достался ровно один новый путь: сырые отсчёты наверх.
 */
export class MusicCapture {
  #encoder: AudioEncoder | null = null;
  /** Хвост, не набравший целого блока Opus. Чередующийся, как и всё остальное. */
  #tail = new Float32Array(0);
  #channels = 2;
  #timestamp = 0;

  get channels(): number {
    return this.#channels;
  }

  async start(onError: (message: string) => void): Promise<void> {
    const encoder = new AudioEncoder({
      output: (chunk) => {
        const data = new Uint8Array(chunk.byteLength);
        chunk.copyTo(data);
        // Число каналов едет в тех же битах, где у картинки кодек: декодер
        // настраивается до первого кадра, и угадывать раскладку ему нечем.
        void send(pack(TRACK_MUSIC, true, this.#channels, chunk.timestamp, data));
      },
      error: (error) => onError(`кодировщик музыки: ${error.message}`),
    });

    // Стерео даётся не везде. Молча свести в моно нельзя — слушатель настроит
    // декодер на два канала и получит шум, — поэтому раскладка едет в кадре.
    try {
      encoder.configure({
        codec: 'opus',
        sampleRate: 48000,
        numberOfChannels: 2,
        bitrate: MUSIC_BITRATE,
      });
    } catch {
      this.#channels = 1;
      encoder.configure({
        codec: 'opus',
        sampleRate: 48000,
        numberOfChannels: 1,
        bitrate: MUSIC_BITRATE / 2,
      });
    }
    this.#encoder = encoder;

    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (payload) => this.#take(new Float32Array(payload));
    await invoke('music_stream', { channel });
  }

  stop(): void {
    shut(this.#encoder);
    this.#encoder = null;
    this.#tail = new Float32Array(0);
  }

  /**
   * Нарезать пришедшее на кадры Opus.
   *
   * Система отдаёт блоки своего размера, а кодек ждёт ровно двадцать
   * миллисекунд. Без нарезки каждый второй блок приезжал бы неполным.
   */
  #take(incoming: Float32Array): void {
    const encoder = this.#encoder;
    if (!encoder || encoder.state !== 'configured') return;

    // Захват отдаёт всегда два канала; если кодируем в моно — сводим сами.
    const samples = this.#channels === 2 ? incoming : downmix(incoming);
    const merged = new Float32Array(this.#tail.length + samples.length);
    merged.set(this.#tail);
    merged.set(samples, this.#tail.length);

    const step = AUDIO_BLOCK * this.#channels;
    let at = 0;
    while (merged.length - at >= step) {
      const block = merged.slice(at, at + step);
      at += step;
      const frame = new AudioData({
        format: 'f32',
        sampleRate: 48000,
        numberOfFrames: AUDIO_BLOCK,
        numberOfChannels: this.#channels,
        timestamp: this.#timestamp,
        data: block,
      });
      encoder.encode(frame);
      // Память AudioData живёт вне кучи JavaScript, и сборщик её не трогает.
      frame.close();
      this.#timestamp += Math.round((AUDIO_BLOCK / 48000) * 1_000_000);
    }
    this.#tail = merged.slice(at);
  }
}

/** Свести чередующееся стерео в моно. */
function downmix(samples: Float32Array): Float32Array {
  const out = new Float32Array(samples.length >> 1);
  for (let i = 0; i < out.length; i += 1) {
    out[i] = (samples[i * 2] + samples[i * 2 + 1]) / 2;
  }
  return out;
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
  #floor: number;
  #lead: number;
  #spread = 0;
  #last = 0;

  constructor(floor = AUDIO_LEAD_MIN) {
    this.#floor = floor;
    this.#lead = floor;
  }

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
      Math.max(this.#floor, (this.#spread * 2.5 + 20) / 1000),
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
  /** Каким кодеком настроен декодер дорожки: сменился — надо пересобирать. */
  #codecs = new Map<string, number>();
  #canvases = new Map<string, HTMLCanvasElement>();
  #context: AudioContext | null = null;
  #nextPlay = new Map<string, number>();
  /** По регулятору громкости на каждого: в звонке люди звучат по-разному. */
  #gains = new Map<string, GainNode>();
  #volumes = new Map<string, number>();
  /**
   * Громкость общего плеера — своя у каждого слушателя и отдельно от голоса.
   *
   * Иначе не выйдет главного: сделать музыку потише, чтобы за ней было слышно
   * разговор. Один регулятор на всё двигал бы их вместе.
   */
  #musicVolume = 1;
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
  /** Громкость общего плеера у себя. Соседей она не касается. */
  setMusicVolume(value: number): void {
    this.#musicVolume = Math.max(0, Math.min(2, value));
    for (const [key, node] of this.#gains) {
      if (key.startsWith('music:') && this.#context) {
        node.gain.setTargetAtTime(this.#musicVolume, this.#context.currentTime, 0.02);
      }
    }
  }

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
    for (const key of [author, `music:${author}`]) {
      shut(this.#audio.get(key));
      this.#audio.delete(key);
      this.#nextPlay.delete(key);
      this.#jitter.delete(key);
      this.#gains.get(key)?.disconnect();
      this.#gains.delete(key);
    }
    for (const track of ['audio', 'screen-audio', 'music']) {
      this.#lastSeq.delete(`${track}:${author}`);
    }
    for (const track of ['video', 'screen']) {
      const key = `${track}:${author}`;
      shut(this.#video.get(key));
      this.#video.delete(key);
      this.#codecs.delete(key);
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
    this.#codecs.clear();
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

  /** Кто из участников сейчас транслирует музыку. */
  playingMusic(): string[] {
    return [...this.#audio.keys()]
      .filter((key) => key.startsWith('music:'))
      .map((key) => key.slice('music:'.length));
  }

  /** Кто из участников сейчас показывает экран. */
  sharingScreen(): string[] {
    return [...this.#video.keys()]
      .filter((key) => key.startsWith('screen:'))
      .map((key) => key.slice('screen:'.length));
  }

  #handleAudio(frame: IncomingFrame, onError: (message: string) => void): void {
    // Звук экрана микшируется с голосом того же человека — это его звук.
    // Музыка общего плеера — нет: у неё своя громкость и своя раскладка.
    const music = frame.track === 'music';
    const key = music ? `music:${frame.author}` : frame.author;

    let decoder = this.#audio.get(key);
    if (!decoder) {
      decoder = new AudioDecoder({
        output: (data) => this.#play(key, data, music),
        error: (error) => onError(`декодер звука: ${error.message}`),
      });
      decoder.configure({
        codec: 'opus',
        sampleRate: 48000,
        // У музыки число каналов приезжает в кадре: ведущий мог не получить
        // стерео от своего движка и кодировать в моно.
        numberOfChannels: music ? Math.max(1, Math.min(2, frame.codec)) : 1,
      });
      this.#audio.set(key, decoder);
    }
    if (decoder.state !== 'configured') return;

    // Пропуск в номерах — это потерянная датаграмма. Считаем её: по потерям
    // видно, что буфер пора растить, а не гадать по одному только разбросу.
    const seqKey = `${frame.track}:${frame.author}`;
    const previous = this.#lastSeq.get(seqKey);
    if (previous !== undefined && frame.seq > previous + 1) {
      this.#lost += frame.seq - previous - 1;
      this.#jitterFor(key, music).underrun();
    }
    if (previous === undefined || frame.seq > previous) this.#lastSeq.set(seqKey, frame.seq);

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
    // Собеседник мог пересобрать кодировщик на другом кодеке — например, включив
    // демонстрацию, под которую нашёлся аппаратный H.264. Старый декодер такой
    // поток не поймёт и будет молча выдавать ошибку за ошибкой.
    if (decoder && this.#codecs.get(key) !== frame.codec) {
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
      decoder.configure({
        codec: VIDEO_CODECS[frame.codec] ?? VIDEO_CODECS[CODEC_VP8],
        optimizeForLatency: true,
      });
      this.#video.set(key, decoder);
      this.#codecs.set(key, frame.codec);
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

  #jitterFor(key: string, music = false): Jitter {
    let jitter = this.#jitter.get(key);
    if (!jitter) {
      jitter = new Jitter(music ? MUSIC_LEAD_MIN : AUDIO_LEAD_MIN);
      this.#jitter.set(key, jitter);
    }
    return jitter;
  }

  /** Планирование звука с запасом, подобранным под реальный разброс прихода. */
  #play(key: string, data: AudioData, music = false): void {
    this.#context ??= new AudioContext({ sampleRate: 48000 });
    const context = this.#context;
    if (context.state === 'suspended') void context.resume();

    const frames = data.numberOfFrames;
    const channels = Math.max(1, data.numberOfChannels);
    const buffer = context.createBuffer(channels, frames, data.sampleRate);
    const plane = new Float32Array(frames);
    for (let index = 0; index < channels; index += 1) {
      data.copyTo(plane, { planeIndex: index, format: 'f32-planar' });
      buffer.copyToChannel(plane, index);
    }
    data.close();
    // Громкость до регулятора: подсветка говорит о человеке, а не о том, как
    // громко его сделали у себя. Считаем по последнему каналу — для голоса он
    // единственный, а музыке подсветка не нужна вовсе.
    const level = loudness(plane);

    const jitter = this.#jitterFor(key, music);
    jitter.observe(buffer.duration * 1000);

    const source = context.createBufferSource();
    source.buffer = buffer;
    source.connect(this.#gainFor(key, context));
    // Отыгравший источник обязан отцепиться от графа. Иначе за десятиминутный
    // разговор их накапливается под тридцать тысяч на человека, и каждый
    // держит свой буфер.
    source.onended = () => source.disconnect();

    const queued = this.#nextPlay.get(key) ?? 0;
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
    this.#nextPlay.set(key, at + buffer.duration);

    // Индикатор «говорит» — по громкости, а не по факту прихода кадра. Opus
    // шлёт их непрерывно, и молчащий человек подсвечивался наравне с
    // говорящим: рамка горела у всех и не значила ничего. Музыка сюда не
    // попадает: она играет у всех, а «говорит» — это про человека.
    if (!music && level > SPEAKING_RMS) this.#onSpeaker(key);
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

  /** Регулятор громкости дорожки, создаётся при первом же кадре звука. */
  #gainFor(key: string, context: AudioContext): GainNode {
    let node = this.#gains.get(key);
    if (!node) {
      node = context.createGain();
      node.gain.value = key.startsWith('music:')
        ? this.#musicVolume
        : (this.#volumes.get(key) ?? 1);
      node.connect(context.destination);
      this.#gains.set(key, node);
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
