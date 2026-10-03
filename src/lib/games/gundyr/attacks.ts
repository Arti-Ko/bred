// Мувсет Судии Гундира.
//
// Разбор настоящего боя, а не выдумка: в первой фазе он дерётся алебардой и
// (это главное) **парируется** — почти вся связка ловится на парирование, и
// именно этим его проходят те, кто умеет. Во второй из него лезет бездна: удары
// становятся длиннее, добавляется захват, а окно после связки — короче.
//
// Каждый приём это список взмахов. У взмаха свой замах, своя фигура и свой урон;
// парируемость помечена отдельно — у хвата и у волны её нет, как и в оригинале.

import type { Shape, Vec } from './shapes';

/** Всё, что приёму нужно знать о том, кто его делает. */
export interface Attacker {
  at: Vec;
  facing: number;
  phase: 1 | 2;
}

export interface Swing {
  /** Замах: ровно столько времени видно, куда прилетит. */
  windup: number;
  damage: number;
  /** Сколько стойкости снимает блок. */
  guard: number;
  shape: (boss: Attacker, aim: Vec) => Shape;
  /** Шаг вперёд на замахе — выпады и рывки. */
  step?: number;
  /** Ловится ли на парирование. */
  parryable?: boolean;
  /** Захват: сквозь блок, и перекат от него нужен точный. */
  grab?: boolean;
}

export interface Attack {
  name: string;
  swings: Swing[];
  /** Отдых после связки — то самое окно, в которое бьют. */
  recover: number;
  /** На каком расстоянии приём имеет смысл. */
  from: number;
  to: number;
  /** 0 — в обеих фазах. */
  phase: 0 | 1 | 2;
  /** Вес при выборе: чем больше, тем чаще. */
  weight: number;
}

const arc = (r: number, spread: number) => (boss: Attacker): Shape => ({
  kind: 'arc',
  x: boss.at.x,
  y: boss.at.y,
  r,
  facing: boss.facing,
  spread,
});

const line = (len: number, half: number) => (boss: Attacker): Shape => ({
  kind: 'line',
  x: boss.at.x,
  y: boss.at.y,
  facing: boss.facing,
  len,
  half,
});

export const ATTACKS: Attack[] = [
  {
    // Два горизонтальных взмаха подряд — то, чем он встречает чаще всего.
    name: 'связка из двух',
    from: 0,
    to: 120,
    phase: 0,
    weight: 3,
    recover: 0.55,
    swings: [
      { windup: 0.62, damage: 92, guard: 38, parryable: true, shape: arc(112, 2.0) },
      { windup: 0.46, damage: 96, guard: 40, parryable: true, shape: arc(120, 2.4) },
    ],
  },
  {
    // Три взмаха: последний самый долгий и самый наказуемый.
    name: 'связка из трёх',
    from: 0,
    to: 115,
    phase: 0,
    weight: 2,
    recover: 0.72,
    swings: [
      { windup: 0.55, damage: 86, guard: 34, parryable: true, shape: arc(108, 1.9) },
      { windup: 0.42, damage: 90, guard: 36, parryable: true, shape: arc(114, 2.2) },
      { windup: 0.66, damage: 118, guard: 52, parryable: true, shape: arc(126, 2.6) },
    ],
  },
  {
    // Удар сверху: бьёт по точке, где вы стояли в начале замаха.
    name: 'удар сверху',
    from: 0,
    to: 150,
    phase: 0,
    weight: 2,
    recover: 0.62,
    swings: [
      {
        windup: 0.78,
        damage: 128,
        guard: 58,
        step: 32,
        parryable: true,
        shape: (_boss, aim) => ({ kind: 'circle', x: aim.x, y: aim.y, r: 72 }),
      },
    ],
  },
  {
    // Выпад с шагом: узкий, но длинный — от него уходят вбок, а не назад.
    name: 'выпад',
    from: 90,
    to: 290,
    phase: 0,
    weight: 2,
    recover: 0.58,
    swings: [
      { windup: 0.66, damage: 112, guard: 48, step: 160, parryable: true, shape: line(200, 26) },
    ],
  },
  {
    // Пинок. В оригинале он для тех, кто стоит за щитом, — и здесь так же:
    // ломает блок, но урона почти не наносит.
    name: 'пинок',
    from: 0,
    to: 80,
    phase: 0,
    weight: 1,
    recover: 0.5,
    swings: [{ windup: 0.5, damage: 28, guard: 120, shape: arc(74, 1.1) }],
  },
  {
    // Круговой: единственный приём первой фазы, от которого не спрятаться за
    // спину — уходят перекатом наружу.
    name: 'круговой',
    from: 0,
    to: 130,
    phase: 0,
    weight: 1,
    recover: 0.78,
    swings: [
      {
        windup: 0.92,
        damage: 124,
        guard: 56,
        shape: (boss) => ({ kind: 'circle', x: boss.at.x, y: boss.at.y, r: 136 }),
      },
    ],
  },
  {
    // Рывок через арену, если вы отбежали.
    name: 'рывок с ударом',
    from: 200,
    to: 420,
    phase: 0,
    weight: 2,
    recover: 0.6,
    swings: [
      { windup: 0.8, damage: 106, guard: 46, step: 260, parryable: true, shape: arc(118, 1.6) },
    ],
  },

  // ── вторая фаза ───────────────────────────────────────────────────────────
  {
    name: 'хлыст',
    from: 110,
    to: 340,
    phase: 2,
    weight: 3,
    recover: 0.45,
    swings: [
      { windup: 0.5, damage: 104, guard: 44, shape: line(310, 22) },
      { windup: 0.44, damage: 108, guard: 46, shape: arc(160, 2.1) },
    ],
  },
  {
    // Захват. Сквозь блок, парировать нельзя, урон страшный — только перекат.
    name: 'захват',
    from: 0,
    to: 100,
    phase: 2,
    weight: 2,
    recover: 0.8,
    swings: [
      { windup: 0.7, damage: 268, guard: 999, grab: true, step: 52, shape: arc(94, 1.0) },
    ],
  },
  {
    name: 'прыжок сверху',
    from: 120,
    to: 320,
    phase: 2,
    weight: 2,
    recover: 0.68,
    swings: [
      {
        windup: 0.86,
        damage: 146,
        guard: 64,
        step: 200,
        shape: (_boss, aim) => ({ kind: 'circle', x: aim.x, y: aim.y, r: 88 }),
      },
    ],
  },
  {
    name: 'волна бездны',
    from: 0,
    to: 200,
    phase: 2,
    weight: 2,
    recover: 0.82,
    swings: [
      {
        windup: 0.95,
        damage: 132,
        guard: 60,
        shape: (boss) => ({ kind: 'circle', x: boss.at.x, y: boss.at.y, r: 178 }),
      },
    ],
  },
];
