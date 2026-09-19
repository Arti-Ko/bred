// Судия Гундир.
//
// Он сидит на коленях с мечом в груди и не шевелится, пока к нему не подойдут —
// бой начинается не по входу в комнату, а по пробуждению. Дальше он выбирает
// приём по расстоянию и весам, доводит связку до конца и отдыхает: отдых и есть
// то окно, в которое его бьют.
//
// Ломается он двумя способами. Первый — стойкость: несколько тяжёлых подряд, и
// он падает на колено. Второй — парирование: почти вся первая фаза ловится, и
// это самый быстрый способ снять половину здоровья.

import { ATTACKS, type Attack } from './attacks';
import { angleTo, clampToArena, turnTowards, type Vec } from './shapes';

export const BOSS = {
  r: 27,
  hp: 1700,
  speed: 114,
  /** Во второй фазе он быстрее — и по ногам, и по рукам. */
  rage: 1.24,
  turn: 2.4,
  /** Сколько стойкости надо снять, чтобы уронить его на колено. */
  poise: 220,
  /** Стойкость набирается обратно, если перестать бить. */
  poiseDecay: 45,
  /** Сколько он стоит открытым после парирования и после слома стойкости. */
  openParry: 2.4,
  openPoise: 1.7,
  /** Половина здоровья — и из него лезет бездна. */
  phaseAt: 0.55,
  wakeRange: 96,
  wakeTime: 1.1,
};

export type BossState =
  | 'спит'
  | 'пробуждается'
  | 'подход'
  | 'замах'
  | 'удар'
  | 'отдых'
  | 'открыт'
  | 'превращение';

export interface Boss {
  at: Vec;
  facing: number;
  hp: number;
  phase: 1 | 2;
  state: BossState;
  attack: Attack | null;
  swing: number;
  timer: number;
  cooldown: number;
  /** Куда нацелен текущий замах: он выбирается в начале и дальше не следит. */
  aim: Vec;
  poise: number;
  /** Чем его открыли: за парирование риспост бьёт больнее. */
  opening: 'нет' | 'парирование' | 'стойкость';
}

export function newBoss(at: Vec): Boss {
  return {
    at: { ...at },
    facing: Math.PI / 2,
    hp: BOSS.hp,
    phase: 1,
    state: 'спит',
    attack: null,
    swing: 0,
    timer: 0,
    cooldown: 0.7,
    aim: { ...at },
    poise: 0,
    opening: 'нет',
  };
}

/** Приём под расстояние и фазу, с учётом весов. */
export function choose(boss: Boss, distance: number): Attack | null {
  const fits = ATTACKS.filter(
    (a) => (a.phase === 0 || a.phase === boss.phase) && distance >= a.from && distance <= a.to,
  );
  if (fits.length === 0) return null;
  const total = fits.reduce((sum, a) => sum + a.weight, 0);
  let roll = Math.random() * total;
  for (const attack of fits) {
    roll -= attack.weight;
    if (roll <= 0) return attack;
  }
  return fits[fits.length - 1];
}

/** Урон по боссу: возвращает `true`, если этим ударом его уронили. */
export function wound(boss: Boss, damage: number, poise: number): boolean {
  boss.hp -= damage;
  if (boss.state === 'открыт' || boss.state === 'превращение') return false;
  boss.poise += poise;
  if (boss.poise < BOSS.poise) return false;
  boss.poise = 0;
  boss.state = 'открыт';
  boss.opening = 'стойкость';
  boss.timer = BOSS.openPoise;
  boss.attack = null;
  return true;
}

/** Парирование удалось: он открыт дольше, и риспост бьёт больнее. */
export function parried(boss: Boss): void {
  boss.state = 'открыт';
  boss.opening = 'парирование';
  boss.timer = BOSS.openParry;
  boss.attack = null;
  boss.poise = 0;
}

/** Поворот и шаги — всё, что он делает между приёмами. */
export function approach(boss: Boss, target: Vec, dt: number, haste: number): void {
  boss.facing = turnTowards(boss.facing, angleTo(boss.at, target), BOSS.turn * dt);
  const distance = Math.hypot(target.x - boss.at.x, target.y - boss.at.y);
  if (distance <= 82) return;
  const speed = (boss.phase === 2 ? BOSS.speed * BOSS.rage : BOSS.speed) * haste;
  boss.at.x += Math.cos(boss.facing) * speed * dt;
  boss.at.y += Math.sin(boss.facing) * speed * dt;
  clampToArena(boss.at, BOSS.r);
}
