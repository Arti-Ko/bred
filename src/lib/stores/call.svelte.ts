// Состояние звонка. Отдельно от общей сессии: у звонка своя частота изменений
// (кадры идут десятками в секунду) и своё время жизни.

import { getCurrentWindow } from '@tauri-apps/api/window';
import type { UnlistenFn } from '@tauri-apps/api/event';

import {
  api,
  errorText,
  type Id,
  type MusicSource,
  type PlayerCommand,
  type PlayerState,
} from '../ipc';
import { Capture, missingCapabilities, MusicCapture, Playback } from '../media';
import { prefs } from './prefs.svelte';

/**
 * Шкала индикатора громкости.
 *
 * Логарифмическая: линейная среднеквадратичная громкость речи почти всё время
 * прижата к нулю, и полоска по ней стоит на месте. Нижняя граница — тишина
 * комнаты после шумодава, верхняя — обычный разговорный уровень, а не крик.
 */
const LEVEL_FLOOR_DB = -60;
const LEVEL_TOP_DB = -15;
/** Столько ступенек у индикатора: меньше — рвано, больше — лишние перерисовки. */
const LEVEL_STEPS = 12;
/** С какой ступеньки считаем, что человек говорит, а не дышит в микрофон. */
const SPEAKING_LEVEL = 2 / LEVEL_STEPS;

export interface Participant {
  id: Id;
  /** Когда от участника последний раз приходил звук — для отметки «говорит». */
  spokeAt: number;
}

export class Call {
  active = $state(false);
  channel = $state<Id | null>(null);
  space = $state<Id | null>(null);
  micMuted = $state(false);
  camOn = $state(false);
  screenOn = $state(false);
  /** Идёт ли звук вместе с демонстрацией и доступен ли он вообще. */
  screenAudio = $state(false);
  screenAudioAvailable = $state(false);
  /** Звонок на всё окно, и режим «только демонстрация». */
  expanded = $state(false);
  screenOnly = $state(false);
  /**
   * Демонстрация во весь физический экран.
   *
   * Это не то же самое, что `expanded`: тот раскрывает звонок на всё окно, но
   * окно остаётся окном — сверху строка системы, снизу док, вокруг рамка. Здесь
   * в полноэкранный режим уходит само окно, и от чужого экрана человека не
   * отделяет уже ничего.
   */
  fullscreen = $state(false);
  /** Кто из собеседников показывает экран. */
  screens = $state<Id[]>([]);
  participants = $state<Participant[]>([]);
  /**
   * Свой поток. Именно реактивное состояние, а не геттер к приватному полю:
   * геттер не будил эффект, который присваивает поток элементу video, и
   * человек видел чёрный квадрат вместо себя.
   */
  stream = $state<MediaStream | null>(null);
  status = $state('');
  /** Чего не хватает платформе. Непусто — звонки недоступны, и надо сказать честно. */
  gaps = $state<string[]>([]);
  /**
   * Насколько громко звучит собственный микрофон, 0..1, ступеньками.
   *
   * Своего голоса в звонке не слышно, и без индикатора «меня слышно?» узнаётся
   * только вопросом вслух — обычно после минуты речи в отключённый микрофон.
   */
  level = $state(0);
  /**
   * Что играет в комнате и у кого. Приезжает от ведущего, а не считается
   * локально: источник звука один на всех, и состояние у него тоже одно.
   */
  player = $state<PlayerState | null>(null);
  /** Что можно подключить. Заполняется, когда открывают выбор источника. */
  sources = $state<MusicSource[]>([]);
  /** Транслируем ли звук мы сами. */
  sharingMusic = $state(false);
  /** Открыт ли выбор источника. */
  musicPicker = $state(false);

