<script lang="ts">
  // Поле ввода: скрепка, эмодзи, текст, отправка.
  //
  // Команды никуда не делись — строка, начинающаяся со слэша, уходит тем же
  // путём. Но знать их больше не нужно: всё, что они делали, есть кнопками.

  import EmojiPicker from '../components/EmojiPicker.svelte';
  import Icon from './Icon.svelte';
  import { session } from '../stores/session.svelte';

  let draft = $state('');
  let input: HTMLTextAreaElement | undefined = $state();
  let picker = $state(false);
  /** История отправленного: ↑ и ↓ листают её, как в шелле. */
  let history = $state<string[]>([]);
  let cursor = $state(-1);
  let lastTyping = 0;

  const channel = $derived(session.channel);

  export function focus(): void {
    input?.focus();
  }

  function grow(): void {
    if (!input) return;
    input.style.height = 'auto';
    input.style.height = `${Math.min(input.scrollHeight, 160)}px`;
  }

  function submit(): void {
    const text = draft.trim();
    // Пустая строка при прикреплённом файле — это «отправить только файл».
    if (!text && session.pending.length === 0) return;
    if (text) {
      history = [text, ...history].slice(0, 50);
      cursor = -1;
    }
    draft = '';
    queueMicrotask(grow);
    void session.send(text);
  }

  function onKey(event: KeyboardEvent): void {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      submit();
      return;
    }
    if (event.key === 'ArrowUp' && draft === '' && cursor + 1 < history.length) {
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
    }
  }

  function onInput(): void {
    grow();
    // Не чаще раза в две секунды: «печатает» — подсказка, а не телеметрия.
    const now = Date.now();
    if (draft && now - lastTyping > 2000) {
      lastTyping = now;
      session.notifyTyping();
    }
  }
</script>

<div class="composer-wrap">
  {#if session.threadRoot}
    <div class="strip">
      <span class="label">Ветка</span>
      <span class="quote">ответ уйдёт в ветку, а не в общий канал</span>
      <button class="icon-btn" onclick={() => session.closeThread()} aria-label="Закрыть ветку">
        <Icon name="close" size={13} />
      </button>
    </div>
  {/if}

  {#if session.replyTo}
    <div class="strip">
      <span class="label">Ответ</span>
      <b>{session.replyTo.nick}</b>
      <span class="quote">{session.replyTo.body.slice(0, 90)}</span>
      <button class="icon-btn" onclick={() => (session.replyTo = null)} aria-label="Отменить ответ">
        <Icon name="close" size={13} />
      </button>
    </div>
  {/if}

  {#if session.pending.length > 0}
    <div class="strip">
      <span class="label">Прикреплено</span>
      {#each session.pending as file (file.hash)}
        <button class="chip" onclick={() => session.dropAttachment(file.hash)} title="Убрать">
          {file.name} <Icon name="close" size={11} />
        </button>
      {/each}
    </div>
  {/if}

  <div class="composer">
    <button class="icon-btn" onclick={() => session.attach()} title="Прикрепить файл" aria-label="Прикрепить файл">
      <Icon name="clip" />
    </button>
    <div class="anchor">
      <button class="icon-btn" onclick={() => (picker = !picker)} title="Эмодзи и стикеры" aria-label="Эмодзи">
        <Icon name="smile" />
      </button>
      {#if picker}
        <EmojiPicker
          onpick={(emoji) => {
            draft += emoji;
            picker = false;
            input?.focus();
          }}
          onclose={() => (picker = false)}
        />
      {/if}
    </div>

    <textarea
      bind:this={input}
      bind:value={draft}
      rows="1"
      onkeydown={onKey}
      oninput={onInput}
      placeholder={session.threadRoot
        ? 'Ответить в ветке…'
        : channel
          ? `Написать в #${channel.name}…`
          : 'Выберите канал слева или создайте свой'}
      aria-label="Сообщение"
    ></textarea>

    <button
      class="send"
      onclick={submit}
      disabled={!draft.trim() && session.pending.length === 0}
      aria-label="Отправить"
      title="Отправить · Enter"
    >
      <Icon name="send" size={16} />
    </button>
  </div>

  {#if session.typing.length > 0}
    <div class="typing">
      {session.typing.join(', ')} печата{session.typing.length > 1 ? 'ют' : 'ет'}…
    </div>
  {/if}
</div>

<style>
  .composer-wrap {
    padding: 0 14px 12px;
  }

  .strip {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    margin-bottom: 6px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--bg-raised);
    color: var(--fg-dim);
    font-size: var(--text-sm);
  }
  .strip .label {
    color: var(--fg-dimmer);
    font-size: 11px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }
  .strip b {
    color: var(--fg-hi);
    font-weight: 600;
  }
  .strip .quote {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 9px;
    border: 1px solid var(--line);
    border-radius: 999px;
    color: var(--fg);
    font-size: var(--text-xs);
  }
  .chip:hover {
    border-color: var(--fg-dim);
    color: var(--fg-hi);
  }

  .composer {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 7px 8px 7px 10px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--bg-raised);
    box-shadow: 0 16px 36px -30px #000;
  }
  .composer:focus-within {
    border-color: var(--edge);
  }
  .anchor {
    position: relative;
    flex: none;
  }

  textarea {
    flex: 1;
    min-width: 0;
    max-height: 160px;
    padding: 7px 4px;
    resize: none;
    line-height: 1.5;
    font: inherit;
    color: var(--fg);
    background: none;
    border: 0;
    outline: none;
  }
  textarea::placeholder {
    color: var(--fg-faint);
  }

  .send {
    width: 34px;
    height: 34px;
    flex: none;
    display: grid;
    place-items: center;
    border-radius: var(--radius-sm);
    background: var(--inv-bg);
    color: var(--inv-fg);
    transition: opacity var(--fast) var(--ease);
  }
  .send:disabled {
    opacity: 0.35;
    cursor: default;
  }

  .typing {
    padding: 5px 4px 0;
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
  }
</style>
