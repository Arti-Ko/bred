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
  /** Шаг назад: короче переката и почти без неуязвимости — как и положено. */
  backstep: { time: 0.34, iframes: 0.1, speed: 300, cost: 11 },
  /** Удар с бега: дальше и больнее обычного, но с длинным замахом. */
  runAttack: { windup: 0.28, recover: 0.42, cost: 24, damage: 134, poise: 60, reach: 76, spread: 1.2 },
  /** Сколько живёт нажатие, сделанное «в отказ»: в соулсах ввод буферизуется. */
  buffer: 0.4,
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
  /** Нажатие, сделанное во время другого действия: сработает, как освободимся. */
  buffered: 'перекат' | 'удар' | 'тяжёлый' | 'парирует' | 'лечится' | null;
  bufferLeft: number;
  /** Бежал ли в момент удара — от этого зависит, какой удар получится. */
  running: boolean;
}

export interface Input {
  /**
   * Куда смотрит камера. Движение в соулсах отсчитывается от неё, а не от мира:
   * «вперёд» — это вперёд по экрану, и при обходе босса по кругу рука не
   * переучивается.
   */
  yaw: number;
  /** Захват цели: с ним взгляд приклеен к боссу, без него — к движению. */
  lockOn: boolean;
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
  yaw: 0,
  lockOn: true,
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
    buffered: null,
    bufferLeft: 0,
    running: false,
  };
}

/**
 * Направление, которое человек задал клавишами, — повёрнутое под камеру.
 * Пусто — значит не задал.
 */
export function heading(input: Input): Vec | null {
  const raw = {
    x: (input.right ? 1 : 0) - (input.left ? 1 : 0),
    y: (input.down ? 1 : 0) - (input.up ? 1 : 0),
  };
  const distance = len(raw);
  if (distance === 0) return null;

  const cos = Math.cos(input.yaw);
  const sin = Math.sin(input.yaw);
  const dir = {
    x: (raw.x * cos - raw.y * sin) / distance,
    y: (raw.x * sin + raw.y * cos) / distance,
  };
  return dir;
}

/** Что человек нажал прямо сейчас, в порядке старшинства. */
function pressed(input: Input): Player['buffered'] {
  if (input.roll) return 'перекат';
  if (input.parry) return 'парирует';
  if (input.heavy) return 'тяжёлый';
  if (input.light) return 'удар';
  if (input.heal) return 'лечится';
  return null;
}

/**
 * Начать действие. Если человек занят — нажатие не теряется, а ждёт своей
 * очереди: в соулсах ввод буферизуется, и без этого связка рассыпается.
 */
export function begin(player: Player, input: Input): void {
  const now = pressed(input);
  if (player.state !== 'свободен') {
    if (now) {
      player.buffered = now;
      player.bufferLeft = PLAYER.buffer;
    }
    return;
  }

  const wanted = now ?? (player.bufferLeft > 0 ? player.buffered : null);
  player.buffered = null;
  player.bufferLeft = 0;
  if (!wanted) return;

  if (wanted === 'перекат' && player.stamina >= PLAYER.backstep.cost) {
    const dir = heading(input);
    if (!dir) {
      // Без направления — шаг назад, а не перекат. Разница в неуязвимости.
      player.roll = { x: -Math.cos(player.facing), y: -Math.sin(player.facing) };
      player.state = 'перекат';
      player.timer = PLAYER.backstep.time;
      player.iframes = PLAYER.backstep.iframes;
      spend(player, PLAYER.backstep.cost);
      return;
    }
    if (player.stamina < PLAYER.roll.cost) return;
    player.roll = dir;
    player.state = 'перекат';
    player.timer = PLAYER.roll.time;
    player.iframes = PLAYER.roll.iframes;
    spend(player, PLAYER.roll.cost);
    return;
  }
  if (wanted === 'парирует' && player.stamina >= PLAYER.parry.cost) {
    player.state = 'парирует';
    player.timer = PLAYER.parry.time;
    spend(player, PLAYER.parry.cost);
    return;
  }
  if (wanted === 'тяжёлый' && player.stamina >= PLAYER.heavy.cost) {
    player.state = 'тяжёлый';
    player.timer = PLAYER.heavy.windup + PLAYER.heavy.recover;
    player.hit = false;
    spend(player, PLAYER.heavy.cost);
    return;
  }
  if (wanted === 'удар' && player.stamina >= PLAYER.light.cost) {
    // Удар с бега — отдельный приём: дальше, больнее и с длинным замахом.
    const dashing = player.running && input.sprint && player.stamina >= PLAYER.runAttack.cost;
    const spec = dashing ? PLAYER.runAttack : PLAYER.light;
    player.state = 'удар';
    player.timer = spec.windup + spec.recover;
    player.hit = false;
    player.running = dashing;
    player.chain = dashing ? 1 : Math.min(3, player.chainWindow > 0 ? player.chain + 1 : 1);
    spend(player, spec.cost);
    return;
  }
  if (wanted === 'лечится' && player.estus > 0 && player.hp < PLAYER.hp) {
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
  player.bufferLeft = Math.max(0, player.bufferLeft - dt);
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
      player.running = running;
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