  #capture: Capture | null = null;
  #music: MusicCapture | null = null;
  #playback: Playback | null = null;
  #poll: number | null = null;
  /**
   * Когда от кого последний раз приходил звук.
   *
   * Реактивное состояние, а не обычная Map: подсветка говорящего — самое
   * быстрое, что есть в звонке, а из Map она обновлялась только когда
   * перерисовку вызывало что-то другое, то есть раз в полторы секунды.
   */
  #speakers = $state<Record<Id, number>>({});
  /**
   * Несглаженная громкость. Обычное поле, а не состояние: она меняется полсотни
   * раз в секунду, а перерисовывать индикатор надо на порядок реже.
   */
  #rawLevel = 0;
  /** Что было развёрнуто до полного экрана — чтобы вернуть, а не сбросить. */
  #beforeFullscreen: { expanded: boolean; screenOnly: boolean } | null = null;
  /** Слежение за окном: из полного экрана можно выйти и мимо нашей кнопки. */
  #unwatchWindow: UnlistenFn | null = null;

  get localStream(): MediaStream | null {
    return this.stream;
  }

  /** Говорит ли участник прямо сейчас (звук приходил меньше 400 мс назад). */
  speaking(id: Id): boolean {
    const at = this.#speakers[id];
    return at !== undefined && Date.now() - at < 400;
  }

  /** Говорим ли сейчас мы сами. Выключенный микрофон — сразу нет, без задержки. */
  get speakingSelf(): boolean {
    return !this.micMuted && this.level > SPEAKING_LEVEL;
  }

  /**
   * Принять громкость микрофона от захвата.
   *
   * Вверх — сразу, вниз — плавно: индикатор, гаснущий в паузах между слогами,
   * мигает и читается хуже, чем неподвижный. Наружу отдаём ступеньками, иначе
   * полсотни перерисовок в секунду ради полоски в три пикселя.
   */
  #takeLevel(rms: number): void {
    const db = 20 * Math.log10(Math.max(rms, 1e-6));
    const value = Math.max(0, Math.min(1, (db - LEVEL_FLOOR_DB) / (LEVEL_TOP_DB - LEVEL_FLOOR_DB)));
    this.#rawLevel = value > this.#rawLevel ? value : this.#rawLevel + (value - this.#rawLevel) * 0.2;
    const shown = Math.round(this.#rawLevel * LEVEL_STEPS) / LEVEL_STEPS;
    if (shown !== this.level) this.level = shown;
  }

  // ── общий плеер ─────────────────────────────────────────────────────────

  /** Играет ли плеер в этой комнате, а не в соседней. */
  get playerHere(): boolean {
    return this.player !== null && this.player.channel === this.channel;
  }

  /** Громкость общего плеера у себя. Соседей она не касается. */
  get musicVolume(): number {
    return prefs.musicVolume;
  }

  setMusicVolume(value: number): void {
    prefs.setMusicVolume(value);
    this.#playback?.setMusicVolume(value);
  }

  /** Открыть или закрыть выбор источника. Список спрашиваем при открытии:
   *  приложения запускают и закрывают, а список, собранный час назад, врёт. */
  async toggleMusicPicker(): Promise<void> {
    this.musicPicker = !this.musicPicker;
    if (this.musicPicker) await this.loadSources();
  }

  /** Спросить у системы, чей звук можно подключить. */
  async loadSources(): Promise<void> {
    try {
      this.sources = await api.musicSources();
    } catch (error) {
      this.sources = [];
      this.status = errorText(error);
    }
  }

  /** Включить звук приложения на всю комнату. */
  async startMusic(source: string): Promise<void> {
    if (!this.active) return;
    try {
      // Сначала подписка, потом захват: иначе первые блоки звука упадут в
      // никуда, и трансляция начнётся с провала на четверть секунды.
      const music = new MusicCapture();
      await music.start((message) => (this.status = message));
      this.#music = music;

      await api.musicStart(source);
      this.sharingMusic = true;
      this.musicPicker = false;
      this.status = '';
      await this.refreshPlayer();
    } catch (error) {
      this.#music?.stop();
      this.#music = null;
      this.sharingMusic = false;
      this.status = errorText(error);
    }
  }

  /** Выключить свою трансляцию. */
  async stopMusic(): Promise<void> {
    this.#music?.stop();
    this.#music = null;
    this.sharingMusic = false;
    await api.musicStop().catch(() => undefined);
    this.player = null;
  }

