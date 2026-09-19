<script lang="ts">
  // Ветка — отдельная колонка справа, а не полоска внизу экрана.
  //
  // Так она и устроена в голове у человека: разговор в сторону от общей ленты,
  // который видно рядом с ней, а не вместо неё.

  import Avatar from './Avatar.svelte';
  import Icon from './Icon.svelte';
  import { chunks, timecode } from '../format';
  import { session } from '../stores/session.svelte';

  const replies = $derived(session.thread.slice(1));
  const root = $derived(session.thread[0]);

  /** «1 ответ», «2 ответа», «5 ответов». */
  function word(count: number): string {
    const tail = count % 100;
    if (tail >= 11 && tail <= 14) return 'ответов';
    switch (count % 10) {
      case 1:
        return 'ответ';
      case 2:
      case 3:
      case 4:
        return 'ответа';
      default:
        return 'ответов';
    }
  }
</script>

<aside class="thread-pane" aria-label="Ветка обсуждения">
  <div class="head">
    <h3>Ветка</h3>
    <span class="count">{replies.length} {word(replies.length)}</span>
    <button class="icon-btn" onclick={() => session.closeThread()} aria-label="Закрыть ветку">
      <Icon name="close" size={13} />
    </button>
  </div>

  <div class="body">
    {#if root}
      <article class="msg root">
        <Avatar id={root.author} nick={root.nick} />
        <div class="side">
          <div class="who"><b>{root.nick}</b><time>{timecode(root.ts)}</time></div>
          {#each chunks(root.body) as part (part.text)}
            {#if part.code}
              <pre class="selectable">{part.text}</pre>
            {:else}
              <div class="text selectable">{part.text}</div>
            {/if}
          {/each}
        </div>
      </article>
      <div class="split">
        <span>{replies.length > 0 ? 'ответы' : 'ответов пока нет'}</span>
      </div>
    {/if}

    {#each replies as message (message.id)}
      <article class="msg">
        <Avatar id={message.author} nick={message.nick} size="sm" />
        <div class="side">
          <div class="who"><b>{message.nick}</b><time>{timecode(message.ts)}</time></div>
          {#each chunks(message.body) as part (part.text)}
            {#if part.code}
              <pre class="selectable">{part.text}</pre>
            {:else}
              <div class="text selectable">{part.text}</div>
            {/if}
          {/each}
        </div>
      </article>
    {/each}

    {#if replies.length === 0}
      <p class="empty">
        Всё, что вы напишете в поле внизу, уйдёт сюда, а не в общий канал.
      </p>
    {/if}
  </div>
</aside>

<style>
  /* Не `.thread`: так называется кнопка ветки в сообщении, и общее правило
     из adequate.css превращало бы колонку в пилюлю. */
  .thread-pane {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-left: 1px solid var(--line);
    background: rgba(0, 0, 0, 0.35);
  }

  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 8px 8px 12px;
    border-bottom: 1px solid var(--line);
  }
  h3 {
    margin: 0;
    font-size: 11px;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--fg-dimmer);
    font-weight: 600;
  }
  .count {
    flex: 1;
    color: var(--fg-dim);
    font-size: var(--text-sm);
  }

  .body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 8px;
  }

  .msg {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 10px;
    padding: 7px 8px;
    border-radius: var(--radius-sm);
  }
  .msg:hover {
    background: var(--lift);
  }
  .msg.root {
    background: var(--lift);
  }
  .side {
    min-width: 0;
  }
  .who {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  .who b {
    color: var(--fg-hi);
    font-weight: 620;
    font-size: var(--text);
  }
  time {
    color: var(--fg-faint);
    font-family: var(--mono);
    font-size: var(--text-xs);
  }
  .text {
    color: var(--fg);
    white-space: pre-wrap;
    word-break: break-word;
  }
  pre {
    margin: 6px 0 0;
    padding: 8px 10px;
    overflow-x: auto;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--bg);
    font-family: var(--mono);
    font-size: var(--text-sm);
  }

  .split {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 8px 8px;
    color: var(--fg-dimmer);
    font-size: 11px;
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }
  .split::before,
  .split::after {
    content: '';
    flex: 1;
    height: 1px;
    background: var(--line);
  }

  .empty {
    margin: 8px;
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
    line-height: 1.5;
  }
</style>
