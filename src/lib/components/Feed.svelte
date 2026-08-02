<script lang="ts">
  import Message from './Message.svelte';
  import { session } from '../stores/session.svelte';
  import { dayStamp } from '../format';

  let viewport: HTMLElement | undefined = $state();
  let selectedId = $state<string | null>(null);
  /** Прокручиваем вниз только если пользователь и так был внизу. */
  let pinned = $state(true);

  const messages = $derived(session.messages);

  function onScroll() {
    if (!viewport) return;
    const distance = viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight;
    pinned = distance < 80;

    // Подошли к верхнему краю — тянем предыдущую страницу.
    if (viewport.scrollTop < 120) void loadOlder();
  }

  /**
   * Догрузка вверх с сохранением позиции: без поправки на приросшую высоту
   * содержимое прыгало бы под курсором при каждой подгрузке.
   */
  async function loadOlder() {
    if (!viewport || session.loadingOlder || !session.hasOlder) return;
    const before = viewport.scrollHeight;
    const added = await session.loadOlder();
    if (added > 0) {
      requestAnimationFrame(() => {
        if (viewport) viewport.scrollTop += viewport.scrollHeight - before;
      });
    }
  }

  $effect(() => {
    // Зависимость от длины: перематываем при новых сообщениях, а не при любом рендере.
    messages.length;
    if (pinned && viewport) {
      requestAnimationFrame(() => {
        if (viewport) viewport.scrollTop = viewport.scrollHeight;
      });
    }
  });

  function toggle(id: string) {
    selectedId = selectedId === id ? null : id;
  }

  /** Разделитель дня вставляем, когда дата отличается от предыдущего сообщения. */
  function dayBreak(index: number): string | null {
    const current = dayStamp(messages[index].ts);
    if (index === 0) return current;
    return dayStamp(messages[index - 1].ts) === current ? null : current;
  }
</script>

<main class="pane">
  <div class="pane-head">
    <span>
      {#if session.channel}
        #{session.channel.name}
      {:else}
        канал не выбран
      {/if}
    </span>
    <b>{session.members.filter((m) => m.online).length} online</b>
  </div>

  <div class="log" bind:this={viewport} onscroll={onScroll} role="listbox" tabindex="-1" aria-label="Лента сообщений">
    {#if session.hasOlder}
      <button class="older" onclick={loadOlder} disabled={session.loadingOlder}>
        {session.loadingOlder ? 'грузим…' : '↑ показать более раннее'}
      </button>
    {/if}

    {#if messages.length === 0}
      <p class="empty">
        {#if session.channel}
          пусто. напишите первое сообщение — оно уйдёт соседям, как только они появятся
        {:else}
          создайте пространство командой <code>/простор Название</code> или войдите
          по ссылке: <code>/войти bred://join/…</code>
        {/if}
      </p>
    {/if}

    {#each messages as message, index (message.id)}
      {@const stamp = dayBreak(index)}
      {#if stamp}
        <div class="day">{stamp}</div>
      {/if}
      <div
        role="option"
        aria-selected={selectedId === message.id}
        tabindex="0"
        onclick={() => toggle(message.id)}
        onkeydown={(event) => event.key === 'Enter' && toggle(message.id)}
      >
        <Message
          {message}
          mine={message.author === session.me}
          selected={selectedId === message.id}
          onreact={(emoji) => session.toggleReaction(message, emoji)}
          onreply={() => (session.replyTo = message)}
          ondownload={(hash) => session.download(hash)}
          onthread={() => session.openThread(message.id)}
        />
      </div>
    {/each}

    {#if session.typing.length > 0}
      <div class="typing">
        {session.typing.join(', ')} печата{session.typing.length > 1 ? 'ют' : 'ет'}<i class="blink"
        ></i>
      </div>
    {/if}
  </div>
</main>

<style>
  .log {
    flex: 1;
    overflow-y: auto;
    padding: 10px 0 4px;
  }

  .day {
    color: var(--fg-faint);
    font-size: var(--text-xs);
    padding: 8px 14px 10px;
    letter-spacing: 0.12em;
    overflow: hidden;
    white-space: nowrap;
  }
  .day::after {
    content: ' ──────────────────────────────────────────────────────────────────────';
  }

  .older {
    display: block;
    width: 100%;
    padding: 6px 14px;
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
    text-align: left;
    border-bottom: 1px solid var(--line);
  }
  .older:hover:not(:disabled) {
    color: var(--fg-hi);
    background: var(--bg-raised);
  }

  .empty {
    color: var(--fg-dimmer);
    padding: 16px 14px;
    max-width: 70ch;
    line-height: 1.7;
  }
  .empty code {
    color: var(--fg);
    border: 1px solid var(--line);
    padding: 0 4px;
  }

  .typing {
    padding: 4px 14px 8px;
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
  }
  .typing i {
    display: inline-block;
    width: 8px;
    height: 13px;
    background: var(--fg-dim);
    vertical-align: -2px;
    margin-left: 5px;
  }
</style>
