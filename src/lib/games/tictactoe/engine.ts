// Крестики-нолики, в которые есть смысл играть.
//
// Поле 3×3 не годится: там всё разобрано до последнего хода, и безошибочный
// противник не проигрывает никогда — а выиграть тут надо по условию. Поэтому
// поле 5×5 и линия из четырёх: перевес у того, кто ходит первым, но только если
// он умеет ставить двойную угрозу. Это и есть «очень сложные».

export const SIZE = 5;
/** Сколько подряд нужно собрать. */
export const LINE = 4;
export const CENTER = Math.floor((SIZE * SIZE) / 2);

export type Mark = 'x' | 'o';
export type Cell = Mark | null;
export type Board = Cell[];

export interface Win {
  mark: Mark;
  cells: number[];
}

export function emptyBoard(): Board {
  return Array<Cell>(SIZE * SIZE).fill(null);
}

export function other(mark: Mark): Mark {
  return mark === 'x' ? 'o' : 'x';
}

/** Все возможные линии из `LINE` клеток: по ним и считается всё остальное. */
function buildWindows(): number[][] {
  const windows: number[][] = [];
  const at = (r: number, c: number) => r * SIZE + c;
  const steps = [
    [0, 1],
    [1, 0],
    [1, 1],
    [1, -1],
  ];
  for (let r = 0; r < SIZE; r++) {
    for (let c = 0; c < SIZE; c++) {
      for (const [dr, dc] of steps) {
        const last = [r + dr * (LINE - 1), c + dc * (LINE - 1)];
        if (last[0] < 0 || last[1] < 0 || last[0] >= SIZE || last[1] >= SIZE) continue;
        windows.push(
          Array.from({ length: LINE }, (_, k) => at(r + dr * k, c + dc * k)),
        );
      }
    }
  }
  return windows;
}

const WINDOWS = buildWindows();

/** Линии, проходящие через клетку. Нужны, чтобы не пересчитывать поле целиком. */
const THROUGH: number[][][] = Array.from({ length: SIZE * SIZE }, (_, cell) =>
  WINDOWS.filter((w) => w.includes(cell)),
);

export function winner(board: Board): Win | null {
  for (const w of WINDOWS) {
    const mark = board[w[0]];
    if (mark && w.every((i) => board[i] === mark)) return { mark, cells: w };
  }
  return null;
}

export function full(board: Board): boolean {
  return board.every((cell) => cell !== null);
}

/** Есть ли у клетки занятый сосед: играть в пустом углу поля бессмысленно. */
function nearOccupied(board: Board, cell: number): boolean {
  const r = Math.floor(cell / SIZE);
  const c = cell % SIZE;
  for (let dr = -2; dr <= 2; dr++) {
    for (let dc = -2; dc <= 2; dc++) {
      const rr = r + dr;
      const cc = c + dc;
      if (rr < 0 || cc < 0 || rr >= SIZE || cc >= SIZE) continue;
      if (board[rr * SIZE + cc]) return true;
    }
  }
  return false;
}

/** Во сколько оценивается `n` своих клеток в одной линии. */
const WEIGHT = [0, 1, 14, 180, 1_000_000];
/** Чужая угроза дороже своей: пропущенная тройка стоит партии. */
const DEFENCE = 1.3;
const WIN_SCORE = 1_000_000;

export function evaluate(board: Board, me: Mark): number {
  const you = other(me);
  let score = 0;
  for (const w of WINDOWS) {
    let mine = 0;
    let theirs = 0;
    for (const i of w) {
      const cell = board[i];
      if (cell === me) mine++;
      else if (cell === you) theirs++;
    }
    if (mine && theirs) continue; // линия мёртвая для обоих
    if (mine) score += WEIGHT[mine];
    else if (theirs) score -= WEIGHT[theirs] * DEFENCE;
  }
  return score;
}

/** Быстрая оценка одного хода — только по линиям, которые он задевает. */
function moveScore(board: Board, cell: number, mark: Mark): number {
  const you = other(mark);
  let score = 0;
  for (const w of THROUGH[cell]) {
    let mine = 0;
    let theirs = 0;
    for (const i of w) {
      const c = board[i];
      if (c === mark) mine++;
      else if (c === you) theirs++;
    }
    if (mine && theirs) continue;
    score += mine ? WEIGHT[mine + 1] : WEIGHT[theirs + 1] * DEFENCE;
  }
  return score;
}

function candidates(board: Board, mark: Mark): number[] {
  const cells: number[] = [];
  for (let i = 0; i < board.length; i++) {
    if (!board[i] && nearOccupied(board, i)) cells.push(i);
  }
  if (cells.length === 0) return board[CENTER] ? [] : [CENTER];
  // Порядок решает всё: хорошее отсечение начинается с хорошего первого хода.
  return cells.sort((a, b) => moveScore(board, b, mark) - moveScore(board, a, mark));
}

/** Сигнал «время вышло»: недосчитанную глубину целиком выбрасываем. */
const TIMEOUT = Symbol('время вышло');

function negamax(
  board: Board,
  turn: Mark,
  depth: number,
  ply: number,
  alpha: number,
  beta: number,
  deadline: number,
): number {
  if (performance.now() > deadline) throw TIMEOUT;

  const win = winner(board);
  // Выиграл тот, кто ходил до нас, — значит для нас это проигрыш. Чем позже
  // он случится, тем он «дешевле»: так бот тянет время в проигранной позиции
  // и не медлит в выигранной.
  if (win) return -(WIN_SCORE - ply);
  if (full(board)) return 0;
  if (depth === 0) return evaluate(board, turn);

  let best = -Infinity;
  for (const cell of candidates(board, turn)) {
    board[cell] = turn;
    let score: number;
    // Возврат клетки — обязательно через finally. Время выходит броском, и
    // раскрутка стека иначе оставляла на доске все пробные ходы разом: человек
    // кликал по одной клетке, а на поле появлялось шесть значков.
    try {
      score = -negamax(board, other(turn), depth - 1, ply + 1, -beta, -alpha, deadline);
    } finally {
      board[cell] = null;
    }

    if (score > best) best = score;
    if (best > alpha) alpha = best;
    if (alpha >= beta) break;
  }
  return best;
}

/**
 * Ход бота. Считает, сколько успеет за отпущенное время: сперва мелкую глубину,
 * потом всё глубже, и берёт лучший полностью досчитанный вариант.
 */
export function bestMove(board: Board, mark: Mark, budgetMs = 220): number | null {
  const options = candidates(board, mark);
  if (options.length === 0) return null;
  if (options.length === 1) return options[0];

  const deadline = performance.now() + budgetMs;
  let chosen = options[0];

  for (let depth = 2; depth <= board.length; depth += 1) {
    let best = -Infinity;
    let bestCell = chosen;
    try {
      for (const cell of options) {
        board[cell] = mark;
        let score: number;
        try {
          score = -negamax(board, other(mark), depth - 1, 1, -Infinity, Infinity, deadline);
        } finally {
          board[cell] = null;
        }
        if (score > best) {
          best = score;
          bestCell = cell;
        }
      }
    } catch (error) {
      if (error !== TIMEOUT) throw error;
      break; // недосчитанная глубина ничего не говорит — берём прошлую
    }
    chosen = bestCell;
    // Выигрыш найден и доказан — глубже искать нечего.
    if (best >= WIN_SCORE - board.length) break;
    if (performance.now() > deadline) break;
  }
  return chosen;
}
