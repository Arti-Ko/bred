// Короткий сигнал о входящем сообщении.
//
// Звук синтезируем на месте, а не тащим файлом: во-первых, это два десятка
// строк вместо лишнего килобайта в сборке, во-вторых — так он звучит ровно так,
// как выглядит остальной интерфейс. Никаких «дзинь» из мессенджера: два
// коротких щелчка на затухании, как отклик терминала.

/** Тише этого — не услышишь, громче — раздражает уже на третьем сообщении. */
const GAIN = 0.08;

/** Длительность одного щелчка, секунды. */
const BLIP = 0.055;

/** Частоты двух щелчков, герцы. Второй выше — сигнал «пришло», а не «ошибка». */
const TONES = [880, 1320];

/**
 * Не чаще одного сигнала за этот срок, миллисекунды.
 *
 * Пачка из двадцати событий приезжает разом после переподключения, и без
 * ограничения это очередь из двадцати писков подряд.
 */
const COOLDOWN = 1500;

let ctx: AudioContext | null = null;
let last = 0;

/** Контекст создаём при первом звуке: до жеста человека браузер его усыпляет. */
function audio(): AudioContext | null {
  try {
    ctx ??= new AudioContext();
    if (ctx.state === 'suspended') void ctx.resume();
    return ctx;
  } catch {
    // Нет звукового устройства — не повод ронять уведомление целиком.
    return null;
  }
}

/** Пикнуть, если с прошлого раза прошло достаточно времени. */
export function chime(): void {
  const now = Date.now();
  if (now - last < COOLDOWN) return;
  last = now;

  const context = audio();
  if (!context) return;

  TONES.forEach((frequency, index) => {
    const at = context.currentTime + index * BLIP * 1.4;
    const osc = context.createOscillator();
    const gain = context.createGain();

    osc.type = 'square';
    osc.frequency.value = frequency;

    // Ступенька по громкости щёлкает по динамику, поэтому затухаем плавно.
    gain.gain.setValueAtTime(GAIN, at);
    gain.gain.exponentialRampToValueAtTime(0.0001, at + BLIP);

    osc.connect(gain).connect(context.destination);
    osc.start(at);
    osc.stop(at + BLIP);
  });
}
