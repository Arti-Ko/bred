// Русские шашки: правила целиком, без поблажек.
//
// Бить обязательно, бой идёт до конца цепочки, простая бьёт и назад, дамка
// ходит и бьёт на любое расстояние, побитые снимаются только в конце хода и до
// тех пор мешают прыгать через себя. Из этих мелочей и состоит разница между
// «шашками» и «шашками против бота, которого не обыграть».

export type Side = 'w' | 'b';
export type Piece = 'w' | 'W' | 'b' | 'B';
export type Cell = Piece | null;
export type Board = Cell[];

export const SIZE = 8;
/** Побитая, но ещё не снятая с доски: прыгать через неё второй раз нельзя. */
const DOOMED = 'x';
type Square = Cell | typeof DOOMED;

export interface Move {
  from: number;
  /** Клетки приземления по порядку. Для тихого хода — одна. */
  path: number[];
  /** Что снимается с доски по завершении. */
  captured: number[];
}

export const at = (row: number, col: number): number => row * SIZE + col;
export const rowOf = (cell: number): number => Math.floor(cell / SIZE);
export const colOf = (cell: number): number => cell % SIZE;
export const dark = (cell: number): boolean => (rowOf(cell) + colOf(cell)) % 2 === 1;
export const sideOf = (piece: Piece): Side => (piece === 'w' || piece === 'W' ? 'w' : 'b');
export const isKing = (piece: Piece): boolean => piece === 'W' || piece === 'B';
export const other = (side: Side): Side => (side === 'w' ? 'b' : 'w');

const DIRS: [number, number][] = [
  [-1, -1],
  [-1, 1],
  [1, -1],
  [1, 1],
];

export function initialBoard(): Board {
  const board: Board = Array<Cell>(SIZE * SIZE).fill(null);
  for (let cell = 0; cell < board.length; cell++) {
    if (!dark(cell)) continue;
    const row = rowOf(cell);
    if (row < 3) board[cell] = 'b';
    if (row > 4) board[cell] = 'w';
  }
  return board;
}

/** Последняя горизонталь для стороны: дошёл — стал дамкой. */
const crown = (side: Side): number => (side === 'w' ? 0 : SIZE - 1);

function inside(row: number, col: number): boolean {
  return row >= 0 && col >= 0 && row < SIZE && col < SIZE;
}

/** Все ходы боем для одной шашки. Рекурсия идёт по цепочке. */
function captures(
  board: Square[],
  from: number,
  cell: number,
  side: Side,
  king: boolean,
  taken: number[],
  path: number[],
  out: Move[],
): void {
  const row = rowOf(cell);
  const col = colOf(cell);
  let found = false;

  for (const [dr, dc] of DIRS) {
    // Дамка ищет жертву на любом расстоянии, простая — только вплотную.
    let r = row + dr;
    let c = col + dc;
    if (!king) {
      if (!inside(r, c)) continue;
    } else {
      while (inside(r, c) && board[at(r, c)] === null) {
        r += dr;
        c += dc;
      }
      if (!inside(r, c)) continue;
    }

    const victim = at(r, c);
    const piece = board[victim];
    if (piece === null || piece === DOOMED) continue;
    if (sideOf(piece as Piece) === side) continue;

    // Куда приземляемся: простая — сразу за жертвой, дамка — на любое пустое
    // поле за ней, пока не упрётся.
    let lr = r + dr;
    let lc = c + dc;
    while (inside(lr, lc) && board[at(lr, lc)] === null) {
      const landing = at(lr, lc);

      board[cell] = null;
      board[victim] = DOOMED;
      // Дошёл до последней горизонтали в бою — дальше бьёт уже дамкой.
      const crowned = king || rowOf(landing) === crown(side);
      board[landing] = crowned ? (side === 'w' ? 'W' : 'B') : (side as Piece);

      found = true;
      captures(
        board,
        from,
        landing,
        side,
        crowned,
        [...taken, victim],
        [...path, landing],
        out,
      );

      board[landing] = null;
      board[victim] = piece;
      board[cell] = king ? (side === 'w' ? 'W' : 'B') : (side as Piece);

      if (!king) break; // простая приземляется ровно за жертвой
      lr += dr;
      lc += dc;
    }
  }

  // Дальше бить нечем — цепочка закончилась, ход целиком готов.
  if (!found && taken.length > 0) out.push({ from, path, captured: taken });
}

function quietMoves(board: Board, cell: number, piece: Piece, out: Move[]): void {
  const side = sideOf(piece);
  const row = rowOf(cell);
  const col = colOf(cell);
  const forward = side === 'w' ? -1 : 1;

  for (const [dr, dc] of DIRS) {
    if (!isKing(piece) && dr !== forward) continue; // простая ходит только вперёд
    let r = row + dr;
    let c = col + dc;
    while (inside(r, c) && board[at(r, c)] === null) {
      out.push({ from: cell, path: [at(r, c)], captured: [] });
      if (!isKing(piece)) break;
      r += dr;
      c += dc;
    }
  }
}

