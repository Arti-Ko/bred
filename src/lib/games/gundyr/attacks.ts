// Что умеет судия. Каждый приём — это список взмахов: у каждого свой замах,
// своя фигура и свой урон. Отдых после связки и есть то окно, в котором его
// бьют: сам он его не подарит.

import type { Shape, Vec } from './shapes';

/** Всё, что атаке нужно знать о том, кто её делает. */
export interface Attacker {
  at: Vec;
  facing: number;
  phase: 1 | 2;
}

export interface Swing {
  /** Замах: ровно столько времени видно, куда прилетит. */
  windup: number;
  damage: number;
  shape: (boss: Attacker, aim: Vec) => Shape;
  /** Шаг вперёд на замахе — выпады и рывки. */
  step?: number;
}

export interface Attack {
  name: string;
  swings: Swing[];
  recover: number;
  /** На каком расстоянии приём имеет смысл. */
  from: number;
  to: number;
  /** 0 — в обеих фазах. */
  phase: 0 | 1 | 2;
}

export const ATTACKS: Attack[] = [
  {
    name: 'занос алебарды',
    from: 0,
    to: 150,
    phase: 0,
    recover: 0.85,
    swings: [
      {
        windup: 0.72,
        damage: 32,
        step: 34,
        shape: (_boss, aim) => ({ kind: 'circle', x: aim.x, y: aim.y, r: 74 }),
      },
    ],
  },
  {
    name: 'связка на два взмаха',
    from: 0,
    to: 110,
    phase: 0,
    recover: 0.7,
    swings: [
      {
        windup: 0.5,
        damage: 24,
        shape: (boss) => ({
          kind: 'arc',
          x: boss.at.x,
          y: boss.at.y,
          r: 110,
          facing: boss.facing,
          spread: 1.9,
        }),
      },
      {
        windup: 0.42,
        damage: 26,
        shape: (boss) => ({
          kind: 'arc',
          x: boss.at.x,
          y: boss.at.y,
          r: 118,
          facing: boss.facing,
          spread: 2.3,
        }),
      },
    ],
  },
  {
    name: 'выпад',
    from: 90,
    to: 280,
    phase: 0,
    recover: 0.75,
    swings: [
      {
        windup: 0.62,
        damage: 30,
        step: 150,
        shape: (boss) => ({
          kind: 'line',
          x: boss.at.x,
          y: boss.at.y,
          facing: boss.facing,
          len: 190,
          half: 26,
        }),
      },
    ],
  },
  {
    name: 'круговой',
    from: 0,
    to: 130,
    phase: 0,
    recover: 1.05,
    swings: [
      {
        windup: 0.95,
        damage: 34,
        shape: (boss) => ({ kind: 'circle', x: boss.at.x, y: boss.at.y, r: 132 }),
      },
    ],
  },
  {
    name: 'плеть',
    from: 120,
    to: 330,
    phase: 2,
    recover: 0.6,
    swings: [
      {
        windup: 0.52,
        damage: 28,
        shape: (boss) => ({
          kind: 'line',
          x: boss.at.x,
          y: boss.at.y,
          facing: boss.facing,
          len: 300,
          half: 20,
        }),
      },
    ],
  },
  {
    name: 'захват',
    from: 0,
    to: 95,
    phase: 2,
    recover: 1.1,
    swings: [
      {
        windup: 0.66,
        damage: 52,
        step: 46,
        shape: (boss) => ({
          kind: 'arc',
          x: boss.at.x,
          y: boss.at.y,
          r: 92,
          facing: boss.facing,
          spread: 1.1,
        }),
      },
    ],
  },
];