  /** Нажать кнопку на пульте — своём или чужом. */
  async musicCommand(command: PlayerCommand): Promise<void> {
    try {
      await api.musicControl(command);
    } catch (error) {
      this.status = errorText(error);
    }
  }

  /** Перечитать состояние плеера. Зовётся по уведомлению от ядра. */
  async refreshPlayer(): Promise<void> {
    if (!this.space) {
      this.player = null;
      return;
    }
    try {
      this.player = await api.playerState(this.space);
    } catch {
      // Ядро ответит на следующем уведомлении — паниковать не из-за чего.
    }
  }

  /** Громкость собеседника: 0 — не слышно, 1 — как есть, до 4 — усиление. */
  volume(author: Id): number {
    return prefs.volumes[author] ?? 1;
  }

  setVolume(author: Id, value: number): void {
    prefs.setVolume(author, value);
    this.#playback?.setVolume(author, value);
  }

  attachCanvas(author: Id, canvas: HTMLCanvasElement | null, track: 'video' | 'screen' = 'video'): void {
    this.#playback?.attachCanvas(author, canvas, track);
  }

  /** Развернуть звонок на всё окно и обратно. */
  toggleExpanded(): void {
    // Из полного экрана «свернуть» обязано выводить именно из него, иначе
    // кнопка выглядит сломанной: нажал — ничего не изменилось.
    if (this.fullscreen) {
      void this.exitFullscreen();
      return;
    }
    this.expanded = !this.expanded;
    if (!this.expanded) this.screenOnly = false;
  }

  /** Полный экран для чужой демонстрации — туда и обратно. */
  async toggleFullscreen(): Promise<void> {
    if (this.fullscreen) await this.exitFullscreen();
    else await this.enterFullscreen();
  }

  async enterFullscreen(): Promise<void> {
    if (this.fullscreen || this.screens.length === 0) return;
    if (!(await this.#setWindowFullscreen(true))) return;

    this.#beforeFullscreen = { expanded: this.expanded, screenOnly: this.screenOnly };
    this.fullscreen = true;
    // Полный экран без «только экрана» показывал бы чужую демонстрацию рядом с
    // плитками камер — то есть мелко, ровно там, где хотели крупно.
    this.expanded = true;
    this.screenOnly = true;
    await this.#watchWindow();
  }

  async exitFullscreen(): Promise<void> {
    if (!this.fullscreen) return;
    await this.#setWindowFullscreen(false);
    this.#forgetFullscreen();
  }

  /**
   * Вернуть интерфейс из полного экрана, не трогая само окно.
   *
   * Отдельно от `exitFullscreen`, потому что окно могло выйти само: на macOS
   * это зелёная кнопка и Ctrl+Cmd+F, и тогда просить систему выйти повторно
   * незачем — надо лишь догнать состояние.
   */
  #forgetFullscreen(): void {
    this.fullscreen = false;
    const before = this.#beforeFullscreen;
    this.#beforeFullscreen = null;
    this.expanded = before?.expanded ?? false;
    this.screenOnly = before?.screenOnly ?? false;
    this.#unwatchWindow?.();
    this.#unwatchWindow = null;
  }

  async #setWindowFullscreen(on: boolean): Promise<boolean> {
    try {
      await getCurrentWindow().setFullscreen(on);
      return true;
    } catch (error) {
      // Платформа не дала полноэкранный режим. Сказать честно лучше, чем
      // оставить человека с кнопкой, которая молча ничего не делает.
      this.status = errorText(error);
      return false;
    }
  }

