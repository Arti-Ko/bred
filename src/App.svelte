<script lang="ts">
  import CallPanel from './lib/components/CallPanel.svelte';
  import ChannelPane from './lib/components/ChannelPane.svelte';
  import Feed from './lib/components/Feed.svelte';
  import MembersPane from './lib/components/MembersPane.svelte';
  import CommandBar from './lib/components/CommandBar.svelte';
  import EmojiPicker from './lib/components/EmojiPicker.svelte';
  import Settings from './lib/components/Settings.svelte';
  import ThreadPanel from './lib/components/ThreadPanel.svelte';
  import { call } from './lib/stores/call.svelte';
  import { session } from './lib/stores/session.svelte';

  let draft = $state('');
  let input: HTMLInputElement | undefined = $state();
  /** История ввода, как в шелле: ↑/↓ листают отправленное. */
  let history = $state<string[]>([]);
  let cursor = $state(-1);
  let lastTyping = 0;
  let pickerOpen = $state(false);
  let settingsOpen = $state(false);

  $effect(() => {
    void session.init();
  });

  function submit() {
    const text = draft.trim();
    // Пустая строка при прикреплённом файле — это «отправить только файл»,
    // а не «ничего не делать». Раньше здесь стоял ранний выход, и одно
    // вложение без подписи отправить было нельзя.
    if (!text && session.pending.length === 0) return;
    if (text) {
      history = [text, ...history].slice(0, 50);
      cursor = -1;
    }
    draft = '';
    void session.send(text);
  }

  function onKey(event: KeyboardEvent) {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      submit();
      return;
    }
    if (event.key === 'ArrowUp' && cursor + 1 < history.length) {
      event.preventDefault();
      cursor += 1;
      draft = history[cursor];
      return;
    }
    if (event.key === 'ArrowDown' && cursor >= 0) {
      event.preventDefault();
      cursor -= 1;
      draft = cursor >= 0 ? history[cursor] : '';
      return;
    }
    if (event.key === 'Escape') {
      if (session.threadRoot) session.closeThread();
      session.replyTo = null;
      session.status = '';
    }
  }

  function onInput() {
    // Не чаще раза в 2 секунды: «печатает» — подсказка, а не телеметрия.
    const now = Date.now();
    if (draft && now - lastTyping > 2000) {
      lastTyping = now;
      session.notifyTyping();
    }
  }

  /** Горячие клавиши приложения. Всё, что делается мышью, делается и с клавиатуры. */
  function onGlobalKey(event: KeyboardEvent) {
    const mod = event.ctrlKey || event.metaKey;
    if (mod && event.key.toLowerCase() === 'k') {
      event.preventDefault();
      input?.focus();
      return;
    }
    if (mod && event.key.toLowerCase() === 'i') {
      event.preventDefault();
      void session.invite();
      return;
    }
    if (mod && event.key === ',') {
      event.preventDefault();
      settingsOpen = !settingsOpen;
      return;
    }
    if (mod && event.key.toLowerCase() === 'u') {
      event.preventDefault();
      void session.attach();
      return;
    }
    if (mod && event.key.toLowerCase() === 'd') {
      event.preventDefault();
      if (call.active) call.toggleMic();
      return;
    }
    if (mod && event.key.toLowerCase() === 'e') {
      event.preventDefault();
      if (call.active) void call.leave();
      return;
    }
    // Ctrl+1..9 — переключение каналов по номеру, как вкладки в терминале.
    if (mod && /^[1-9]$/.test(event.key)) {
      const index = Number(event.key) - 1;
      const channel = session.channels[index];
      if (channel) {
        event.preventDefault();
        void session.selectChannel(channel.id);
      }
    }
  }
</script>

<svelte:window onkeydown={onGlobalKey} />

