<script lang="ts">
  // Общая рама для игры: путь назад, название, строка состояния.
  // Одна на три игры — чтобы они выглядели одним залом, а не тремя сайтами.
  import type { Snippet } from 'svelte';

  interface Props {
    title: string;
    hint: string;
    status: string;
    tone?: 'обычный' | 'победа' | 'поражение';
    onback: () => void;
    children: Snippet;
    controls?: Snippet;
  }

  const { title, hint, status, tone = 'обычный', onback, children, controls }: Props = $props();
</script>

<div class="screen">
  <header>
    <button class="back" onclick={onback}>← в зал</button>
    <div class="titles">
      <h2>{title}</h2>
      <span class="hint">{hint}</span>
    </div>
  </header>

  <div class="stage">
    {@render children()}
  </div>

  <footer>
    <span class="status" class:win={tone === 'победа'} class:lose={tone === 'поражение'}>
      {status}
    </span>
    <span class="sp"></span>
    {#if controls}{@render controls()}{/if}
  </footer>
</div>

<style>
  .screen {
    display: flex;
    flex-direction: column;
    min-height: 100vh;
    max-width: 68rem;
    margin: 0 auto;
    padding: clamp(1.2rem, 0.8rem + 2vw, 2.4rem) clamp(1rem, 0.6rem + 2vw, 2.5rem) 2rem;
  }

  header {
    display: grid;
    gap: 0.45rem;
    justify-items: start;
  }

  .titles {
    display: flex;
    align-items: baseline;
    gap: 0.8rem;
    flex-wrap: wrap;
  }

  .back {
    font-size: 0.82rem;
    letter-spacing: 0.06em;
    color: var(--muted);
    padding: 0.3rem 0.55rem;
    margin-left: -0.55rem;
    border-radius: 6px;
    transition: color 140ms ease, background 140ms ease;
  }
  .back:hover {
    color: var(--paper);
    background: rgba(255, 255, 255, 0.06);
  }

  h2 {
    margin: 0;
    font-size: clamp(1.4rem, 1.1rem + 1.2vw, 2rem);
    font-weight: 620;
    letter-spacing: -0.02em;
  }

  .hint {
    color: var(--muted);
    font-size: 0.85rem;
  }

  .stage {
    flex: 1;
    display: grid;
    place-items: center;
    padding: clamp(1rem, 0.6rem + 2vw, 2rem) 0;
  }

  footer {
    display: flex;
    align-items: center;
    gap: 0.8rem;
    padding-top: 1rem;
    border-top: 1px solid var(--edge);
  }

  .status {
    font-size: 0.95rem;
    color: var(--muted);
  }
  .status.win {
    color: var(--accent);
    font-weight: 700;
  }
  .status.lose {
    color: var(--paper);
    font-weight: 600;
  }

  .sp {
    flex: 1;
  }
</style>
