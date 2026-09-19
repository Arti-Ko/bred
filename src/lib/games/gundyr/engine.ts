// Бой с Судией Гундиром: мир и один его кадр.
//
// Здесь нет ни одного обращения к холсту и к DOM: на входе нажатые клавиши и
// прошедшее время, на выходе — мир. Поэтому бой можно прогнать без окна и
// убедиться, что он вообще проходим (см. pnpm test:games).

import { approach, BOSS, choose, newBoss, parried, wound, type Boss } from './boss';
import {
  advance,
  begin,
  heading,
  newPlayer,
  PLAYER,
  spend,
  type Input,
  type Player,
} from './player';
import { ARENA, angleTo, insideShape, len, sub, type Shape, type Vec } from './shapes';

export { ARENA } from './shapes';
export { noInput } from './player';
export type { Input, Player } from './player';
export type { Shape, Vec } from './shapes';

export type Phase = 'завеса' | 'бой' | 'превращение' | 'победа' | 'смерть';

/** Что показать поверх пола: замахи, вспышки и подписи. */
export interface Tell {
  shape: Shape;
  progress: number;
  hot: boolean;
  /** Парируемый замах помечен отдельно — по нему и ловят. */
  parryable?: boolean;
}

export interface World {
  player: Player;
  boss: Boss;
  phase: Phase;
  tells: Tell[];
  /** Строка внизу: что он делает прямо сейчас. */
  message: string;
  /** Короткая подсказка о том, что случилось: «парировано», «блок сломан». */
  flash: string;
  flashLeft: number;
  time: number;
  attempt: number;
}

export const maxHp = (): number => PLAYER.hp;
export const maxStamina = (): number => PLAYER.stamina;
export const maxEstus = (): number => PLAYER.estus.charges;
export const bossMaxHp = (): number => BOSS.hp;
export const playerRadius = (): number => PLAYER.r;
export const bossRadius = (): number => BOSS.r;

export function newWorld(attempt = 1): World {
  return {
    player: newPlayer({ x: ARENA.x, y: ARENA.y + 190 }),
    boss: newBoss({ x: ARENA.x, y: ARENA.y - 70 }),
    phase: 'завеса',
    tells: [],
    message: 'за завесой ждёт судия',
    flash: '',
    flashLeft: 0,
    time: 0,
    attempt,
  };
}

function say(world: World, text: string): void {
  world.flash = text;
  world.flashLeft = 1.6;
}

/** Удар игрока: дуга перед собой. Возвращает урон, если попал. */
function strike(world: World, damage: number, poise: number, reach: number, spread: number): void {
  const { player, boss } = world;
  const shape: Shape = {
    kind: 'arc',
    x: player.at.x,
    y: player.at.y,
    r: reach,
    facing: player.facing,
    spread,
  };
  world.tells.push({ shape, progress: 1, hot: true });
  if (!insideShape(boss.at, BOSS.r, shape)) return;

  player.hit = true;
  if (boss.state === 'спит' || boss.state === 'пробуждается') {
    boss.hp -= damage;
    return;
  }
  if (wound(boss, damage, poise)) say(world, 'стойкость сломана');
}

/** Риспост: добивание открытого босса, если стоять вплотную. */
function riposte(world: World): boolean {
  const { player, boss } = world;
  if (boss.state !== 'открыт') return false;
  if (len(sub(boss.at, player.at)) > PLAYER.riposte.reach + BOSS.r) return false;

  const bonus = boss.opening === 'парирование' ? 1 : 0.7;
  boss.hp -= PLAYER.riposte.damage * bonus;
  boss.state = 'отдых';
  boss.timer = 0.6;
  boss.opening = 'нет';
  player.state = 'риспост';
  player.timer = PLAYER.riposte.time;
  player.iframes = PLAYER.riposte.time;
  say(world, 'добивание');
  return true;
}

