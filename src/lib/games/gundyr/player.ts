// Пепел: тот, кто держит меч.
//
// Числа взяты с оглядкой на оригинал: перекат даёт неуязвимость примерно на
// треть своей длительности, выносливость восстанавливается с задержкой, а
// блок стоит стойкости и кончается, если держать его под связку целиком.

import { clampToArena, len, type Vec } from './shapes';

export const PLAYER = {
  r: 13,
  speed: 196,
  sprint: 300,
  hp: 450,
  stamina: 100,
  regen: 30,
  /** Пауза перед восстановлением: махать без остановки нельзя. */
  regenDelay: 0.55,
  roll: { time: 0.55, iframes: 0.38, speed: 430, cost: 22 },
  /** Лёгкий удар. Складывается в связку до трёх, если успевать. */
  light: { windup: 0.17, recover: 0.32, cost: 18, damage: 92, poise: 42, reach: 54, spread: 1.4 },
  /** Тяжёлый: долгий замах, зато ломает стойкость почти с двух. */
  heavy: { windup: 0.46, recover: 0.54, cost: 34, damage: 175, poise: 95, reach: 66, spread: 1.1 },
  /** Парирование: короткое окно, промах наказывается. */
  parry: { window: 0.2, time: 0.55, cost: 12 },
  riposte: { time: 1.0, damage: 430, reach: 74 },
  estus: { charges: 3, time: 0.95, heal: 315 },
  /** Блок: сколько урона держит и как дорого это стоит. */
  block: { soak: 0.78 },
  /** Неуязвимость после пропущенного удара — чтобы не умирать с одного касания. */
  mercy: 0.5,
  /** Оглушение после сломанного блока. */
  guardBreak: 1.3,
};

export type PlayerState =
  | 'свободен'
  | 'перекат'
  | 'удар'
  | 'тяжёлый'
  | 'парирует'
  | 'риспост'
  | 'лечится'
  | 'оглушён';

export interface Player {
  at: Vec;
  facing: number;
  hp: number;
  stamina: number;
  estus: number;
  state: PlayerState;
  timer: number;
  /** Неуязвимость: перекат и есть основная защита. */
  iframes: number;
  roll: Vec;
  /** Сколько ударов связки уже сделано и сколько осталось на продолжение. */
  chain: number;
  chainWindow: number;
  blocking: boolean;
  /** Удар этого замаха уже нанесён. */
  hit: boolean;
  /** Пауза перед восстановлением выносливости. */
  rest: number;
}

export interface Input {
  up: boolean;
  down: boolean;
  left: boolean;
  right: boolean;
  roll: boolean;
  light: boolean;
  heavy: boolean;
  block: boolean;
  parry: boolean;
  heal: boolean;
  sprint: boolean;
}

export const noInput: Input = {
  up: false,
  down: false,
  left: false,
  right: false,
  roll: false,
  light: false,
  heavy: false,
  block: false,
  parry: false,
  heal: false,
  sprint: false,
};

export function newPlayer(at: Vec): Player {
  return {
    at: { ...at },
    facing: -Math.PI / 2,
    hp: PLAYER.hp,
    stamina: PLAYER.stamina,
    estus: PLAYER.estus.charges,
    state: 'свободен',
    timer: 0,
    iframes: 0,
    roll: { x: 0, y: 0 },
    chain: 0,
    chainWindow: 0,
    blocking: false,
    hit: false,
    rest: 0,
  };
}

/** Направление, которое человек задал клавишами. Пусто — значит не задал. */
export function heading(input: Input): Vec | null {
  const dir = {
    x: (input.right ? 1 : 0) - (input.left ? 1 : 0),
    y: (input.down ? 1 : 0) - (input.up ? 1 : 0),
  };
  const distance = len(dir);
  if (distance === 0) return null;
  return { x: dir.x / distance, y: dir.y / distance };
}

/** Начать действие, если человек свободен и хватает выносливости. */
export function begin(player: Player, input: Input): void {
  if (player.state !== 'свободен') return;

  if (input.roll && player.stamina >= PLAYER.roll.cost) {
    const dir = heading(input);
    // Без направления катимся назад от врага: так и задумано в первоисточнике.
    player.roll = dir ?? { x: -Math.cos(player.facing), y: -Math.sin(player.facing) };
    player.state = 'перекат';
    player.timer = PLAYER.roll.time;
    player.iframes = PLAYER.roll.iframes;
    spend(player, PLAYER.roll.cost);
    return;
  }
  if (input.parry && player.stamina >= PLAYER.parry.cost) {
    player.state = 'парирует';
    player.timer = PLAYER.parry.time;
    spend(player, PLAYER.parry.cost);
    return;
  }
  if (input.heavy && player.stamina >= PLAYER.heavy.cost) {
    player.state = 'тяжёлый';
    player.timer = PLAYER.heavy.windup + PLAYER.heavy.recover;
    player.hit = false;
    spend(player, PLAYER.heavy.cost);
    return;
  }
  if (input.light && player.stamina >= PLAYER.light.cost) {
    player.state = 'удар';
    player.timer = PLAYER.light.windup + PLAYER.light.recover;
    player.hit = false;
    player.chain = Math.min(3, player.chainWindow > 0 ? player.chain + 1 : 1);
    spend(player, PLAYER.light.cost);
    return;
  }
  if (input.heal && player.estus > 0 && player.hp < PLAYER.hp) {
    player.state = 'лечится';
    player.timer = PLAYER.estus.time;
    player.estus -= 1;
  }
}

export function spend(player: Player, cost: number): void {
  player.stamina = Math.max(0, player.stamina - cost);
  player.rest = PLAYER.regenDelay;
}

/** Движение и часы: всё, что происходит с человеком само по себе. */
export function advance(player: Player, input: Input, dt: number): void {
  player.iframes = Math.max(0, player.iframes - dt);
  player.chainWindow = Math.max(0, player.chainWindow - dt);
  player.rest = Math.max(0, player.rest - dt);
  player.blocking = input.block && player.state === 'свободен' && player.stamina > 0;

  if (player.rest <= 0 && !player.blocking) {
    player.stamina = Math.min(PLAYER.stamina, player.stamina + PLAYER.regen * dt);
  }

  if (player.state === 'перекат') {
    player.at.x += player.roll.x * PLAYER.roll.speed * dt;
    player.at.y += player.roll.y * PLAYER.roll.speed * dt;
    clampToArena(player.at, PLAYER.r);
  } else if (player.state === 'свободен') {
    const dir = heading(input);
    if (dir) {
      // Бег тратит выносливость и не даёт её восстанавливать.
      const running = input.sprint && player.stamina > 0 && !player.blocking;
      const speed = running ? PLAYER.sprint : player.blocking ? PLAYER.speed * 0.6 : PLAYER.speed;
      if (running) spend(player, 12 * dt);
      player.at.x += dir.x * speed * dt;
      player.at.y += dir.y * speed * dt;
      clampToArena(player.at, PLAYER.r);
    }
  }

  if (player.state !== 'свободен') {
    player.timer -= dt;
    if (player.timer <= 0) {
      // После связки даётся окно на продолжение — им и набирают три удара.
      if (player.state === 'удар') player.chainWindow = 0.45;
      player.state = 'свободен';
      player.timer = 0;
    }
  }
}
