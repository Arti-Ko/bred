<script lang="ts">
  // Плашка «вышло обновление» поверх всего: канал нельзя заглушить, и о новом
  // выпуске человек узнаёт, даже если смотрит в другой канал.
  import { news } from '../stores/news.svelte';
  import { prefs } from '../stores/prefs.svelte';
</script>

{#if news.fresh && !news.open}
  <div class="toast" class:soft={prefs.adequate} role="status">
    <span class="mark" aria-hidden="true">✦</span>
    <div class="text">
      <b>{news.fresh.title}</b>
      <span>вышло обновление {news.fresh.version}</span>
    </div>
    <button class="open" onclick={() => news.show()}>Что нового</button>
  </div>
{/if}

<style>
  .toast {
    position: fixed;
    right: 16px;
    bottom: 16px;
    z-index: 115;
    display: flex;
    align-items: center;
    gap: 12px;
    max-width: min(26rem, calc(100vw - 2rem));
    padding: 10px 12px;
    background: var(--bg-raised);
    border: 1px solid var(--fg);
    box-shadow: 0 18px 40px -18px rgba(0, 0, 0, 0.9);
    animation: arrive var(--fast) var(--ease);
  }
  .toast.soft {
    border-radius: 14px;
    border-color: var(--edge, var(--fg-dim));
  }
  .mark {
    color: var(--fg-hi);
    font-size: 18px;
  }
  .text {
    display: grid;
    min-width: 0;
  }
  .text b {
    color: var(--fg-hi);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .text span {
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
  }
  .open {
    flex: 0 0 auto;
    padding: 5px 11px;
    background: var(--inv-bg);
    color: var(--inv-fg);
    font-size: var(--text-sm);
    font-weight: 700;
  }
  .soft .open {
    border-radius: 8px;
  }
  @keyframes arrive {
    from {
      transform: translateY(8px);
      opacity: 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .toast {
      animation: none;
    }
  }
</style>
