<script lang="ts">
  // Карточка входящего зова. Одна на оба оформления: зов важнее любого из них,
  // и выглядеть он должен одинаково — как то, что надо заметить сразу.
  import { call } from '../stores/call.svelte';
  import { prefs } from '../stores/prefs.svelte';
  import { ringing } from '../stores/ringing.svelte';

  $effect(() => {
    // Подписка приезжает асинхронно. Если компонент успел уйти раньше, она
    // осталась бы висеть, и каждый зов показывался бы дважды.
    let stop: (() => void) | null = null;
    let disposed = false;
    void ringing
      .listen()
      .then((unlisten) => {
        if (disposed) unlisten();
        else stop = unlisten;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      stop?.();
    };
  });

  // Зашли в комнату сами — зовы туда гаснут: второй раз туда не войти.
  $effect(() => {
    if (call.active && call.space && call.channel) ringing.joined(call.space, call.channel);
  });
</script>

{#if ringing.incoming.length > 0}
  <div class="calls" role="region" aria-label="Входящие зовы" aria-live="assertive">
    {#each ringing.incoming as ring (ring.key)}
      <article class="call" class:soft={prefs.adequate}>
        <span class="pulse" aria-hidden="true"></span>
        <div class="text">
          <b>{ring.nick}</b>
          <span>зовёт в «{ring.room}»</span>
          <small>{ring.spaceName}</small>
        </div>
        <div class="actions">
          <button class="join" onclick={() => ringing.accept(ring)}>
            {prefs.adequate ? 'Подключиться' : 'подключиться'}
          </button>
          <button class="later" onclick={() => ringing.decline(ring)}>
            {prefs.adequate ? 'Не сейчас' : 'не сейчас'}
          </button>
        </div>
      </article>
    {/each}
  </div>
{/if}

<style>
  .calls {
    position: fixed;
    top: 14px;
    left: 50%;
    z-index: 120;
    transform: translateX(-50%);
    display: grid;
    gap: 8px;
    width: min(26rem, calc(100vw - 2rem));
  }

  .call {
    display: grid;
    grid-template-columns: auto 1fr;
    align-items: center;
    gap: 6px 14px;
    padding: 12px 14px;
    background: var(--bg-raised);
    border: 1px solid var(--fg);
    box-shadow: 0 18px 40px -18px rgba(0, 0, 0, 0.9);
    animation: arrive var(--fast) var(--ease);
  }
  .call.soft {
    border-color: var(--edge, var(--fg-dim));
    border-radius: 14px;
  }

  /* Звонок «дышит»: кольцо расходится, как волна от трубки. Цвета в системе
     нет — заметность держится на движении и контрасте. */
  .pulse {
    position: relative;
    margin: 0 4px 0 6px;
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: var(--fg-hi);
  }
  .pulse::after {
    content: '';
    position: absolute;
    inset: -6px;
    border-radius: 50%;
    border: 1px solid var(--fg-hi);
    animation: ring 1.2s ease-out infinite;
  }

  .text {
    display: grid;
    gap: 1px;
    min-width: 0;
  }
  .text b {
    color: var(--fg-hi);
    font-size: var(--text-md, 15px);
  }
  .text span {
    color: var(--fg);
    font-size: var(--text-sm);
  }
  .text small {
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
  }

  .actions {
    grid-column: 1 / -1;
    display: flex;
    gap: 8px;
  }
  .actions button {
    flex: 1;
    padding: 6px 10px;
    font-size: var(--text-sm);
    border: 1px solid var(--fg-faint);
  }
  .actions .join {
    background: var(--inv-bg);
    color: var(--inv-fg);
    border-color: var(--inv-bg);
    font-weight: 700;
  }
  .actions .later {
    color: var(--fg-dim);
  }
  .actions .later:hover {
    color: var(--fg-hi);
    border-color: var(--fg-dim);
  }
  .soft .actions button {
    border-radius: 8px;
  }

  @keyframes ring {
    from {
      transform: scale(0.6);
      opacity: 1;
    }
    to {
      transform: scale(1.8);
      opacity: 0;
    }
  }
  @keyframes arrive {
    from {
      transform: translateY(-8px);
      opacity: 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .pulse::after,
    .call {
      animation: none;
    }
  }
</style>
