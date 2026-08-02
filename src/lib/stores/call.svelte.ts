// Состояние звонка. Отдельно от общей сессии: у звонка своя частота изменений
// (кадры идут десятками в секунду) и своё время жизни.

import { api, errorText, type Id } from '../ipc';
import { Capture, missingCapabilities, Playback } from '../media';

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
  /** Кто из собеседников показывает экран. */
  screens = $state<Id[]>([]);
  participants = $state<Participant[]>([]);
  status = $state('');
  /** Чего не хватает платформе. Непусто — звонки недоступны, и надо сказать честно. */
  gaps = $state<string[]>([]);

  #capture: Capture | null = null;
  #playback: Playback | null = null;
  #poll: number | null = null;
  #speakers = new Map<Id, number>();

  get localStream(): MediaStream | null {
    return this.#capture?.stream ?? null;
  }

  /** Говорит ли участник прямо сейчас (звук приходил меньше 400 мс назад). */
  speaking(id: Id): boolean {
    const at = this.#speakers.get(id);
    return at !== undefined && Date.now() - at < 400;
  }

  attachCanvas(author: Id, canvas: HTMLCanvasElement | null, track: 'video' | 'screen' = 'video'): void {
    this.#playback?.attachCanvas(author, canvas, track);
  }

  /** Показать или убрать демонстрацию своего экрана. */
  async toggleScreen(): Promise<void> {
    if (!this.active || !this.#capture) return;
    try {
      if (this.screenOn) {
        this.#capture.stopScreen();
        this.screenOn = false;
      } else {
        await this.#capture.startScreen((message) => (this.status = message));
        this.screenOn = this.#capture.sharingScreen;
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

      this.#capture = new Capture();
      await this.#capture.start({
        video: withVideo,
        onError: (message) => (this.status = message),
      });

      await api.joinCall(space, channel);

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
    this.screens = [];
    this.screenOn = false;
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
