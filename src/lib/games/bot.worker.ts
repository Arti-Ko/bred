// Бот думает в стороне от главного потока.
//
// Причина ровно та же, по которой звук в звонке считается в AudioWorklet, а не
// в главном потоке: этот поток принимает кадры собеседника и вызывает декодеры.
// Перебор на четыреста миллисекунд — это четыреста миллисекунд, в которые
// входящие кадры не разбираются, а потом выпускаются рывком. Игру открывают и
// посреди звонка, и щелчок в чужих наушниках на каждый ход шашками — цена,
// которую платить незачем.

import { bestMove as checkersMove, type Board as CheckersBoard, type Move, type Side } from './checkers/engine';
import { bestMove as tictactoeMove, type Board as MarksBoard, type Mark } from './tictactoe/engine';

/** Сам вопрос, без номера: номером его снабжает отправляющая сторона. */
export type Question =
  | { game: 'крестики'; board: MarksBoard; mark: Mark; budgetMs: number }
  | { game: 'шашки'; board: CheckersBoard; side: Side; budgetMs: number };

export type Ask = Question & { id: number };

export type Answer =
  | { id: number; game: 'крестики'; move: number | null }
  | { id: number; game: 'шашки'; move: Move | null };

/** Минимальное описание области воркера: без него пришлось бы тянуть lib webworker. */
interface Scope {
  onmessage: ((event: MessageEvent<Ask>) => void) | null;
  postMessage: (answer: Answer) => void;
}

const scope = self as unknown as Scope;

scope.onmessage = (event) => {
  const ask = event.data;
  scope.postMessage(
    ask.game === 'крестики'
      ? { id: ask.id, game: 'крестики', move: tictactoeMove(ask.board, ask.mark, ask.budgetMs) }
      : { id: ask.id, game: 'шашки', move: checkersMove(ask.board, ask.side, ask.budgetMs) },
  );
};