  /**
   * Следить, не вышло ли окно из полного экрана помимо нас.
   *
   * Полный экран снимается и средствами системы, и тогда интерфейс остался бы
   * в режиме «только экран» с кнопкой «из полного экрана» — при обычном окне.
   */
  async #watchWindow(): Promise<void> {
    try {
      const window = getCurrentWindow();
      this.#unwatchWindow = await window.onResized(async () => {
        if (!this.fullscreen) return;
        if (!(await window.isFullscreen())) this.#forgetFullscreen();
      });
    } catch {
      // Без слежения полный экран работает, просто выходить из него придётся
      // нашей же кнопкой. Это не повод не пускать в него вовсе.
    }
  }

  /** Оставить на экране только демонстрацию, без плиток с камерами. */
  toggleScreenOnly(): void {
    this.screenOnly = !this.screenOnly;
    if (this.screenOnly) this.expanded = true;
  }

  /** Звук вместе с демонстрацией: включается и выключается на лету. */
  toggleScreenAudio(): void {
    if (!this.screenAudioAvailable) return;
    this.screenAudio = !this.screenAudio;
    this.#capture?.setScreenAudio(this.screenAudio);
  }

  /** Показать или убрать демонстрацию своего экрана. */
  async toggleScreen(): Promise<void> {
    if (!this.active || !this.#capture) return;
    try {
      if (this.screenOn) {
        this.#capture.stopScreen();
        this.screenOn = false;
        this.screenAudio = false;
        this.screenAudioAvailable = false;
        this.screenOnly = false;
      } else {
        await this.#capture.startScreen((message) => (this.status = message));
        this.screenOn = this.#capture.sharingScreen;
        // Системный звук отдаёт не каждая платформа: на macOS вебвью его не
        // даёт вовсе, поэтому проверяем дорожку, а не полагаемся на запрос.
        this.screenAudioAvailable = this.#capture.screenHasAudio;
        this.screenAudio = this.screenAudioAvailable;
        if (!this.screenAudioAvailable) {
          this.status = 'система не отдала звук экрана — идёт только картинка';
        }
      }
    } catch (error) {
      // Отказ в системном диалоге выбора окна — не ошибка, просто передумали.
      this.screenOn = false;
      const text = errorText(error);
      if (!/denied|abort|NotAllowed/i.test(text)) this.status = text;
    }
  }

  async join(space: Id, channel: Id, withVideo: boolean): Promise<void> {
    const gaps = missingCapabilities();
    if (gaps.length > 0) {
      this.gaps = gaps;
      this.status = `звонки недоступны: платформа без ${gaps.join(', ')}`;
      return;
    }

    try {
      this.#playback = new Playback((author) => {
        const now = Date.now();
        // Обновляем не чаще, чем нужно глазу: звук идёт полсотни кадров
        // в секунду, и дёргать перерисовку на каждый — расточительство.
        if (now - (this.#speakers[author] ?? 0) > 200) {
          this.#speakers = { ...this.#speakers, [author]: now };
        }
      });
      await this.#playback.listen((message) => (this.status = message));
      // Возвращаем ранее настроенную громкость, чтобы не крутить её заново.
      for (const [author, value] of Object.entries(prefs.volumes)) {
        this.#playback.setVolume(author, value);
      }
      this.#playback.setMusicVolume(prefs.musicVolume);

      this.#capture = new Capture();
      await this.#capture.start({
        video: withVideo,
        onError: (message) => (this.status = message),
        onLevel: (rms) => this.#takeLevel(rms),
      });

      await api.joinCall(space, channel);

      this.stream = this.#capture.stream;
      this.active = true;
      this.space = space;
      this.channel = channel;
      this.camOn = withVideo;
      this.micMuted = false;
      this.status = '';
      this.#startPolling();
      // В комнате уже может кто-то транслировать — панель обязана появиться
      // сразу, а не через два удара сердца ведущего.
      await this.refreshPlayer();
    } catch (error) {
      // Не оставляем захват висеть, если на полпути что-то отвалилось:
      // иначе индикатор камеры продолжит гореть при отсутствующем звонке.
      this.status = errorText(error);
      await this.leave();
    }
  }

  async leave(): Promise<void> {
    // Первым делом отпускаем экран: выйти из звонка и остаться в полноэкранном
    // окне без единой кнопки — это тупик, из которого не видно выхода.
    await this.exitFullscreen().catch(() => undefined);
    // Трансляцию гасим до всего остального: захват чужого приложения, который
    // пережил выход из звонка, — это уже не функция, а слежка.
    if (this.sharingMusic) await this.stopMusic().catch(() => undefined);
    // Ни одна поломка при остановке не должна запереть человека в звонке.
    try {
      this.#capture?.stop();
    } catch {
      // всё равно выходим
    }
    try {
      this.#playback?.stop();
    } catch {
      // всё равно выходим
    }
    this.#capture = null;
    this.#playback = null;
    if (this.#poll !== null) {
      window.clearInterval(this.#poll);
      this.#poll = null;
    }
    this.#speakers = {};
    this.#rawLevel = 0;
    this.level = 0;
    this.player = null;
    this.sources = [];
    this.musicPicker = false;
    this.participants = [];
    this.stream = null;
    this.screens = [];
    this.screenOn = false;
    this.screenAudio = false;
    this.screenAudioAvailable = false;
    this.expanded = false;
    this.screenOnly = false;
    this.active = false;
    this.channel = null;
    this.space = null;
    this.camOn = false;

    await api.leaveCall().catch(() => undefined);
  }

  toggleMic(): void {
    this.micMuted = !this.micMuted;
    this.#capture?.setMuted(this.micMuted);
    // Выключенный микрофон отдаёт тишину, и полоска сползёт сама — но не сразу,
    // а за десяток блоков. Заметно: кнопку нажали, индикатор ещё живой.
    if (this.micMuted) {
      this.#rawLevel = 0;
      this.level = 0;
    }
  }

  /**
   * Выдать ключевой кадр по просьбе ядра — в звонке появился новый собеседник.
   *
   * Без этого он смотрел на чёрный прямоугольник до ближайшего кадра по
   * расписанию, то есть несколько секунд.
   */
  requestKeyframe(): void {
    this.#capture?.forceKeyframe();
  }

  /** Битрейт, подобранный ядром под реальный исходящий канал. */
  applyBitrate(track: string, bps: number): void {
    if (track !== 'video' && track !== 'screen') return;
    this.#capture?.setBitrate(track, bps);
  }

  /** Камеру включаем пересбором захвата: кодировщик настраивается один раз. */
  async toggleCamera(): Promise<void> {
    if (!this.active || !this.space || !this.channel) return;
    const space = this.space;
    const channel = this.channel;
    const next = !this.camOn;

    try {
      this.#capture?.stop();
    } catch {
      // Старый захват мог уже развалиться — это не повод не включать камеру.
    }
    this.#capture = new Capture();
    try {
      await this.#capture.start({
        video: next,
        onError: (message) => (this.status = message),
        onLevel: (rms) => this.#takeLevel(rms),
      });
      this.#capture.setMuted(this.micMuted);
      this.stream = this.#capture.stream;
      this.camOn = next;
      this.space = space;
      this.channel = channel;
    } catch (error) {
      this.status = errorText(error);
    }
  }

  /** Состав комнаты приходит из присутствия, а оно обновляется не мгновенно. */
  #startPolling(): void {
    const tick = async () => {
      try {
        const state = await api.callState();
        if (!state.channel) return;

        // Ушедшим освобождаем декодеры, иначе за долгий звонок с текучкой
        // их накопится по паре на каждого, кто когда-либо заходил.
        const present = new Set(state.participants);
        for (const previous of this.participants) {
          if (!present.has(previous.id)) {
            this.#playback?.forget(previous.id);
            delete this.#speakers[previous.id];
          }
        }

        this.participants = state.participants.map((id) => ({
          id,
          spokeAt: this.#speakers[id] ?? 0,
        }));
        this.screens = this.#playback?.sharingScreen() ?? [];
        // Демонстрацию сняли — держать полный экран больше не на чем, там
        // остался бы чёрный прямоугольник во весь монитор.
        if (this.fullscreen && this.screens.length === 0) void this.exitFullscreen();
      } catch {
        // Молча: звонок важнее, чем точность списка на одном тике.
      }
    };
    void tick();
    this.#poll = window.setInterval(tick, 1500);
  }
}

export const call = new Call();
