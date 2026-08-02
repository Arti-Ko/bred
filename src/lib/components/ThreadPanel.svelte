<script lang="ts">
  import { session } from '../stores/session.svelte';
  import { chunks, timecode } from '../format';
</script>

{#if session.threadRoot}
  <aside class="thread" aria-label="Ветка обсуждения">
    <div class="pane-head">
      <span>ветка · {Math.max(0, session.thread.length - 1)} ответов</span>
      <button onclick={() => session.closeThread()}>закрыть ✕</button>
    </div>

    <div class="body">
      {#each session.thread as message, index (message.id)}
        <article class="row" class:root={index === 0}>
          <div class="head">
            <span class="who">{message.nick}</span>
            <time>{timecode(message.ts)}</time>
          </div>
          {#each chunks(message.body) as part (part.text)}
            {#if part.code}
              <pre class="selectable">{part.text}</pre>
            {:else}
              <div class="text selectable">{part.text}</div>
            {/if}
          {/each}
        </article>
        {#if index === 0}
          <div class="split">─── ответы ───</div>
        {/if}
      {/each}

      {#if session.thread.length <= 1}
        <p class="empty">ответов пока нет. всё, что напишете сейчас, уйдёт в ветку.</p>
      {/if}
    </div>
  </aside>
{/if}

<style>
  .thread {
    display: flex;
    flex-direction: column;
    min-height: 0;
    max-height: 40vh;
    border-top: 1px solid var(--line);
    background: var(--bg-raised);
  }

  .pane-head {
    display: flex;
    justify-content: space-between;
    padding: 5px var(--gap-4);
    border-bottom: 1px solid var(--line);
    color: var(--fg-dimmer);
    font-size: var(--text-label);
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }
  .pane-head button {
    color: var(--fg-dim);
    font-size: var(--text-xs);
  }
  .pane-head button:hover {
    color: var(--fg-hi);
  }

  .body {
    flex: 1;
    overflow-y: auto;
    padding: var(--gap-3) 0;
  }

  .row {
    padding: 4px 14px 6px;
    border-left: 2px solid transparent;
  }
  /* Корень ветки выделен: без этого непонятно, к чему всё относится */
  .row.root {
    border-left-color: var(--fg);
  }

  .head {
    display: flex;
    align-items: baseline;
    gap: 9px;
  }
  .who {
    color: var(--fg-hi);
    font-weight: 700;
  }
  time {
    margin-left: auto;
    color: var(--fg-faint);
    font-size: var(--text-xs);
  }

  .text {
    color: var(--fg);
    white-space: pre-wrap;
    word-break: break-word;
    max-width: 82ch;
  }

  pre {
    margin: 6px 0 2px;
    border: 1px solid var(--line);
    padding: 7px 11px;
    font-size: var(--text-sm);
    overflow-x: auto;
    background: var(--bg);
  }

  .split {
    padding: 6px 14px;
    color: var(--fg-faint);
    font-size: var(--text-xs);
    letter-spacing: 0.14em;
  }

  .empty {
    margin: 0;
    padding: 6px 14px;
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
  }
</style>
