// Форматирование для терминального вида: всё моноширинное, значит ширина
// колонок предсказуема и её стоит держать постоянной.

/** Таймкод вида 14:05:12 — как строка лога. */
export function timecode(ts: number): string {
  const d = new Date(ts);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

/** Дата-разделитель ленты. */
export function dayStamp(ts: number): string {
  const d = new Date(ts);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${pad(d.getDate())}.${pad(d.getMonth() + 1)}.${d.getFullYear()}`;
}

/** «2ч», «1д» — компактно, чтобы влезало в узкую колонку участников. */
export function ago(ts: number): string {
  if (!ts) return '—';
  const seconds = Math.max(0, Math.floor((Date.now() - ts) / 1000));
  if (seconds < 60) return 'сейчас';
  if (seconds < 3600) return `${Math.floor(seconds / 60)}м`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}ч`;
  return `${Math.floor(seconds / 86400)}д`;
}

/** Разбор тела сообщения на текст и блоки кода по ограждению ```. */
export interface Chunk {
  code: boolean;
  text: string;
}

/** Кусок текста, разобранный на слова и свои эмодзи. */
export interface Token {
  emoji: string | null;
  text: string;
}

/**
 * Разбор `:имя:` в тексте. Имена берём из набора пространства: всё, чего в
 * наборе нет, остаётся обычным текстом — иначе двоеточия в коде и во времени
 * превращались бы в дырки.
 */
export function tokenize(text: string, known: Record<string, unknown>): Token[] {
  const out: Token[] = [];
  const pattern = /:([a-zA-Z0-9_-]{1,32}):/g;
  let last = 0;

  for (const match of text.matchAll(pattern)) {
    const name = match[1];
    if (!(name in known)) continue;
    if (match.index > last) out.push({ emoji: null, text: text.slice(last, match.index) });
    out.push({ emoji: name, text: match[0] });
    last = match.index + match[0].length;
  }
  if (last < text.length) out.push({ emoji: null, text: text.slice(last) });
  return out;
}

/** Сообщение целиком из одного стикера показывается крупно. */
export function loneSticker(body: string, known: Record<string, { sticker: boolean }>): string | null {
  const trimmed = body.trim();
  const match = /^:([a-zA-Z0-9_-]{1,32}):$/.exec(trimmed);
  if (!match) return null;
  const found = known[match[1]];
  return found?.sticker ? match[1] : null;
}

export function chunks(body: string): Chunk[] {
  const parts = body.split('```');
  return parts
    .map((text, index) => ({ code: index % 2 === 1, text: index % 2 ? text.replace(/^\n/, '') : text }))
    .filter((chunk) => chunk.text.length > 0);
}