/** Всё, что стороне разрешено. Есть бой — только бой: правило обязательное. */
export function legalMoves(board: Board, side: Side): Move[] {
  const taking: Move[] = [];
  const quiet: Move[] = [];
  const working: Square[] = [...board];

  for (let cell = 0; cell < board.length; cell++) {
    const piece = board[cell];
    if (!piece || sideOf(piece) !== side) continue;
    captures(working, cell, cell, side, isKing(piece), [], [], taking);
  }
  if (taking.length > 0) return taking;

  for (let cell = 0; cell < board.length; cell++) {
    const piece = board[cell];
    if (!piece || sideOf(piece) !== side) continue;
    quietMoves(board, cell, piece, quiet);
  }
  return quiet;
}

export function applyMove(board: Board, move: Move): Board {
  const next = [...board];
  const piece = next[move.from];
  if (!piece) return next;
  const side = sideOf(piece);
  const landing = move.path[move.path.length - 1];

  next[move.from] = null;
  for (const victim of move.captured) next[victim] = null;

  const crowned = isKing(piece) || rowOf(landing) === crown(side);
  next[landing] = crowned ? (side === 'w' ? 'W' : 'B') : (side as Piece);
  return next;
}

/** Кто проиграл: у кого не осталось ни шашек, ни ходов. */
export function loser(board: Board, turn: Side): Side | null {
  if (legalMoves(board, turn).length === 0) return turn;
  return null;
}

// ── бот ──────────────────────────────────────────────────────────────────────

const MAN = 100;
const KING = 320;
/** Продвижение к дамкам: шашка на седьмой горизонтали стоит дороже своей же на второй. */
const ADVANCE = 9;
/** Задняя линия держит оборону: её разбор — частая причина внезапного проигрыша. */
const HOME = 12;

export function evaluate(board: Board, me: Side): number {
  let score = 0;
  for (let cell = 0; cell < board.length; cell++) {
    const piece = board[cell];
    if (!piece) continue;
    const side = sideOf(piece);
    const sign = side === me ? 1 : -1;
    if (isKing(piece)) {
      score += sign * KING;
      continue;
    }
    const advance = side === 'w' ? SIZE - 1 - rowOf(cell) : rowOf(cell);
    let value = MAN + advance * ADVANCE;
    if (rowOf(cell) === (side === 'w' ? SIZE - 1 : 0)) value += HOME;
    score += sign * value;
  }
  return score;
}

const TIMEOUT = Symbol('время вышло');
const WIN = 100_000;

function negamax(
  board: Board,
  turn: Side,
  depth: number,
  ply: number,
  alpha: number,
  beta: number,
  deadline: number,
): number {
  if (performance.now() > deadline) throw TIMEOUT;

  const moves = legalMoves(board, turn);
  if (moves.length === 0) return -(WIN - ply); // ходить нечем — проиграл

  // Бой продолжаем считать и за горизонтом: остановиться посреди размена —
  // значит поверить, что отданная шашка просто исчезла.
  const forced = moves.length === 1 || moves[0].captured.length > 0;
  if (depth <= 0 && !forced) return evaluate(board, turn);
  if (depth <= -6) return evaluate(board, turn);

  let best = -Infinity;
  for (const move of moves) {
    const next = applyMove(board, move);
    const score = -negamax(next, other(turn), depth - 1, ply + 1, -beta, -alpha, deadline);
    if (score > best) best = score;
    if (best > alpha) alpha = best;
    if (alpha >= beta) break;
  }
  return best;
}

export function bestMove(board: Board, side: Side, budgetMs = 400): Move | null {
  const moves = legalMoves(board, side);
  if (moves.length === 0) return null;
  if (moves.length === 1) return moves[0];

  const deadline = performance.now() + budgetMs;
  // Сперва самые «злые»: с ними отсечение работает лучше всего.
  const ordered = [...moves].sort((a, b) => b.captured.length - a.captured.length);
  let chosen = ordered[0];

  for (let depth = 2; depth <= 12; depth++) {
    let best = -Infinity;
    let bestMove = chosen;
    try {
      for (const move of ordered) {
        const next = applyMove(board, move);
        const score = -negamax(next, other(side), depth - 1, 1, -Infinity, Infinity, deadline);
        if (score > best) {
          best = score;
          bestMove = move;
        }
      }
    } catch (error) {
      if (error !== TIMEOUT) throw error;
      break;
    }
    chosen = bestMove;
    if (best >= WIN - 50) break;
    if (performance.now() > deadline) break;
  }
  return chosen;
}
