// Судия Гундир: первый босс, который учит играть.
//
// Симуляция отделена от рисования намеренно. Здесь нет ни одного обращения к
// холсту и к DOM: на входе нажатые клавиши и прошедшее время, на выходе — мир.
// Поэтому бой можно прогнать без окна и проверить, что он вообще проходим.

import { ATTACKS, type Attack } from './attacks';
import {
  ARENA,
  angleTo,
  clampToArena,
  insideShape,
  len,
  sub,
  turnTowards,
  type Shape,
  type Vec,
} from './shapes';

export { ARENA } from './shapes';
export type { Shape, Vec } from './shapes';

const PLAYER = {
  r: 13,
  speed: 205,
  hp: 100,
  stamina: 100,
  regen: 32,
  /** Пауза перед восстановлением: махать без остановки нельзя. */
  regenDelay: 0.45,
  roll: { time: 0.42, iframes: 0.3, speed: 470, cost: 26 },
  swing: { windup: 0.14, active: 0.1, recover: 0.24, cost: 20, damage: 55, reach: 52, spread: 1.5 },
  estus: { charges: 3, time: 0.9, heal: 42 },
  /** Неуязвимость после пропущенного удара, чтобы не умереть с одного касания. */
  mercy: 0.45,
};

const BOSS = {
  r: 26,
  hp: 900,
  speed: 92,
  /** Во второй фазе он быстрее — и по ногам, и по рукам. */
  rage: 1.22,
  turn: 2.6,
};

export type Phase = 'бой' | 'превращение' | 'победа' | 'смерть';

interface Boss {
  at: Vec;
  facing: number;
  hp: number;
  phase: 1 | 2;
  attack: Attack | null;
  swing: number;
  timer: number;
  stage: 'подход' | 'замах' | 'удар' | 'отдых';
  cooldown: number;
  /** Куда нацелен текущий замах: он выбирается в начале и дальше не следит. */
  aim: Vec;
}

export interface Player {
  at: Vec;
  facing: number;
  hp: number;
  stamina: number;
  estus: number;
  state: 'свободен' | 'перекат' | 'удар' | 'лечится';
  timer: number;
  /** Неуязвимость: перекат и есть основная защита. */
  iframes: number;
  roll: Vec;
  hit: boolean;
}

export interface World {
  player: Player;
  boss: Boss;
  phase: Phase;
  /** Что показать поверх пола: замахи и вспышки попаданий. */
  tells: { shape: Shape; progress: number; hot: boolean }[];
  message: string;
  time: number;
}

export interface Input {
  up: boolean;
  down: boolean;
  left: boolean;
  right: boolean;
  roll: boolean;
  attack: boolean;
  heal: boolean;
}

export const noInput: Input = {
  up: false,
  down: false,
  left: false,
  right: false,
  roll: false,
  attack: false,
  heal: false,
};

export const maxHp = (): number => PLAYER.hp;
export const maxStamina = (): number => PLAYER.stamina;
export const bossMaxHp = (): number => BOSS.hp;
export const playerRadius = (): number => PLAYER.r;
export const bossRadius = (): number => BOSS.r;

export function newWorld(): World {
  return {
    player: {
      at: { x: ARENA.x, y: ARENA.y + 160 },
      facing: -Math.PI / 2,
      hp: PLAYER.hp,
      stamina: PLAYER.stamina,
      estus: PLAYER.estus.charges,
      state: 'свободен',
      timer: 0,
      iframes: 0,
      roll: { x: 0, y: 0 },
      hit: false,
    },
    boss: {
      at: { x: ARENA.x, y: ARENA.y - 120 },
      facing: Math.PI / 2,
      hp: BOSS.hp,
      phase: 1,
      attack: null,
      swing: 0,
      timer: 0,
      stage: 'подход',
      cooldown: 0.9,
      aim: { x: ARENA.x, y: ARENA.y },
    },
    phase: 'бой',
    tells: [],
    message: 'подойди и ударь',
    time: 0,
  };
}

