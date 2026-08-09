// Захват микрофона вне главного потока.
//
// Раньше здесь стоял ScriptProcessorNode. Он объявлен устаревшим больше десяти
// лет назад и — что важнее — работает **в главном потоке**: любая перерисовка
// ленты, досинхронизация истории или тяжёлый кадр видео заставляли его
// пропустить блок. Это и есть та самая «хрипота, когда что-то происходит».
//
// AudioWorklet живёт в потоке аудиорендера, где ему никто не мешает. Заодно
// уходит лишняя задержка: ScriptProcessor отдавал 2048 сэмплов, то есть 42.7 мс,
// а Opus всё равно режет их на кадры по 20 мс. Отдаём ровно по 20 мс — столько,
// сколько кодек и просит, ни миллисекундой больше.
//
// Файл лежит в `public/` намеренно: CSP приложения разрешает скрипты только со
// своего origin, поэтому blob-URL сюда не годится, а отдельный файл — годится.

/** 20 мс при 48 кГц — родной размер кадра Opus. */
const DEFAULT_BLOCK = 960;

class CaptureProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    this.blockSize = options?.processorOptions?.blockSize ?? DEFAULT_BLOCK;
    this.buffer = new Float32Array(this.blockSize);
    this.filled = 0;
  }

  process(inputs) {
    const channel = inputs[0]?.[0];
    // Дорожки может не быть какой-то один квант — это не повод останавливаться:
    // вернув false, процессор больше никогда не позовут, и звук пропадёт молча.
    if (!channel) return true;

    let offset = 0;
    while (offset < channel.length) {
      const take = Math.min(this.blockSize - this.filled, channel.length - offset);
      this.buffer.set(channel.subarray(offset, offset + take), this.filled);
      this.filled += take;
      offset += take;

      if (this.filled === this.blockSize) {
        // Отдаём владение буфером, а не копию: перенос вместо структурного
        // клонирования экономит копирование пятидесяти блоков в секунду.
        const block = this.buffer;
        this.port.postMessage(block, [block.buffer]);
        this.buffer = new Float32Array(this.blockSize);
        this.filled = 0;
      }
    }
    return true;
  }
}

registerProcessor('bred-capture', CaptureProcessor);
