// Спросить у бота ход, не занимая главный поток.
//
// Если отдельный поток завести не вышло (или он умер), считаем на месте: игра
// важнее чистоты приёма — но по умолчанию она уступает дорогу звонку.

import type { Answer, Ask, Question } from './bot.worker';
import { bestMove as checkersMove, type Board as CheckersBoard, type Move, type Side } from './checkers/engine';
import { bestMove as tictactoeMove, type Board as MarksBoard, type Mark } from './tictactoe/engine';

/** `undefined` — ещё не пробовали, `null` — не получилось, считаем сами. */
let worker: Worker | null | undefined;
let counter = 0;
const waiting = new Map<number, (answer: Answer) => void>();

function pool(): Worker | null {
  if (worker !== undefined) return worker;
  try {
    worker = new Worker(new URL('./bot.worker.ts', import.meta.url), { type: 'module' });
    worker.onmessage = (event: MessageEvent<Answer>) => {
      waiting.get(event.data.id)?.(event.data);
      waiting.delete(event.data.id);
    };
    worker.onerror = () => {
      // Поток умер — дальше считаем на месте, а ждущих отпускаем без ответа.
      worker = null;
      for (const resolve of waiting.values()) resolve({ id: -1, game: 'крестики', move: null });
      waiting.clear();
    };
  } catch {
    worker = null;
  }
  return worker;
}

function ask(request: Question): Promise<Answer | null> {
  const thread = pool();
  if (!thread) return Promise.resolve(null);
  const id = ++counter;
  return new Promise((resolve) => {
    waiting.set(id, resolve);
    thread.postMessage({ ...request, id } as Ask);
  });
}

export async function thinkMarks(
  board: MarksBoard,
  mark: Mark,
  budgetMs = 220,
): Promise<number | null> {
  const answer = await ask({ game: 'крестики', board, mark, budgetMs });
  if (answer?.game === 'крестики' && answer.id > 0) return answer.move;
  return tictactoeMove(board, mark, budgetMs);
}

export async function thinkCheckers(
  board: CheckersBoard,
  side: Side,
  budgetMs = 400,
): Promise<Move | null> {
  const answer = await ask({ game: 'шашки', board, side, budgetMs });
  if (answer?.game === 'шашки' && answer.id > 0) return answer.move;
  return checkersMove(board, side, budgetMs);
}
