// Разбор связи в звонке: какая линия хорошая, какая плохая, и у кого беда.
//
// Ядро присылает свои замеры и чужие отчёты. Здесь из них собирается сетка
// «каждый с каждым» и вывод человеческими словами. Главный вопрос — на чьей
// стороне проблема: если человеку плохо со всеми, дело в нём; если плохо
// только между двумя, а остальным с каждым из них нормально, — в их линии.

import type { Id, LinkReport } from './ipc';

export type Grade = 'good' | 'fair' | 'bad' | 'none';

/** Пороги задержки, мс: до неё разговор ещё не замечает задержки. */
const GOOD_RTT = 150;
const FAIR_RTT = 300;
/** Пороги потерь, проценты: Opus прячет пару процентов, дальше слышно. */
const GOOD_LOSS = 3;
const FAIR_LOSS = 8;

export interface Line {
  a: Id;
  b: Id;
  rtt: number;
  loss: number;
  relay: boolean;
  grade: Grade;
}

export function grade(rtt: number, loss: number): Grade {
  if (rtt <= 0) return 'none';
  if (rtt < GOOD_RTT && loss < GOOD_LOSS) return 'good';
  if (rtt < FAIR_RTT && loss < FAIR_LOSS) return 'fair';
  return 'bad';
}

const RANK: Record<Grade, number> = { good: 0, fair: 1, bad: 2, none: 3 };

export function worse(x: Grade, y: Grade): Grade {
  return RANK[x] >= RANK[y] ? x : y;
}

function key(a: Id, b: Id): string {
  return a < b ? `${a}|${b}` : `${b}|${a}`;
}

/**
 * Сетка линий между всеми, кто в звонке. Одна линия видна с двух концов —
 * берём худшую из двух оценок: жаловаться есть на что, если плохо хоть одному.
 */
export function lines(report: LinkReport, me: Id, present: Id[]): Line[] {
  const here = new Set(present);
  const byPair = new Map<string, Line>();
  const add = (from: Id, link: { peer: Id; rtt_ms: number; loss: number; relay: boolean }): void => {
    if (!here.has(from) || !here.has(link.peer) || from === link.peer) return;
    const id = key(from, link.peer);
    const fresh: Line = {
      a: from,
      b: link.peer,
      rtt: link.rtt_ms,
      loss: link.loss,
      relay: link.relay,
      grade: grade(link.rtt_ms, link.loss),
    };
    const seen = byPair.get(id);
    if (!seen) {
      byPair.set(id, fresh);
      return;
    }
    byPair.set(id, {
      ...seen,
      rtt: Math.max(seen.rtt, fresh.rtt),
      loss: Math.max(seen.loss, fresh.loss),
      relay: seen.relay || fresh.relay,
      grade: worse(seen.grade, fresh.grade),
    });
  };
  for (const link of report.mine) add(me, link);
  for (const other of report.others) for (const link of other.links) add(other.from, link);

  // С кем у меня нет соединения вовсе — это тоже линия, и худшая.
  for (const id of present) {
    if (id !== me && !byPair.has(key(me, id))) {
      byPair.set(key(me, id), { a: me, b: id, rtt: 0, loss: 0, relay: false, grade: 'none' });
    }
  }
  return [...byPair.values()];
}

export interface Verdict {
  grade: Grade;
  /** Кто виноват, если это видно. */
  culprit: Id | null;
  text: string;
}

/** Вывод по сетке. `name` даёт имя участника, `me` — свой ключ. */
export function verdict(all: Line[], me: Id, present: Id[], name: (id: Id) => string): Verdict {
  const others = present.filter((id) => id !== me);
  if (others.length === 0) return { grade: 'good', culprit: null, text: 'В звонке пока только вы.' };

  const bad = all.filter((line) => line.grade === 'bad' || line.grade === 'none');
  const worst = all.reduce<Grade>((acc, line) => worse(acc, line.grade), 'good');
  if (bad.length === 0) {
    return worst === 'fair'
      ? { grade: 'fair', culprit: null, text: 'Связь терпимая: задержка заметна, но говорить можно.' }
      : { grade: 'good', culprit: null, text: 'Связь в порядке у всех.' };
  }

  // Кому плохо со всеми, с кем он вообще связан, — беда у него. Нужно хотя
  // бы два собеседника: при одном не понять, чей конец линии виноват.
  const suspects = present.filter((id) => {
    const mine = all.filter((line) => line.a === id || line.b === id);
    return mine.length >= 2 && mine.every((line) => line.grade === 'bad' || line.grade === 'none');
  });
  if (suspects.length === 1) {
    const [culprit] = suspects;
    return culprit === me
      ? { grade: 'bad', culprit, text: 'Похоже, проблема у вас: плохо со всеми. Проверьте интернет и VPN.' }
      : { grade: 'bad', culprit, text: `Похоже, проблема у ${name(culprit)}: плохо со всеми, остальным друг с другом нормально.` };
  }

  if (others.length === 1) {
    return {
      grade: 'bad',
      culprit: null,
      text: `Плохо между вами и ${name(others[0])}. Чей это конец — видно, когда в звонке есть третий.`,
    };
  }
  if (bad.length === 1) {
    const [line] = bad;
    const who = (id: Id): string => (id === me ? 'вами' : name(id));
    return {
      grade: 'bad',
      culprit: null,
      text: `Плохо только между ${who(line.a)} и ${who(line.b)}, у остальных связь нормальная.`,
    };
  }
  return { grade: 'bad', culprit: null, text: 'Связь плохая у нескольких — похоже, беда не у одного человека.' };
}

/** Короткое слово к оценке. */
export const GRADE_TEXT: Record<Grade, string> = {
  good: 'отлично',
  fair: 'терпимо',
  bad: 'плохо',
  none: 'нет связи',
};
