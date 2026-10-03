// Звонок входящего зова в комнату.
//
// Синтезируем на месте, как и сигнал сообщения: два десятка строк вместо файла
// в сборке, и звучит так же, как выглядит интерфейс. Но характер другой — это
// не «пришло», а «зовут»: две пары тонов с паузой, повторяются, пока человек
// не ответит, не откажется или звонящий не даст отбой.

/** Громче сигнала сообщения: зов пропускать жалко, сообщение — нет. */
const GAIN = 0.11;
/** Длительность одного тона, секунды. */
const NOTE = 0.16;
/** Пара тонов «вверх» — узнаётся как звонок, а не как ошибка. */
const TONES = [660, 880];
/** Период повтора — как у телефона: звонок, пауза. */
const PERIOD = 2400;
/** Дольше этого не звоним: позвавший уже наверняка ушёл звать других. */
export const RING_LIMIT = 30_000;

export class Ringtone {
  #context: AudioContext | null = null;
  #timer: number | null = null;
  #stopAt = 0;

  get ringing(): boolean {
    return this.#timer !== null;
  }

  start(): void {
    this.#stopAt = Date.now() + RING_LIMIT;
    if (this.#timer !== null) return;
    try {
      this.#context ??= new AudioContext();
      if (this.#context.state !== 'running') void this.#context.resume().catch(() => undefined);
    } catch {
      return; // нет звукового устройства — карточка зова всё равно на экране
    }
    this.#burst();
    this.#timer = window.setInterval(() => {
      if (Date.now() >= this.#stopAt) this.stop();
      else this.#burst();
    }, PERIOD);
  }

  stop(): void {
    if (this.#timer !== null) window.clearInterval(this.#timer);
    this.#timer = null;
  }

  #burst(): void {
    const context = this.#context;
    if (!context) return;
    for (let pair = 0; pair < 2; pair += 1) {
      TONES.forEach((frequency, index) => {
        const at = context.currentTime + pair * NOTE * 2.6 + index * NOTE * 1.1;
        const osc = context.createOscillator();
        const gain = context.createGain();
        osc.type = 'triangle';
        osc.frequency.value = frequency;
        // Мягкие края: ступенька по громкости щёлкает по динамику.
        gain.gain.setValueAtTime(0.0001, at);
        gain.gain.exponentialRampToValueAtTime(GAIN, at + 0.012);
        gain.gain.exponentialRampToValueAtTime(0.0001, at + NOTE);
        osc.connect(gain).connect(context.destination);
        osc.start(at);
        osc.stop(at + NOTE);
      });
    }
  }
}
