<script lang="ts">
  import Screen from '../Screen.svelte';
  import { thinkMarks } from '../think';
  import {
    emptyBoard,
    full,
    SIZE,
    winner,
    type Board,
    type Win,
  } from './engine';

  interface Props {
    onwin: () => void;
    onback: () => void;
  }
  const { onwin, onback }: Props = $props();

  let board = $state<Board>(emptyBoard());
  let thinking = $state(false);
  let result = $state<'идёт' | 'победа' | 'поражение' | 'ничья'>('идёт');
  let win = $state<Win | null>(null);

  const status = $derived(
    result === 'победа'
      ? 'вы выиграли — замок открыт'
      : result === 'поражение'
        ? 'нолики собрали четыре'
        : result === 'ничья'
          ? 'ничья: поле кончилось'
          : thinking
            ? 'думает…'
            : 'ваш ход, крестики',
  );

  function settle(): boolean {
    const found = winner(board);
    if (found) {
      win = found;
      result = found.mark === 'x' ? 'победа' : 'поражение';
      if (found.mark === 'x') onwin();
      return true;
    }
    if (full(board)) {
      result = 'ничья';
      return true;
    }
    return false;
  }

  function play(cell: number): void {
    if (result !== 'идёт' || thinking || board[cell]) return;
    board[cell] = 'x';
    if (settle()) return;

    // Ход бота — следующим кадром: иначе свой крестик человек увидит уже
    // вместе с ответом и не поймёт, что вообще произошло.
    thinking = true;
    setTimeout(async () => {
      // Считает отдельный поток, и по обычной копии: доска в состоянии —
      // реактивный прокси, каждая пробная запись в него дороже перебора.
      const answer = await thinkMarks([...board], 'o');
      if (answer !== null) board[answer] = 'o';
      thinking = false;
      settle();
    }, 90);
  }

  function restart(): void {
    board = emptyBoard();
    result = 'идёт';
    win = null;
    thinking = false;
  }
</script>

<Screen
  title="Крестики-нолики"
  hint="поле 5×5 · линия из четырёх · вы ходите первым"
  {status}
  tone={result === 'победа' ? 'победа' : result === 'поражение' ? 'поражение' : 'обычный'}
  {onback}
>
  <div class="board" style="--size: {SIZE}">
    {#each board as cell, index (index)}
      <button
        class="cell"
        class:won={win?.cells.includes(index)}
        disabled={!!cell || result !== 'идёт' || thinking}
        onclick={() => play(index)}
        aria-label={`клетка ${index + 1}`}
      >
        {#if cell === 'x'}
          <svg viewBox="0 0 40 40" class="x"><path d="M11 11 L29 29 M29 11 L11 29" /></svg>
        {:else if cell === 'o'}
          <svg viewBox="0 0 40 40" class="o"><circle cx="20" cy="20" r="9.5" /></svg>
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
    grid-template-columns: repeat(var(--size), 1fr);
    gap: 6px;
    padding: 10px;
    border: 1px solid var(--edge);
    border-radius: 16px;
    background: linear-gradient(170deg, var(--raised), var(--surface));
    box-shadow: 0 30px 60px -40px rgba(0, 0, 0, 0.95);
  }

  .cell {
    width: clamp(3rem, 2rem + 4.5vw, 4.6rem);
    aspect-ratio: 1;
    display: grid;
    place-items: center;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.05);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.04);
    transition: background 140ms ease, transform 140ms ease;
  }
  .cell:hover:not(:disabled) {
    background: var(--lift-2);
    transform: translateY(-2px);
  }
  .cell:disabled {
    cursor: default;
  }
  /* Выигравшая линия — светом и рамкой: цвета в системе нет */
  .cell.won {
    background: rgba(255, 255, 255, 0.16);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.55);
  }

  svg {
    width: 62%;
    height: 62%;
    fill: none;
    stroke-width: 3.4;
    stroke-linecap: round;
  }
  /* Свой знак — белый, чужой — серый: различаются светлотой, а не цветом */
  .x {
    stroke: var(--accent);
  }
  .o {
    stroke: var(--muted);
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
    border-color: var(--paper);
    background: var(--lift);
  }
</style>
