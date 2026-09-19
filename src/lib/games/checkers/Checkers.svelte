<script lang="ts">
  import Screen from '../Screen.svelte';
  import { thinkCheckers } from '../think';
  import {
    applyMove,
    colOf,
    dark,
    initialBoard,
    isKing,
    legalMoves,
    loser,
    rowOf,
    sideOf,
    type Board,
    type Cell,
  } from './engine';

  interface Props {
    onwin: () => void;
    onback: () => void;
  }
  const { onwin, onback }: Props = $props();

  let board = $state<Board>(initialBoard());
  let selected = $state<number | null>(null);
  /** Уже выбранные точки боя: цепочку человек проходит по шагам, как рукой. */
  let chain = $state<number[]>([]);
  let thinking = $state(false);
  let result = $state<'идёт' | 'победа' | 'поражение'>('идёт');

  const mine = $derived(legalMoves(board, 'w'));
  const mustTake = $derived(mine.length > 0 && mine[0].captured.length > 0);

  /** Ходы, которые ещё можно доиграть из текущего выбора. */
  const live = $derived(
    selected === null
      ? []
      : mine.filter(
          (move) =>
            move.from === selected && chain.every((cell, index) => move.path[index] === cell),
        ),
  );
  const targets = $derived(new Set(live.map((move) => move.path[chain.length]).filter((c) => c !== undefined)));
  const standing = $derived(chain.length > 0 ? chain[chain.length - 1] : selected);

  const status = $derived(
    result === 'победа'
      ? 'вы выиграли — замок открыт'
      : result === 'поражение'
        ? 'вам нечем ходить'
        : thinking
          ? 'думает…'
          : mustTake
            ? 'ваш ход: бить обязательно'
            : 'ваш ход, белые',
  );

  function finishTurn(next: Board): void {
    board = next;
    selected = null;
    chain = [];
    if (loser(board, 'b') === 'b') {
      result = 'победа';
      onwin();
      return;
    }
    thinking = true;
    setTimeout(async () => {
      const answer = await thinkCheckers([...board], 'b');
      if (answer) board = applyMove(board, answer);
      thinking = false;
      if (loser(board, 'w') === 'w') result = 'поражение';
      else if (loser(board, 'b') === 'b') {
        result = 'победа';
        onwin();
      }
    }, 90);
  }

  function click(cell: number): void {
    if (result !== 'идёт' || thinking) return;
    const piece = board[cell];

    if (piece && sideOf(piece) === 'w' && mine.some((move) => move.from === cell)) {
      selected = cell;
      chain = [];
      return;
    }
    if (selected === null || !targets.has(cell)) return;

    const next = [...chain, cell];
    const done = live.find(
      (move) => move.path.length === next.length && next.every((c, i) => move.path[i] === c),
    );
    if (done) {
      finishTurn(applyMove(board, done));
      return;
    }
    chain = next; // цепочка продолжается: бить надо до конца
  }

  function restart(): void {
    board = initialBoard();
    selected = null;
    chain = [];
    thinking = false;
    result = 'идёт';
  }

  /** Куда рисовать шашку: по ходу цепочки она уже «в руке» и стоит на пути. */
  function pieceAt(cell: number): Cell {
    if (chain.length > 0 && selected !== null) {
      if (cell === standing) return board[selected];
      if (cell === selected) return null;
    }
    return board[cell];
  }
</script>

<Screen
  title="Шашки"
  hint="русские · бить обязательно · дамка ходит через всю доску"
  {status}
  tone={result === 'победа' ? 'победа' : result === 'поражение' ? 'поражение' : 'обычный'}
  {onback}
>
  <div class="board">
    {#each board as _, cell (cell)}
      {@const piece = pieceAt(cell)}
      <button
        class="square"
        class:dark={dark(cell)}
        class:target={targets.has(cell)}
        class:standing={cell === standing}
        style="grid-area: {rowOf(cell) + 1} / {colOf(cell) + 1}"
        onclick={() => click(cell)}
        aria-label={`поле ${cell}`}
      >
        {#if piece}
          <span class="piece" class:black={sideOf(piece) === 'b'} class:king={isKing(piece)}>
            {#if isKing(piece)}<span class="crown">★</span>{/if}
          </span>
        {:else if targets.has(cell)}
          <span class="dot"></span>
        {/if}
      </button>
    {/each}
  </div>

  {#snippet controls()}
    <button class="ghost" onclick={restart}>заново</button>
  {/snippet}
</Screen>

<style>
  .board {
    display: grid;
    grid-template-columns: repeat(8, 1fr);
    grid-template-rows: repeat(8, 1fr);
    padding: 10px;
    border: 1px solid var(--edge);
    border-radius: 16px;
    background: linear-gradient(170deg, var(--raised), var(--surface));
    box-shadow: 0 30px 60px -40px rgba(0, 0, 0, 0.95);
  }

  .square {
    width: clamp(2.1rem, 1.2rem + 3.6vw, 3.6rem);
    aspect-ratio: 1;
    display: grid;
    place-items: center;
    background: rgba(233, 222, 210, 0.13);
    transition: background 140ms ease;
  }
  .square.dark {
    background: rgba(9, 7, 13, 0.62);
  }
  .square.target {
    background: rgba(232, 115, 74, 0.2);
  }
  .square.standing {
    box-shadow: inset 0 0 0 2px var(--gold);
  }
  /* Скругление достаётся только углам доски, а не каждой клетке */
  .square:first-child {
    border-radius: 8px 0 0 0;
  }
  .square:nth-child(8) {
    border-radius: 0 8px 0 0;
  }
  .square:nth-child(57) {
    border-radius: 0 0 0 8px;
  }
  .square:last-child {
    border-radius: 0 0 8px 0;
  }

  .piece {
    width: 74%;
    height: 74%;
    display: grid;
    place-items: center;
    border-radius: 50%;
    background: radial-gradient(circle at 34% 28%, #fffdfa, #cdc4b8 70%, #a49a8d);
    box-shadow: 0 3px 6px -2px rgba(0, 0, 0, 0.85), inset 0 0 0 2px rgba(255, 255, 255, 0.25);
    color: #6b5a3a;
    font-size: 0.7rem;
  }
  .piece.black {
    background: radial-gradient(circle at 34% 28%, #4a4150, #241f2c 68%, #15121b);
    box-shadow: 0 3px 6px -2px rgba(0, 0, 0, 0.9), inset 0 0 0 2px rgba(232, 115, 74, 0.35);
    color: var(--gold);
  }
  .crown {
    line-height: 1;
  }

  .dot {
    width: 26%;
    height: 26%;
    border-radius: 50%;
    background: var(--ember);
    opacity: 0.85;
  }

  .ghost {
    padding: 0.4rem 0.9rem;
    border: 1px solid var(--edge);
    border-radius: 8px;
    font-size: 0.84rem;
    color: var(--paper);
    transition: border-color 160ms ease, background 160ms ease;
  }
  .ghost:hover {
    border-color: var(--ember);
    background: rgba(232, 115, 74, 0.1);
  }
</style>