function startPlayerAction(world: World, input: Input): void {
  const p = world.player;
  if (p.state !== 'свободен') return;

  if (input.roll && p.stamina >= PLAYER.roll.cost) {
    const dir = { x: (input.right ? 1 : 0) - (input.left ? 1 : 0), y: (input.down ? 1 : 0) - (input.up ? 1 : 0) };
    const distance = len(dir);
    // Без направления катимся назад от босса: так и задумано в первоисточнике.
    p.roll = distance > 0
      ? { x: dir.x / distance, y: dir.y / distance }
      : { x: -Math.cos(p.facing), y: -Math.sin(p.facing) };
    p.state = 'перекат';
    p.timer = PLAYER.roll.time;
    p.iframes = PLAYER.roll.iframes;
    p.stamina -= PLAYER.roll.cost;
    return;
  }
  if (input.attack && p.stamina >= PLAYER.swing.cost) {
    p.state = 'удар';
    p.timer = PLAYER.swing.windup + PLAYER.swing.active + PLAYER.swing.recover;
    p.hit = false;
    p.stamina -= PLAYER.swing.cost;
    return;
  }
  if (input.heal && p.estus > 0 && p.hp < PLAYER.hp) {
    p.state = 'лечится';
    p.timer = PLAYER.estus.time;
    p.estus -= 1;
  }
}

function movePlayer(world: World, input: Input, dt: number): void {
  const p = world.player;
  if (p.state === 'перекат') {
    p.at.x += p.roll.x * PLAYER.roll.speed * dt;
    p.at.y += p.roll.y * PLAYER.roll.speed * dt;
    clampToArena(p.at, PLAYER.r);
    return;
  }
  if (p.state !== 'свободен') return; // удар и глоток держат на месте

  const dir = { x: (input.right ? 1 : 0) - (input.left ? 1 : 0), y: (input.down ? 1 : 0) - (input.up ? 1 : 0) };
  const distance = len(dir);
  if (distance === 0) return;
  p.at.x += (dir.x / distance) * PLAYER.speed * dt;
  p.at.y += (dir.y / distance) * PLAYER.speed * dt;
  clampToArena(p.at, PLAYER.r);
}

function playerSwing(world: World): void {
  const p = world.player;
  const shape: Shape = {
    kind: 'arc',
    x: p.at.x,
    y: p.at.y,
    r: PLAYER.swing.reach,
    facing: p.facing,
    spread: PLAYER.swing.spread,
  };
  world.tells.push({ shape, progress: 1, hot: true });
  if (insideShape(world.boss.at, BOSS.r, shape)) {
    world.boss.hp -= PLAYER.swing.damage;
    p.hit = true;
  }
}

function chooseAttack(boss: Boss, distance: number): Attack | null {
  const fits = ATTACKS.filter(
    (a) => (a.phase === 0 || a.phase === boss.phase) && distance >= a.from && distance <= a.to,
  );
  if (fits.length === 0) return null;
  return fits[Math.floor(Math.random() * fits.length)];
}

function driveBoss(world: World, dt: number): void {
  const boss = world.boss;
  const player = world.player;
  const speed = boss.phase === 2 ? BOSS.speed * BOSS.rage : BOSS.speed;
  const haste = boss.phase === 2 ? 1 / BOSS.rage : 1;
  const distance = len(sub(player.at, boss.at));

  // Доворачивается всегда, но медленно: уйти ему за спину — рабочая тактика.
  boss.facing = turnTowards(boss.facing, angleTo(boss.at, player.at), BOSS.turn * dt);

  if (boss.stage === 'подход') {
    boss.cooldown -= dt;
    if (distance > 78) {
      boss.at.x += Math.cos(boss.facing) * speed * dt;
      boss.at.y += Math.sin(boss.facing) * speed * dt;
      clampToArena(boss.at, BOSS.r);
    }
    if (boss.cooldown <= 0) {
      const attack = chooseAttack(boss, distance);
      if (attack) {
        boss.attack = attack;
        boss.swing = 0;
        boss.stage = 'замах';
        boss.timer = attack.swings[0].windup * haste;
        boss.aim = { ...player.at };
        world.message = attack.name;
      }
    }
    return;
  }

  const attack = boss.attack;
  if (!attack) {
    boss.stage = 'подход';
    return;
  }
  const swing = attack.swings[boss.swing];
  boss.timer -= dt;

  if (boss.stage === 'замах') {
    const total = swing.windup * haste;
    if (swing.step) {
      boss.at.x += Math.cos(boss.facing) * (swing.step / total) * dt;
      boss.at.y += Math.sin(boss.facing) * (swing.step / total) * dt;
      clampToArena(boss.at, BOSS.r);
    }
    world.tells.push({
      shape: swing.shape(boss, boss.aim),
      progress: 1 - Math.max(0, boss.timer) / total,
      hot: false,
    });
    if (boss.timer <= 0) {
      const shape = swing.shape(boss, boss.aim);
      world.tells.push({ shape, progress: 1, hot: true });
      if (player.iframes <= 0 && insideShape(player.at, PLAYER.r, shape)) {
        player.hp -= swing.damage;
        player.iframes = PLAYER.mercy;
        // Из переката и глотка выбивает: за жадность платят.
        player.state = 'свободен';
        player.timer = 0;
      }
      boss.stage = 'удар';
      boss.timer = 0.12;
    }
    return;
  }

  if (boss.stage === 'удар' && boss.timer <= 0) {
    boss.swing += 1;
    if (boss.swing < attack.swings.length) {
      boss.stage = 'замах';
      boss.timer = attack.swings[boss.swing].windup * haste;
      boss.aim = { ...player.at };
    } else {
      boss.stage = 'отдых';
      boss.timer = attack.recover * haste;
    }
    return;
  }

  if (boss.stage === 'отдых' && boss.timer <= 0) {
    boss.stage = 'подход';
    boss.attack = null;
    boss.cooldown = (boss.phase === 2 ? 0.3 : 0.55) + Math.random() * 0.5;
  }
}

