// Самопроверка игровых движков.
//
// Отдельным прогоном, а не тестовым бегунком: ради трёх игр тащить в проект
// целый фреймворк незачем, а проверять их надо — особенно шашки, где правила
// сложнее самой игры, и бой, который обязан быть проходимым.
//
// Запуск: pnpm test:games

import * as t from '../src/lib/games/tictactoe/engine';
import * as c from '../src/lib/games/checkers/engine';
import * as g from '../src/lib/games/gundyr/engine';
import { insideShape } from '../src/lib/games/gundyr/shapes';

let failed = 0;
function ok(name: string, condition: boolean, extra = ''): void {
  if (!condition) failed++;
  console.log(`${condition ? '  ok  ' : 'ПРОВАЛ '} ${name}${extra ? ` — ${extra}` : ''}`);
}

console.log('крестики-нолики');
{
  const board = t.emptyBoard();
  [0, 1, 2, 3].forEach((i) => (board[i] = 'x'));
  ok('линия из четырёх опознаётся', t.winner(board)?.mark === 'x');

  const block = t.emptyBoard();
  [5, 6, 7].forEach((i) => (block[i] = 'x'));
  block[12] = 'o';
  const defence = t.bestMove(block, 'o', 150);
  ok('бот закрывает чужую тройку', defence === 4 || defence === 8, `сходил ${defence}`);

  const kill = t.emptyBoard();
  [10, 11, 12].forEach((i) => (kill[i] = 'o'));
  [0, 1, 2].forEach((i) => (kill[i] = 'x'));
  ok('свою тройку бот дожимает', t.bestMove(kill, 'o', 150) === 13);

  // Поиск обязан оставлять доску такой, какой взял: время выходит броском,
  // и раскрутка стека однажды уже оставляла на поле все пробные ходы.
  const dirty = t.emptyBoard();
  [6, 12, 18, 8].forEach((i) => (dirty[i] = 'x'));
  [7, 11, 13].forEach((i) => (dirty[i] = 'o'));
  const snapshot = [...dirty];
  t.bestMove(dirty, 'o', 60);
  ok('перебор не пачкает доску', dirty.every((cell, i) => cell === snapshot[i]));

  const midgame = t.emptyBoard();
  [6, 12, 18].forEach((i) => (midgame[i] = 'x'));
  [7, 11].forEach((i) => (midgame[i] = 'o'));
  const started = performance.now();
  t.bestMove(midgame, 'o', 220);
  ok(
    'укладывается в отпущенное время',
    performance.now() - started < 900,
    `${Math.round(performance.now() - started)} мс`,
  );
}

console.log('шашки');
{
  const start = c.initialBoard();
  ok('на доске по двенадцать', start.filter((p) => p === 'w').length === 12 && start.filter((p) => p === 'b').length === 12);
  ok('в начале у белых семь ходов', c.legalMoves(start, 'w').length === 7);

  const chain: c.Board = Array(64).fill(null);
  chain[c.at(5, 2)] = 'w';
  chain[c.at(4, 3)] = 'b';
  chain[c.at(2, 3)] = 'b';
  const forced = c.legalMoves(chain, 'w');
  ok('бой обязателен', forced.length > 0 && forced.every((m) => m.captured.length > 0));
  ok('цепочка боёв доигрывается до конца', forced.some((m) => m.captured.length === 2));

  const flying: c.Board = Array(64).fill(null);
  flying[c.at(7, 0)] = 'W';
  flying[c.at(3, 4)] = 'b';
  const king = c.legalMoves(flying, 'w');
  ok('дамка бьёт с расстояния и садится куда хочет', king.length === 3 && king.every((m) => m.captured.length === 1));

  const promo: c.Board = Array(64).fill(null);
  promo[c.at(1, 2)] = 'w';
  ok('дошедшая до края становится дамкой', c.applyMove(promo, c.legalMoves(promo, 'w')[0]).includes('W'));

  const stuck: c.Board = Array(64).fill(null);
  stuck[c.at(0, 1)] = 'w';
  stuck[c.at(1, 0)] = 'b';
  stuck[c.at(1, 2)] = 'b';
  stuck[c.at(2, 3)] = 'b';
  ok('без ходов — поражение', c.loser(stuck, 'w') === 'w');
  const free = [...stuck];
  free[c.at(2, 3)] = null;
  ok('пока есть куда бить — не поражение', c.loser(free, 'w') === null);

  const began = performance.now();
  ok('бот ходит', c.bestMove(c.initialBoard(), 'b', 300) !== null);
  ok('и не думает вечно', performance.now() - began < 1500, `${Math.round(performance.now() - began)} мс`);
}