/** Урон по игроку: сквозь блок, перекат и милость после попадания. */
function hurt(world: World, damage: number, guard: number, grab: boolean): void {
  const player = world.player;
  if (player.iframes > 0) return;

  if (player.blocking && !grab) {
    // Блок съедает урон, но стоит стойкости. Кончилась — блок ломается.
    spend(player, guard);
    player.hp -= damage * (1 - PLAYER.block.soak);
    if (player.stamina <= 0) {
      player.state = 'оглушён';
      player.timer = PLAYER.guardBreak;
      say(world, 'блок сломан');
    }
    player.iframes = 0.2;
    return;
  }

  player.hp -= damage;
  player.iframes = PLAYER.mercy;
  // Из переката, глотка и замаха выбивает: за жадность платят.
  player.state = 'свободен';
  player.timer = 0;
  if (grab) say(world, 'захват');
}

function driveBoss(world: World, input: Input, dt: number): void {
  const { boss, player } = world;
  const haste = boss.phase === 2 ? BOSS.rage : 1;
  const distance = len(sub(player.at, boss.at));

  if (boss.state === 'спит') {
    // Он поднимается, только если подойти или ударить. До тех пор — статуя.
    const touched = distance < BOSS.wakeRange || boss.hp < BOSS.hp;
    if (touched) {
      boss.state = 'пробуждается';
      boss.timer = BOSS.wakeTime;
      world.message = 'судия поднимается';
      say(world, 'меч вынут из груди');
    }
    return;
  }

  if (boss.state === 'пробуждается') {
    boss.timer -= dt;
    if (boss.timer <= 0) {
      boss.state = 'подход';
      boss.cooldown = 0.35;
      world.message = 'бой';
    }
    return;
  }

  if (boss.state === 'открыт') {
    boss.timer -= dt;
    world.message = boss.opening === 'парирование' ? 'открыт — бейте' : 'на колене — бейте';
    // Добить можно, пока он стоит открытым: удар вплотную превращается в риспост.
    if (input.light || input.heavy) riposte(world);
    if (boss.timer <= 0 && boss.state === 'открыт') {
      boss.state = 'отдых';
      boss.timer = 0.4;
      boss.opening = 'нет';
    }
    return;
  }

  if (boss.state === 'подход') {
    boss.cooldown -= dt * haste;
    approach(boss, player.at, dt, 1);
    if (boss.cooldown <= 0) {
      const attack = choose(boss, distance);
      if (attack) {
        boss.attack = attack;
        boss.swing = 0;
        boss.state = 'замах';
        boss.timer = attack.swings[0].windup / haste;
        boss.aim = { ...player.at };
        world.message = attack.name;
      }
    }
    return;
  }

  const attack = boss.attack;
  if (!attack) {
    boss.state = 'подход';
    return;
  }
  const swing = attack.swings[boss.swing];
  boss.timer -= dt;

  if (boss.state === 'замах') {
    const total = swing.windup / haste;
    boss.facing = turnSlightly(boss, player.at, dt);
    if (swing.step) {
      boss.at.x += Math.cos(boss.facing) * (swing.step / total) * dt;
      boss.at.y += Math.sin(boss.facing) * (swing.step / total) * dt;
    }
    world.tells.push({
      shape: swing.shape(boss, boss.aim),
      progress: 1 - Math.max(0, boss.timer) / total,
      hot: false,
      parryable: swing.parryable,
    });

    if (boss.timer <= 0) {
      const shape = swing.shape(boss, boss.aim);
      world.tells.push({ shape, progress: 1, hot: true });

      // Парирование: окно короткое и ловится только на парируемый приём.
      const parrying =
        player.state === 'парирует' && player.timer > PLAYER.parry.time - PLAYER.parry.window;
      if (parrying && swing.parryable && insideShape(player.at, PLAYER.r, shape)) {
        parried(boss);
        say(world, 'парировано');
        player.state = 'свободен';
        player.timer = 0;
        return;
      }
      if (insideShape(player.at, PLAYER.r, shape)) {
        hurt(world, swing.damage, swing.guard, swing.grab === true);
      }
      boss.state = 'удар';
      boss.timer = 0.12;
    }
    return;
  }

  if (boss.state === 'удар' && boss.timer <= 0) {
    boss.swing += 1;
    if (boss.swing < attack.swings.length) {
      boss.state = 'замах';
      boss.timer = attack.swings[boss.swing].windup / haste;
      boss.aim = { ...player.at };
    } else {
      boss.state = 'отдых';
      boss.timer = attack.recover / haste;
      world.message = 'отдыхает — окно';
    }
    return;
  }

  if (boss.state === 'отдых' && boss.timer <= 0) {
    boss.state = 'подход';
    boss.attack = null;
    boss.cooldown = (boss.phase === 2 ? 0.15 : 0.32) + Math.random() * 0.3;
  }
}

