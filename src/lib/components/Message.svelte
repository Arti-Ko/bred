<script lang="ts">
  import type { MessageRow } from '../ipc';
  import { chunks, loneSticker, timecode, tokenize } from '../format';
  import { session } from '../stores/session.svelte';
  import { isViewable, previewUrl } from '../previews';
  import EmojiPicker from './EmojiPicker.svelte';

  interface Props {
    message: MessageRow;
    mine: boolean;
    selected: boolean;
    onreact: (emoji: string) => void;
    onreply: () => void;
    ondownload: (hash: string) => void;
    onthread: () => void;
  }

  const { message, mine, selected, onreact, onreply, ondownload, onthread }: Props = $props();

  /** Ссылки на превью: подтягиваются по мере появления файлов на диске. */
  let previews = $state<Record<string, string>>({});
  let reactionPicker = $state(false);

  const emojiMap = $derived(session.emojiMap);
  const sticker = $derived(loneSticker(message.body, emojiMap));

  /** Картинки своих эмодзи: те же превью, что и у вложений. */
  let glyphs = $state<Record<string, string>>({});

  $effect(() => {
    for (const emoji of session.emojis) {
      if (!glyphs[emoji.name]) {
        void previewUrl(emoji.hash).then((url) => {
          if (url) glyphs = { ...glyphs, [emoji.name]: url };
        });
      }
    }
  });

  $effect(() => {
    for (const file of message.attachments) {
      if (file.local && isViewable(file) && !previews[file.hash]) {
        void previewUrl(file.hash).then((url) => {
          if (url) previews = { ...previews, [file.hash]: url };
        });
      }
    }
  });
  const parts = $derived(chunks(message.body));

  /** Размер в человеческом виде: колонка узкая, байты в ней бесполезны. */
  function size(bytes: number): string {
    if (bytes < 1024) return `${bytes} Б`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} КБ`;
    return `${(bytes / 1024 / 1024).toFixed(1)} МБ`;
  }
</script>

<article class="msg" class:selected class:mine>
  <div class="head">
    <span class="who">{message.nick}</span>
    {#if mine}<span class="tag">вы</span>{/if}
    {#if message.edited}<span class="mark">изменено</span>{/if}
    <time>{timecode(message.ts)}</time>
  </div>

  {#if message.reply_preview}
    <div class="quote">
      <b>│</b>
      {message.reply_preview.nick}: {message.reply_preview.body.slice(0, 120)}
    </div>
  {/if}

  {#if message.deleted}
    <div class="deleted">· сообщение удалено ·</div>
  {:else if sticker && glyphs[sticker]}
    <img class="sticker" src={glyphs[sticker]} alt=":{sticker}:" />
  {:else}
    {#each parts as part (part.text)}
      {#if part.code}
        <pre class="selectable">{part.text}</pre>
      {:else}
        <div class="text selectable">
          {#each tokenize(part.text, emojiMap) as token (token.text + token.emoji)}
            {#if token.emoji && glyphs[token.emoji]}
              <img class="inline-emoji" src={glyphs[token.emoji]} alt={token.text} title={token.text} />
            {:else}{token.text}{/if}
          {/each}
        </div>
      {/if}
    {/each}
  {/if}

  {#each message.attachments as file (file.hash)}
    {#if previews[file.hash]}
      <figure class="shot">
        <img src={previews[file.hash]} alt={file.name} loading="lazy" />
        <figcaption>{file.name} · {size(file.size)}</figcaption>
      </figure>
    {:else}
    <button
      class="file"
      class:ready={file.local}
      onclick={(event) => {
        event.stopPropagation();
        if (!file.local) ondownload(file.hash);
      }}
      title={file.local ? 'файл уже на этом устройстве' : 'скачать у того, у кого он есть'}
    >
      <span class="glyph">{file.local ? '▣' : '▢'}</span>
      <span class="name">{file.name}</span>
      <span class="meta">{size(file.size)}{file.local ? '' : ' · скачать'}</span>
    </button>
    {/if}
  {/each}

  <div class="foot">
    {#each message.reactions as reaction (reaction.emoji)}
      <button
        class="rx"
        class:on={reaction.mine}
        onclick={(event) => {
          event.stopPropagation();
          onreact(reaction.emoji);
        }}
      >
        {reaction.emoji}
        {reaction.count}
      </button>
    {/each}

    {#if selected}
      <div class="picker-anchor">
        <button
          class="rx"
          onclick={(event) => {
            event.stopPropagation();
            reactionPicker = !reactionPicker;
          }}>＋ реакция</button
        >
        {#if reactionPicker}
          <EmojiPicker
            onpick={(emoji) => {
              reactionPicker = false;
              onreact(emoji);
            }}
            onclose={() => (reactionPicker = false)}
          />
        {/if}
      </div>
      <button
        class="rx"
        onclick={(event) => {
          event.stopPropagation();
          onreply();
        }}>ответить</button
      >
    {/if}

    {#if message.thread_replies > 0 || selected}
      <button
        class="thread"
        onclick={(event) => {
          event.stopPropagation();
          onthread();
        }}
      >
        └─ ветка{message.thread_replies > 0 ? ` · ${message.thread_replies}` : ' · начать'}
      </button>
    {/if}
  </div>
</article>

<style>
  .msg {
    padding: 5px 14px 6px;
    border-left: 2px solid transparent;
    outline: none;
  }
  .msg:hover {
    background: var(--bg-raised);
    border-left-color: var(--fg-faint);
  }
  /* Выделение — инверсией рамки, а не цветом: палитры тут нет */
  .msg.selected {
    background: var(--bg-raised);
    border-left-color: var(--fg);
  }

  .head {
    display: flex;
    align-items: baseline;
    gap: 9px;
    flex-wrap: wrap;
  }
  .who {
    color: var(--fg-hi);
    font-weight: 700;
  }
  .tag {
    background: var(--fg-dim);
    color: var(--bg);
    font-size: 10.5px;
    padding: 0 5px;
    letter-spacing: 0.06em;
  }
  .mark {
    color: var(--fg-faint);
    font-size: var(--text-xs);
  }
  time {
    margin-left: auto;
    color: var(--fg-faint);
    font-size: var(--text-xs);
  }

  .text {
    color: var(--fg);
    max-width: 82ch;
    padding-top: 2px;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .deleted {
    color: var(--fg-faint);
    padding-top: 2px;
  }

  /* Свой эмодзи занимает строку текста и не ломает межстрочный ритм */
  .inline-emoji {
    height: 1.25em;
    width: auto;
    vertical-align: -0.25em;
    object-fit: contain;
  }

  .sticker {
    display: block;
    margin: 6px 0 2px;
    max-width: 11rem;
    max-height: 11rem;
    object-fit: contain;
  }

  .quote {
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
    border-left: 1px solid var(--fg-faint);
    padding: 1px 0 1px 9px;
    margin: 3px 0 4px;
  }
  .quote b {
    color: var(--fg-dim);
    font-weight: 400;
  }

  pre {
    margin: 7px 0 3px;
    border: 1px solid var(--line);
    padding: 8px 12px;
    font-size: var(--text-sm);
    line-height: 1.6;
    overflow-x: auto;
    max-width: 76ch;
    background: var(--bg-raised);
    color: var(--fg);
  }

  .foot {
    display: flex;
    gap: 7px;
    margin-top: 6px;
    flex-wrap: wrap;
    align-items: center;
    min-height: 0;
  }
  .foot:empty {
    display: none;
  }

  .picker-anchor {
    position: relative;
  }

  .rx {
    border: 1px solid var(--fg-faint);
    color: var(--fg-dim);
    font-size: var(--text-xs);
    padding: 1px 8px;
    letter-spacing: 0.04em;
    transition: color var(--fast) var(--ease), border-color var(--fast) var(--ease);
  }
  .rx:hover {
    border-color: var(--fg-dim);
    color: var(--fg-hi);
  }
  .rx :global(.emoji) {
    filter: grayscale(1);
  }
  .rx.on {
    background: var(--inv-bg);
    border-color: var(--inv-bg);
    color: var(--inv-fg);
    font-weight: 700;
  }

  .thread {
    color: var(--fg-dim);
    font-size: var(--text-sm);
  }
  .thread:hover {
    color: var(--fg-hi);
    text-decoration: underline;
    text-underline-offset: 3px;
  }

  .shot {
    margin: 7px 0 3px;
    max-width: min(28rem, 100%);
    border: 1px solid var(--line);
    background: var(--bg-raised);
  }
  .shot img {
    display: block;
    width: 100%;
    height: auto;
    max-height: 22rem;
    object-fit: contain;
    /* Картинки тоже монохромные — иначе они выбиваются из всей системы */
    filter: grayscale(1) contrast(1.03);
  }
  .shot img:hover {
    filter: none;
  }
  .shot figcaption {
    padding: 3px 8px;
    border-top: 1px solid var(--line);
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
  }

  .file {
    display: flex;
    align-items: baseline;
    gap: var(--gap-3);
    margin-top: 7px;
    padding: 4px 10px;
    width: min(34rem, 100%);
    border: 1px solid var(--line);
    background: var(--bg-raised);
    color: var(--fg-dim);
    font-size: var(--text-sm);
    text-align: left;
    transition: border-color var(--fast) var(--ease), color var(--fast) var(--ease);
  }
  .file:hover {
    border-color: var(--fg-dim);
    color: var(--fg-hi);
  }
  /* Уже скачанный файл помечен заливкой глифа, а не цветом */
  .file.ready .glyph {
    color: var(--fg-hi);
  }
  .file .name {
    flex: 1;
    color: var(--fg);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .file .meta {
    color: var(--fg-faint);
    font-size: var(--text-xs);
    white-space: nowrap;
  }
</style>
