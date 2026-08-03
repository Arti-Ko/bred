// Состояние звонка. Отдельно от общей сессии: у звонка своя частота изменений
// (кадры идут десятками в секунду) и своё время жизни.

import { api, errorText, type Id } from '../ipc';
import { Capture, missingCapabilities, Playback } from '../media';
import { prefs } from './prefs.svelte';

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
  /** Звонок во весь экран, и режим «только демонстрация». */
  expanded = $state(false);
  screenOnly = $state(false);
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

  #capture: Capture | null = null;
  #playback: Playback | null = null;
  #poll: number | null = null;
  #speakers = new Map<Id, number>();

  get localStream(): MediaStream | null {
    return this.stream;
  }

  /** Говорит ли участник прямо сейчас (звук приходил меньше 400 мс назад). */
  speaking(id: Id): boolean {
    const at = this.#speakers.get(id);
    return at !== undefined && Date.now() - at < 400;
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
    this.expanded = !this.expanded;
    if (!this.expanded) this.screenOnly = false;
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
        this.#speakers.set(author, Date.now());
      });
      await this.#playback.listen((message) => (this.status = message));
      // Возвращаем ранее настроенную громкость, чтобы не крутить её заново.
      for (const [author, value] of Object.entries(prefs.volumes)) {
        this.#playback.setVolume(author, value);
      }

      this.#capture = new Capture();
      await this.#capture.start({
        video: withVideo,
        onError: (message) => (this.status = message),
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
    } catch (error) {
      // Не оставляем захват висеть, если на полпути что-то отвалилось:
      // иначе индикатор камеры продолжит гореть при отсутствующем звонке.
      this.status = errorText(error);
      await this.leave();
    }
  }

  async leave(): Promise<void> {
    this.#capture?.stop();
    this.#playback?.stop();
    this.#capture = null;
    this.#playback = null;
    if (this.#poll !== null) {
      window.clearInterval(this.#poll);
      this.#poll = null;
    }
    this.#speakers.clear();
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
  }

  /** Камеру включаем пересбором захвата: кодировщик настраивается один раз. */
  async toggleCamera(): Promise<void> {
    if (!this.active || !this.space || !this.channel) return;
    const space = this.space;
    const channel = this.channel;
    const next = !this.camOn;

    this.#capture?.stop();
    this.#capture = new Capture();
    try {
      await this.#capture.start({
        video: next,
        onError: (message) => (this.status = message),
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
            this.#speakers.delete(previous.id);
          }
        }

        this.participants = state.participants.map((id) => ({
          id,
          spokeAt: this.#speakers.get(id) ?? 0,
        }));
        this.screens = this.#playback?.sharingScreen() ?? [];
      } catch {
        // Молча: звонок важнее, чем точность списка на одном тике.
      }
    };
    void tick();
    this.#poll = window.setInterval(tick, 1500);
  }
}

export const call = new Call();