/** На замахе он доворачивается, но медленно: уйти ему за спину — рабочая тактика. */
function turnSlightly(boss: Boss, target: Vec, dt: number): number {
  const wanted = angleTo(boss.at, target);
  const delta = ((wanted - boss.facing + Math.PI * 3) % (Math.PI * 2)) - Math.PI;
  const limit = 1.5 * dt;
  return boss.facing + Math.max(-limit, Math.min(limit, delta));
}

/** Один кадр мира. `dt` — секунды, ограничены сверху: вкладка могла спать. */
export function step(world: World, input: Input, dt: number): World {
  if (world.phase === 'победа' || world.phase === 'смерть') return world;
  dt = Math.min(dt, 0.05);
  world.time += dt;
  world.tells = [];
  world.flashLeft = Math.max(0, world.flashLeft - dt);

  const { player, boss } = world;

  if (world.phase === 'завеса') {
    // Завеса: пока не шагнули вперёд, ничего не происходит.
    if (input.up || input.light || input.roll) {
      world.phase = 'бой';
      world.message = 'судия спит — подойдите';
    }
    return world;
  }

  // С захватом цели взгляд приклеен к боссу, без него — к движению: ровно так
  // это работает в соулсах, и от этого зависит, куда полетит удар и перекат.
  if (input.lockOn) {
    player.facing = angleTo(player.at, boss.at);
  } else {
    const dir = heading(input);
    if (dir) player.facing = Math.atan2(dir.y, dir.x);
  }

  advance(player, input, dt);

  // Удар игрока: активная фаза наступает после замаха.
  if (player.state === 'удар' || player.state === 'тяжёлый') {
    const spec =
      player.state === 'тяжёлый'
        ? PLAYER.heavy
        : player.running
          ? PLAYER.runAttack
          : PLAYER.light;
    const elapsed = spec.windup + spec.recover - player.timer;
    if (!player.hit && elapsed >= spec.windup) {
      if (!riposte(world)) {
        const growth = player.state === 'удар' ? 1 + (player.chain - 1) * 0.12 : 1;
        strike(world, spec.damage * growth, spec.poise, spec.reach, spec.spread);
      }
      player.hit = true;
    }
  }
  if (player.state === 'лечится' && player.timer <= dt) {
    player.hp = Math.min(PLAYER.hp, player.hp + PLAYER.estus.heal);
  }

  begin(player, input);

  // Превращение: ровно на половине здоровья и ровно один раз.
  if (world.phase === 'бой' && boss.phase === 1 && boss.hp <= BOSS.hp * BOSS.phaseAt) {
    world.phase = 'превращение';
    boss.state = 'отдых';
    boss.attack = null;
    boss.timer = 2.1;
    world.message = 'из него лезет бездна';
    say(world, 'вторая фаза');
  } else if (world.phase === 'превращение') {
    boss.timer -= dt;
    const shape: Shape = { kind: 'circle', x: boss.at.x, y: boss.at.y, r: 165 };
    if (boss.timer <= 0) {
      boss.phase = 2;
      world.phase = 'бой';
      boss.state = 'подход';
      boss.cooldown = 0.2;
      world.tells.push({ shape, progress: 1, hot: true });
      if (insideShape(player.at, PLAYER.r, shape)) hurt(world, 120, 70, false);
    } else {
      world.tells.push({ shape, progress: 1 - boss.timer / 2.1, hot: false });
    }
  } else {
    driveBoss(world, input, dt);
  }

  boss.poise = Math.max(0, boss.poise - BOSS.poiseDecay * dt);

  if (boss.hp <= 0) {
    world.phase = 'победа';
    world.message = 'ВРАГ ПОВЕРЖЕН';
  } else if (player.hp <= 0) {
    world.phase = 'смерть';
    world.message = 'ВЫ ПОГИБЛИ';
  }
  return world;
}
