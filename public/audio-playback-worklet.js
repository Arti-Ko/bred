// Воспроизведение звонка вне главного потока.
//
// Раньше каждый звуковой кадр каждого собеседника декодировался в главном
// потоке и ставился в очередь отдельным узлом WebAudio — полсотни узлов в
// секунду на человека, рядом с перерисовкой ленты. Любая заминка интерфейса
// слышалась как щелчок или провал.
//
// Теперь приём, восстановление потерь и сведение — в ядре, а сюда приходит
// один готовый поток: двадцать миллисекунд стерео за раз. Ворклет живёт в
// потоке аудиорендера и держит небольшой запас, которого хватает пережить
// задумавшийся главный поток, не превращая разговор в рацию.
//
// Часы у ядра и у звуковой карты разные и расходятся на десятки миллионных
// долей. За час это десятые доли секунды — поэтому запас держится у нормы
// мягко: по одному отсчёту, пока он не вернётся в коридор.

/** Сколько ждём, прежде чем начать играть, — сорок миллисекунд. */
const PRIME = 1920;
/**
 * Докуда запас может вырасти — сто пятьдесят миллисекунд.
 *
 * Сорока миллисекунд хватает, пока главный поток свободен. Во время
 * демонстрации экрана он занят кадрами, особенно на маке, и задумывается
 * дольше — каждый раз это был провал в голосе. Теперь провал растит запас, а
 * тишина в ровном потоке медленно возвращает его к норме.
 */
const MAX_TARGET = 7200;
/** Через сколько квантов без провалов убавляем запас и на сколько. */
const SHRINK_AFTER = 750;
const SHRINK_BY = 240;
/** Коридор вокруг нормы, в пределах которого ничего не трогаем. */
const SLACK = 960;
/** Дальше этого запас уже задержка: выбрасываем лишнее сразу. */
const CEILING = 9600;
/** Потолок кольца — секунда стерео. */
const CAPACITY = 48000;
/** Подравниваем запас не чаще раза в столько квантов рендера. */
const NUDGE_EVERY = 8;

class PlaybackProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.ring = new Float32Array(CAPACITY * 2);
    this.read = 0;
    this.write = 0;
    this.size = 0;
    this.primed = false;
    this.quanta = 0;
    /** Нынешняя норма запаса: растёт на провалах, тает в покое. */
    this.target = PRIME;
    this.steady = 0;
    this.port.onmessage = (event) => this.take(event.data);
  }

  /** Кадр от ядра: `[0]` версия, `[1]` каналы, `[2..4]` отсчётов, дальше int16. */
  take(buffer) {
    if (!(buffer instanceof ArrayBuffer) || buffer.byteLength < 4) return;
    const head = new Uint8Array(buffer, 0, 4);
    if (head[0] !== 1) return;
    const channels = head[1];
    const frames = head[2] | (head[3] << 8);
    const samples = new Int16Array(buffer, 4, Math.min(frames * channels, (buffer.byteLength - 4) >> 1));

    for (let frame = 0; frame < frames; frame += 1) {
      if (this.size === CAPACITY) {
        // Кольцо полно — значит, давно никто не играет. Старое не нужно.
        this.read = (this.read + 1) % CAPACITY;
        this.size -= 1;
      }
      const left = samples[frame * channels] / 32768;
      const right = channels > 1 ? samples[frame * channels + 1] / 32768 : left;
      this.ring[this.write * 2] = left;
      this.ring[this.write * 2 + 1] = right;
      this.write = (this.write + 1) % CAPACITY;
      this.size += 1;
    }

    // Главный поток задумался и выдал накопленное разом: догонять бессмысленно,
    // отставание осталось бы навсегда.
    if (this.size > Math.max(CEILING, this.target * 2)) {
      const drop = this.size - this.target;
      this.read = (this.read + drop) % CAPACITY;
      this.size -= drop;
    }
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    const left = out[0];
    const right = out[1] ?? out[0];
    if (!left) return true;

    if (!this.primed) {
      if (this.size < this.target) {
        left.fill(0);
        if (right !== left) right.fill(0);
        return true;
      }
      this.primed = true;
    }

    // Мягкое выравнивание: лишний отсчёт пропускаем, недостающий повторяем.
    // По одному за несколько квантов — это доли процента, на слух неотличимо.
    this.quanta += 1;
    let nudge = 0;
    if (this.quanta >= NUDGE_EVERY) {
      this.quanta = 0;
      if (this.size > this.target + SLACK) nudge = 1;
      else if (this.size < this.target - SLACK && this.size > 0) nudge = -1;
    }
    if (nudge === 1) {
      this.read = (this.read + 1) % CAPACITY;
      this.size -= 1;
    }

    for (let i = 0; i < left.length; i += 1) {
      if (this.size === 0) {
        // Кончился запас — доигрываем тишиной и набираем его заново, а не
        // заикаемся по одному кадру.
        left[i] = 0;
        right[i] = 0;
        if (this.primed) {
          this.target = Math.min(MAX_TARGET, Math.round(this.target * 1.5));
          this.steady = 0;
        }
        this.primed = false;
        continue;
      }
      left[i] = this.ring[this.read * 2];
      right[i] = this.ring[this.read * 2 + 1];
      if (nudge === -1 && i === 0) continue; // этот отсчёт прозвучит дважды
      this.read = (this.read + 1) % CAPACITY;
      this.size -= 1;
    }
    if (this.primed) {
      this.steady += 1;
      if (this.steady >= SHRINK_AFTER && this.target > PRIME) {
        this.steady = 0;
        this.target = Math.max(PRIME, this.target - SHRINK_BY);
      }
    }
    return true;
  }
}

registerProcessor('bred-playback', PlaybackProcessor);