console.log('судия гундир');
{
  // Столб посреди арены обязан умереть: босс бьёт и попадает.
  const idle = g.newWorld();
  for (let i = 0; i < 60 * 90 && idle.phase === 'бой'; i++) g.step(idle, g.noInput, 1 / 60);
  ok('простоявший столбом погибает', idle.phase === 'смерть');

  // Перекат неуязвим: в нём же весь смысл.
  const rolling = g.newWorld();
  g.step(rolling, { ...g.noInput, roll: true }, 1 / 60);
  ok('в перекате есть неуязвимость', rolling.player.iframes > 0);

  // И главное: бой проходим. Играет простейший ученик — держится вплотную,
  // уходит перекатом с подсвеченного места и бьёт, когда босс отдыхает.
  // Заходов до двадцати, но выходим на первой же победе: проверяется
  // проходимость, а не счёт. Требовать от ученика стабильного результата
  // нельзя — набор приёмов у босса случайный, и разброс честный.
  const runs = 20;
  let wins = 0;
  let best = 0;
  let played = 0;
  for (let run = 0; run < runs && wins === 0; run++) {
    played++;
    const world = g.newWorld();
    for (let i = 0; i < 60 * 240 && world.phase !== 'победа' && world.phase !== 'смерть'; i++) {
      g.step(world, student(world), 1 / 60);
    }
    if (world.phase === 'победа') wins++;
    best = Math.max(best, 1 - Math.max(0, world.boss.hp) / g.bossMaxHp());
  }
  // Проверяем именно проходимость: ученик играет без фантазии и половину
  // заходов проигрывает — это и есть нужная планка. Требовать от него
  // стабильного счёта нельзя, у босса случайный набор приёмов.
  ok(
    'бой проходим',
    wins > 0,
    `выиграл с ${played}-го захода из ${runs}, лучший заход — ${Math.round(best * 100)}% снятого здоровья`,
  );
}

/** Тот, кто уже понял правила, но играет без фантазии. */
function student(world: g.World): g.Input {
  const p = world.player;
  const boss = world.boss;
  const away = Math.atan2(p.at.y - boss.at.y, p.at.x - boss.at.x);
  const danger = world.tells.some(
    (tell) => !tell.hot && tell.progress > 0.48 && insideShape(p.at, 18, tell.shape),
  );
  const direction = {
    up: Math.sin(away) < -0.3,
    down: Math.sin(away) > 0.3,
    left: Math.cos(away) < -0.3,
    right: Math.cos(away) > 0.3,
  };
  if (danger && p.stamina > 26) return { ...g.noInput, ...direction, roll: true };
  if (p.hp < 55 && p.estus > 0 && boss.stage === 'подход' && boss.cooldown > 0.5) {
    return { ...g.noInput, heal: true };
  }
  const distance = Math.hypot(p.at.x - boss.at.x, p.at.y - boss.at.y);
  if (distance > 60) {
    return {
      ...g.noInput,
      up: Math.sin(away) > 0.3,
      down: Math.sin(away) < -0.3,
      left: Math.cos(away) > 0.3,
      right: Math.cos(away) < -0.3,
    };
  }
  // Бьют в отдых после связки — это и есть окно, которое босс отдаёт сам.
  const window = boss.stage === 'отдых' || (boss.stage === 'подход' && boss.cooldown > 0.35);
  if (window && p.stamina > 24) return { ...g.noInput, attack: true };
  return g.noInput;
}

if (failed > 0) throw new Error(`провалов: ${failed}`);
console.log('\nвсё сошлось');
