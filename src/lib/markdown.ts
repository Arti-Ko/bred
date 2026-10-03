// Описание выпуска — Markdown из релиза на GitHub. Разбираем сами и только
// то, что там бывает: заголовки, списки, цитаты, жирный, код, ссылки.
//
// Не через innerHTML: текст приезжает из сети, и строка разметки в нём не
// должна превращаться в разметку страницы. Здесь получаются данные, а
// элементы из них строит Svelte.

export interface Inline {
  text: string;
  bold?: boolean;
  code?: boolean;
  /** Ссылка показывается текстом с адресом в подсказке: переход по ней увёл
   *  бы окно приложения из самого приложения. */
  href?: string;
}

export interface Block {
  kind: 'h1' | 'h2' | 'h3' | 'p' | 'li' | 'quote' | 'rule';
  inline: Inline[];
  /** Номер пункта нумерованного списка. */
  number?: number;
}

const INLINE = /(\*\*[^*]+\*\*|`[^`]+`|\[[^\]]+\]\([^)\s]+\))/g;

export function inline(text: string): Inline[] {
  const out: Inline[] = [];
  for (const piece of text.split(INLINE)) {
    if (!piece) continue;
    if (piece.startsWith('**') && piece.endsWith('**') && piece.length > 4) {
      out.push({ text: piece.slice(2, -2), bold: true });
    } else if (piece.startsWith('`') && piece.endsWith('`') && piece.length > 2) {
      out.push({ text: piece.slice(1, -1), code: true });
    } else {
      const link = /^\[([^\]]+)\]\(([^)\s]+)\)$/.exec(piece);
      out.push(link ? { text: link[1], href: link[2] } : { text: piece });
    }
  }
  return out;
}

export function parse(markdown: string): Block[] {
  const blocks: Block[] = [];
  let paragraph: string[] = [];
  const flush = () => {
    if (paragraph.length > 0) blocks.push({ kind: 'p', inline: inline(paragraph.join('\n')) });
    paragraph = [];
  };

  for (const raw of markdown.replace(/\r\n/g, '\n').split('\n')) {
    const line = raw.trimEnd();
    const trimmed = line.trim();
    if (!trimmed) {
      flush();
      continue;
    }
    const heading = /^(#{1,3})\s+(.*)$/.exec(trimmed);
    const bullet = /^[-*•]\s+(.*)$/.exec(trimmed);
    const numbered = /^(\d+)[.)]\s+(.*)$/.exec(trimmed);
    if (heading) {
      flush();
      const level = heading[1].length as 1 | 2 | 3;
      blocks.push({ kind: `h${level}`, inline: inline(heading[2]) });
    } else if (/^(-{3,}|\*{3,}|_{3,})$/.test(trimmed)) {
      flush();
      blocks.push({ kind: 'rule', inline: [] });
    } else if (bullet) {
      flush();
      blocks.push({ kind: 'li', inline: inline(bullet[1]) });
    } else if (numbered) {
      flush();
      blocks.push({ kind: 'li', inline: inline(numbered[2]), number: Number(numbered[1]) });
    } else if (trimmed.startsWith('>')) {
      flush();
      blocks.push({ kind: 'quote', inline: inline(trimmed.replace(/^>\s?/, '')) });
    } else {
      paragraph.push(trimmed);
    }
  }
  flush();
  return blocks;
}
