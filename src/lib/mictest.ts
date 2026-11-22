// Проверка микрофона: услышать себя, как в Дискорде.
//
// Микрофон открывается так же, как в звонке, — тем же устройством и с тем же
// шумоподавлением, эхоподавлением и выравниванием громкости, — и сразу играет
// в выбранные колонки. Так слышно то, что уходит собеседникам, кроме сжатия
// в Opus. Рядом — уровень сигнала, чтобы видеть, что микрофон вообще ловит.

import { MIC, openDevice } from './media';

/** Ниже — тишина: шум комнаты не должен шевелить полоску. */
const FLOOR_DB = -60;

type SinkCapable = { setSinkId(id: string): Promise<void> };

export class MicTest {
  #stream: MediaStream | null = null;
  #context: AudioContext | null = null;
  #element: HTMLAudioElement | null = null;
  #frame = 0;
  #stopped = false;

  /**
   * Начать. `micId` и `sinkId` пустые — системные устройства.
   * `onLevel` получает уровень от 0 до 1 каждый кадр экрана.
   */
  async start(micId: string, sinkId: string, onLevel: (level: number) => void): Promise<void> {
    const stream = await openDevice('audio', MIC, micId || undefined);
    if (this.#stopped) {
      stream.getTracks().forEach((track) => track.stop());
      return;
    }
    this.#stream = stream;

    const context = new AudioContext({ sampleRate: 48000, latencyHint: 'interactive' });
    this.#context = context;
    const source = context.createMediaStreamSource(stream);
    const analyser = context.createAnalyser();
    analyser.fftSize = 1024;
    source.connect(analyser);
    await this.#route(context, source, sinkId);
    if (context.state === 'suspended') await context.resume().catch(() => undefined);

    const samples = new Float32Array(analyser.fftSize);
    const tick = (): void => {
      if (this.#stopped) return;
      analyser.getFloatTimeDomainData(samples);
      let sum = 0;
      for (const sample of samples) sum += sample * sample;
      const db = 20 * Math.log10(Math.sqrt(sum / samples.length) || 1e-9);
      onLevel(Math.min(1, Math.max(0, (db - FLOOR_DB) / -FLOOR_DB)));
      this.#frame = requestAnimationFrame(tick);
    };
    tick();
  }

  /**
   * Куда играть. Как у звонка: у аудиоконтекста, если он умеет выбирать
   * колонки (Windows), иначе через элемент `<audio>` (мак).
   */
  async #route(context: AudioContext, source: AudioNode, sinkId: string): Promise<void> {
    if (!sinkId) {
      source.connect(context.destination);
      return;
    }
    if ('setSinkId' in context) {
      await (context as AudioContext & SinkCapable).setSinkId(sinkId).catch(() => undefined);
      source.connect(context.destination);
      return;
    }
    const bridge = context.createMediaStreamDestination();
    source.connect(bridge);
    const element = new Audio();
    element.srcObject = bridge.stream;
    this.#element = element;
    // Колонки не выбрались — пусть играет в системные, но играет.
    await (element as HTMLAudioElement & SinkCapable).setSinkId(sinkId).catch(() => undefined);
    await element.play();
  }

  stop(): void {
    this.#stopped = true;
    cancelAnimationFrame(this.#frame);
    if (this.#element) {
      this.#element.pause();
      this.#element.srcObject = null;
      this.#element = null;
    }
    this.#stream?.getTracks().forEach((track) => track.stop());
    this.#stream = null;
    void this.#context?.close().catch(() => undefined);
    this.#context = null;
  }
}