{#if settingsOpen}
  <Settings onclose={() => (settingsOpen = false)} />
{/if}

<div class="term">
  <header class="wins">
    <span class="brand">БРЕД</span>
    {#each session.spaces as space (space.id)}
      <button
        aria-current={space.id === session.spaceId ? 'true' : undefined}
        onclick={() => session.selectSpace(space.id)}
      >
        {space.name}{space.unread > 0 ? '*' : ''}
      </button>
    {/each}
    <span class="sp"></span>
    <span class="meta">{session.online ? 'сеть: p2p' : 'сеть: поднимается'}</span>
    <button class="gear" onclick={() => (settingsOpen = true)} title="настройки [^,]">⚙</button>
  </header>

  <div class="panes">
    <ChannelPane />
    <div class="middle">
      <CallPanel />
      <Feed />
      <ThreadPanel />
    </div>
    <MembersPane />
  </div>

  <CommandBar
    onpick={(command) => {
      // Готовую команду отправляем сразу, командам с аргументом даём дописать.
      if (command.endsWith(' ')) {
        draft = command;
        input?.focus();
      } else {
        void session.send(command);
      }
    }}
  />

  {#if session.pending.length > 0}
    <div class="pending">
      прикреплено:
      {#each session.pending as file (file.hash)}
        <button onclick={() => session.dropAttachment(file.hash)} title="убрать">
          {file.name} ✕
        </button>
      {/each}
    </div>
  {/if}

  {#if session.replyTo}
    <div class="reply">
      ответ → <b>{session.replyTo.nick}</b>: {session.replyTo.body.slice(0, 80)}
      <button onclick={() => (session.replyTo = null)}>esc отменить</button>
    </div>
  {/if}

  <div class="prompt">
    <button class="clip" onclick={() => session.attach()} title="прикрепить файл [^U]">+</button>
    <div class="picker-anchor">
      <button class="clip" onclick={() => (pickerOpen = !pickerOpen)} title="эмодзи">☺</button>
      {#if pickerOpen}
        <EmojiPicker
          onpick={(emoji) => {
            draft += emoji;
            pickerOpen = false;
            input?.focus();
          }}
          onclose={() => (pickerOpen = false)}
        />
      {/if}
    </div>
    <span class="p"
      >{session.nick}@{session.threadRoot
        ? 'ветка'
        : (session.space?.name ?? 'бред')} ❯</span
    >
    <input
      bind:this={input}
      bind:value={draft}
      oninput={onInput}
      onkeydown={onKey}
      placeholder="сообщение или /команда"
      aria-label="Сообщение"
      spellcheck="false"
      autocomplete="off"
    />
    <span class="cur blink"></span>
    <span class="c">{draft.length}/2000</span>
  </div>

  <footer class="status">
    <span class="mode">— ВВОД —</span>
    <span class="s">{session.channel ? `#${session.channel.name}` : 'нет канала'}</span>
    <span class="s">{session.members.filter((m) => m.online).length} online</span>
    <span class="s">{session.unreadTotal} непрочитанных</span>
    <span class="sp"></span>
    {#if session.status}
      <span class="r warn">{session.status}</span>
    {:else}
      <span class="r">^, настройки · ^U файл · ^I позвать{call.active
          ? ' · ^D микрофон · ^E выйти'
          : ''} · ⏎ отправить</span>
    {/if}
  </footer>
</div>

<style>
  .term {
    height: 100vh;
    display: grid;
    grid-template-rows: auto minmax(0, 1fr) auto auto;
  }

  .wins {
    display: flex;
    align-items: stretch;
    border-bottom: 1px solid var(--line);
    font-size: var(--text-sm);
  }
  .brand {
    padding: 5px 12px;
    color: var(--fg-hi);
    font-weight: 700;
    letter-spacing: 0.14em;
  }
  .wins button {
    color: var(--fg-dim);
    padding: 5px 11px;
    font-size: var(--text-sm);
    transition: color var(--fast) var(--ease);
  }
  .wins button:hover {
    color: var(--fg-hi);
  }
  .wins button[aria-current='true'] {
    background: var(--inv-bg);
    color: var(--inv-fg);
    font-weight: 700;
  }
  .sp {
    flex: 1;
  }
  .gear {
    padding: 5px 12px;
    color: var(--fg-dimmer);
  }
  .gear:hover {
    color: var(--fg-hi);
  }

  .meta {
    padding: 5px 12px;
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
  }

  .panes {
    display: grid;
    grid-template-columns: var(--pane-left) minmax(0, 1fr) var(--pane-right);
    min-height: 0;
  }

  /* Панель звонка живёт над лентой, а не поверх: перекрывать текст нельзя */
  .middle {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .pending {
    border-top: 1px solid var(--line);
    padding: 4px 12px;
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
    display: flex;
    gap: var(--gap-3);
    flex-wrap: wrap;
    align-items: center;
  }
  .pending button {
    border: 1px solid var(--fg-faint);
    color: var(--fg);
    padding: 0 7px;
    font-size: var(--text-xs);
  }
  .pending button:hover {
    border-color: var(--fg);
    color: var(--fg-hi);
  }

  .picker-anchor {
    position: relative;
    flex: none;
  }

  .clip {
    color: var(--fg-dim);
    border: 1px solid var(--fg-faint);
    width: 20px;
    height: 20px;
    line-height: 1;
    flex: none;
  }
  .clip:hover {
    color: var(--fg-hi);
    border-color: var(--fg-dim);
  }

  .reply {
    border-top: 1px solid var(--line);
    padding: 4px 12px;
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
  }
  .reply b {
    color: var(--fg);
    font-weight: 400;
  }
  .reply button {
    color: var(--fg-dim);
    margin-left: var(--gap-3);
    text-decoration: underline;
    text-underline-offset: 3px;
  }

  .prompt {
    display: flex;
    align-items: center;
    gap: var(--gap-4);
    padding: 9px 12px;
    border-top: 1px solid var(--line);
  }
  .prompt .p {
    color: var(--fg-hi);
    font-weight: 700;
    white-space: nowrap;
  }
  .prompt input {
    flex: 1;
    min-width: 0;
  }
  .prompt input::placeholder {
    color: var(--fg-faint);
  }
  .cur {
    width: 8px;
    height: 16px;
    background: var(--fg);
    flex: none;
  }
  .prompt .c {
    color: var(--fg-faint);
    font-size: var(--text-xs);
  }

  .status {
    display: flex;
    align-items: stretch;
    background: var(--inv-bg);
    color: var(--inv-fg);
    font-size: var(--text-sm);
    font-weight: 700;
  }
  .status .mode {
    background: var(--bg);
    color: var(--fg);
    padding: 4px 12px;
    letter-spacing: 0.2em;
  }
  .status .s {
    padding: 4px 12px;
    border-right: 1px solid rgba(10, 10, 10, 0.22);
  }
  .status .r {
    padding: 4px 12px;
    border-left: 1px solid rgba(10, 10, 10, 0.22);
    font-weight: 400;
  }
  /* Ошибка тоже без цвета — только жирным. Инверсия и так предельно заметна */
  .status .warn {
    font-weight: 700;
  }

  @media (max-width: 1180px) {
    .panes {
      grid-template-columns: var(--pane-left) minmax(0, 1fr);
    }
    .panes :global(.right) {
      display: none;
    }
  }
  @media (max-width: 860px) {
    .panes {
      grid-template-columns: minmax(0, 1fr);
    }
    .panes :global(.left) {
      display: none;
    }
  }
</style>