/** Один кадр мира. `dt` — секунды, ограничены сверху: вкладка могла спать. */
export function step(world: World, input: Input, dt: number): World {
  if (world.phase === 'победа' || world.phase === 'смерть') return world;
  dt = Math.min(dt, 0.05);
  world.time += dt;
  world.tells = [];

  const p = world.player;
  const boss = world.boss;
  p.facing = angleTo(p.at, boss.at); // взгляд всегда на босса, как при захвате цели

  p.iframes = Math.max(0, p.iframes - dt);

  if (p.state === 'свободен') {
    p.stamina = Math.min(PLAYER.stamina, p.stamina + PLAYER.regen * dt);
  }

  if (p.state !== 'свободен') {
    const before = p.timer;
    p.timer -= dt;
    if (p.state === 'удар') {
      const swung = PLAYER.swing.windup + PLAYER.swing.active + PLAYER.swing.recover;
      const elapsed = swung - p.timer;
      if (!p.hit && elapsed >= PLAYER.swing.windup) playerSwing(world);
    }
    if (p.state === 'лечится' && before > 0 && p.timer <= 0) {
      p.hp = Math.min(PLAYER.hp, p.hp + PLAYER.estus.heal);
    }
    if (p.timer <= 0) {
      p.state = 'свободен';
      p.timer = 0;
      // Пауза перед восстановлением выносливости — за неё и наказывают.
      p.stamina = Math.max(0, p.stamina - PLAYER.regen * PLAYER.regenDelay);
    }
  }

  startPlayerAction(world, input);
  movePlayer(world, input, dt);

  // Превращение: ровно на половине здоровья и ровно один раз.
  if (world.phase === 'бой' && boss.phase === 1 && boss.hp <= BOSS.hp / 2) {
    world.phase = 'превращение';
    boss.stage = 'отдых';
    boss.attack = null;
    boss.timer = 2.0;
    world.message = 'судия поднимается';
  } else if (world.phase === 'превращение') {
    boss.timer -= dt;
    if (boss.timer <= 0) {
      boss.phase = 2;
      world.phase = 'бой';
      boss.stage = 'подход';
      boss.cooldown = 0.25;
      const burst: Shape = { kind: 'circle', x: boss.at.x, y: boss.at.y, r: 150 };
      world.tells.push({ shape: burst, progress: 1, hot: true });
      if (p.iframes <= 0 && insideShape(p.at, PLAYER.r, burst)) {
        p.hp -= 30;
        p.iframes = PLAYER.mercy;
      }
    } else {
      world.tells.push({
        shape: { kind: 'circle', x: boss.at.x, y: boss.at.y, r: 150 },
        progress: 1 - boss.timer / 2.0,
        hot: false,
      });
    }
  } else {
    driveBoss(world, dt);
  }

  if (boss.hp <= 0) {
    world.phase = 'победа';
    world.message = 'ВРАГ ПОВЕРЖЕН';
  } else if (p.hp <= 0) {
    world.phase = 'смерть';
    world.message = 'ВЫ ПОГИБЛИ';
  }
  return world;
}
