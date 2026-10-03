<script lang="ts">
  import { EMOJI } from '../emoji';
  import { session } from '../stores/session.svelte';

  interface Props {
    onpick: (emoji: string) => void;
    onclose: () => void;
    /**
     * Стикер отправляется сразу, отдельным сообщением. Не задано — значит, стикер
     * здесь не к месту (например, в выборе реакции), и вкладки стикеров нет.
     */
    onsticker?: (name: string) => void;
  }

  const { onpick, onclose, onsticker }: Props = $props();
  let active = $state(0);
  /** Открыта ли форма добавления своего эмодзи или стикера. */
  let adding = $state(false);
  let name = $state('');

  const custom = $derived(session.emojis.filter((e) => !e.sticker));
  const stickers = $derived(session.emojis.filter((e) => e.sticker));
  /** Свои наборы идут после стандартных, поэтому нумеруются со сдвигом. */
  const CUSTOM_TAB = EMOJI.length;
  const STICKER_TAB = EMOJI.length + 1;
  const own = $derived(active === STICKER_TAB ? stickers : custom);

  function pickOwn(emojiName: string): void {
    if (active === STICKER_TAB && onsticker) onsticker(emojiName);
    else onpick(`:${emojiName}:`);
  }

  async function add(): Promise<void> {
    const sticker = active === STICKER_TAB;
    await session.addEmoji(name, sticker);
    name = '';
    adding = false;
  }
</script>

<svelte:window onkeydown={(event) => event.key === 'Escape' && onclose()} />

<!-- Подложка ловит клик мимо: пикер должен закрываться, как любое меню -->
<div class="scrim" role="presentation" onclick={onclose}></div>

<div class="picker" role="dialog" aria-label="Выбор эмодзи">
  <nav class="tabs">
    {#each EMOJI as group, index (group.name)}
      <button aria-current={index === active ? 'true' : undefined} onclick={() => (active = index)}>
        {group.name}
      </button>
    {/each}
    <!-- Свои вкладки видны всегда: раньше пустые прятались, и узнать, что
         стикеры вообще есть и как их добавить, было неоткуда. -->
    <button
      aria-current={active === CUSTOM_TAB ? 'true' : undefined}
      onclick={() => (active = CUSTOM_TAB)}>свои</button
    >
    {#if onsticker}
      <button
        aria-current={active === STICKER_TAB ? 'true' : undefined}
        onclick={() => (active = STICKER_TAB)}>стикеры</button
      >
    {/if}
  </nav>

  <div class="grid" class:big={active >= CUSTOM_TAB} class:stickers={active === STICKER_TAB}>
    {#if active < CUSTOM_TAB}
      {#each EMOJI[active].items as emoji (emoji)}
        <button class="cell" onclick={() => onpick(emoji)} title={emoji}>{emoji}</button>
      {/each}
    {:else}
      {#each own as emoji (emoji.name)}
        <button
          class="cell own"
          onclick={() => pickOwn(emoji.name)}
          title={active === STICKER_TAB ? `отправить :${emoji.name}:` : `:${emoji.name}:`}
        >
          {#if session.glyphs[emoji.name]}
            <img src={session.glyphs[emoji.name]} alt=":{emoji.name}:" />
          {:else}
            <span class="pending">:{emoji.name}:</span>
          {/if}
        </button>
      {/each}
      <button class="cell add" onclick={() => (adding = !adding)} title="Добавить свою картинку">＋</button>
    {/if}
  </div>

  {#if active >= CUSTOM_TAB}
    {#if adding}
      <form
        class="adder"
        onsubmit={(event) => {
          event.preventDefault();
          void add();
        }}
      >
        <!-- svelte-ignore a11y_autofocus -->
        <input
          bind:value={name}
          placeholder={active === STICKER_TAB ? 'имя стикера, например котик' : 'имя, например паррот'}
          maxlength="32"
          autofocus
        />
        <button type="submit" disabled={!name.trim()}>выбрать картинку</button>
      </form>
    {:else if own.length === 0}
      <p class="hint">
        {active === STICKER_TAB
          ? 'стикеров пока нет. ＋ — выбрать картинку, она станет стикером для всего пространства'
          : 'своих эмодзи пока нет. ＋ — выбрать картинку и дать ей имя'}
      </p>
    {:else if active === STICKER_TAB}
      <p class="hint">щелчок — отправить стикер сразу</p>
    {/if}
  {/if}
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 40;
  }

  .picker {
    position: absolute;
    bottom: calc(100% + 6px);
    left: 0;
    z-index: 41;
    width: 22rem;
    max-width: calc(100vw - 2rem);
    background: var(--bg-raised);
    border: 1px solid var(--fg-faint);
    display: flex;
    flex-direction: column;
  }

  .tabs {
    display: flex;
    border-bottom: 1px solid var(--line);
    overflow-x: auto;
  }
  .tabs button {
    padding: 4px 10px;
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
    white-space: nowrap;
  }
  .tabs button:hover {
    color: var(--fg-hi);
  }
  .tabs button[aria-current='true'] {
    background: var(--inv-bg);
    color: var(--inv-fg);
    font-weight: 700;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(2rem, 1fr));
    gap: 2px;
    padding: var(--gap-3);
    max-height: 14rem;
    overflow-y: auto;
  }

  .cell {
    aspect-ratio: 1;
    display: grid;
    place-items: center;
    font-size: 17px;
    border: 1px solid transparent;
    /* Эмодзи — единственное цветное пятно в системе, поэтому по умолчанию
       они приглушены и оживают только под курсором. */
    filter: grayscale(1);
    opacity: 0.8;
    transition: filter var(--fast) var(--ease), opacity var(--fast) var(--ease);
  }
  .cell:hover {
    filter: none;
    opacity: 1;
    border-color: var(--fg-dim);
  }

  .grid.big {
    grid-template-columns: repeat(auto-fill, minmax(2.6rem, 1fr));
  }
  /* Стикер — картинка, а не значок: его надо разглядеть до отправки. */
  .grid.stickers {
    grid-template-columns: repeat(auto-fill, minmax(4.4rem, 1fr));
  }
  .cell.own {
    filter: grayscale(1);
  }
  .cell.own img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
  .cell .pending {
    font-size: 9px;
    color: var(--fg-faint);
    word-break: break-all;
  }
  .cell.add {
    border: 1px dashed var(--fg-faint);
    color: var(--fg-dimmer);
    filter: none;
  }
  .cell.add:hover {
    color: var(--fg-hi);
  }

  .adder {
    display: flex;
    gap: 6px;
    padding: var(--gap-3);
    border-top: 1px solid var(--line);
  }
  .adder input {
    flex: 1;
    min-width: 0;
    padding: 3px 6px;
    border: 1px solid var(--fg-faint);
    background: var(--bg);
    color: var(--fg);
    font-size: var(--text-xs);
  }
  .adder button {
    padding: 3px 8px;
    background: var(--inv-bg);
    color: var(--inv-fg);
    font-size: var(--text-xs);
    white-space: nowrap;
  }
  .adder button:disabled {
    opacity: 0.4;
  }

  .hint {
    margin: 0;
    padding: var(--gap-3) var(--gap-4);
    border-top: 1px solid var(--line);
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
  }
</style>
