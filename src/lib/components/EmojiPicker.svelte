<script lang="ts">
  import { EMOJI } from '../emoji';
  import { previewUrl } from '../previews';
  import { session } from '../stores/session.svelte';

  interface Props {
    onpick: (emoji: string) => void;
    onclose: () => void;
  }

  const { onpick, onclose }: Props = $props();
  let active = $state(0);

  /** Картинки своих эмодзи. */
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

  const custom = $derived(session.emojis.filter((e) => !e.sticker));
  const stickers = $derived(session.emojis.filter((e) => e.sticker));
  /** Свои наборы идут после стандартных, поэтому нумеруются со сдвигом. */
  const CUSTOM_TAB = EMOJI.length;
  const STICKER_TAB = EMOJI.length + 1;
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
    {#if custom.length > 0}
      <button
        aria-current={active === CUSTOM_TAB ? 'true' : undefined}
        onclick={() => (active = CUSTOM_TAB)}>свои</button
      >
    {/if}
    {#if stickers.length > 0}
      <button
        aria-current={active === STICKER_TAB ? 'true' : undefined}
        onclick={() => (active = STICKER_TAB)}>стикеры</button
      >
    {/if}
  </nav>

  <div class="grid" class:big={active >= CUSTOM_TAB}>
    {#if active < CUSTOM_TAB}
      {#each EMOJI[active].items as emoji (emoji)}
        <button class="cell" onclick={() => onpick(emoji)} title={emoji}>{emoji}</button>
      {/each}
    {:else}
      {#each active === CUSTOM_TAB ? custom : stickers as emoji (emoji.name)}
        <button class="cell own" onclick={() => onpick(`:${emoji.name}:`)} title=":{emoji.name}:">
          {#if glyphs[emoji.name]}
            <img src={glyphs[emoji.name]} alt=":{emoji.name}:" />
          {:else}
            <span class="pending">:{emoji.name}:</span>
          {/if}
        </button>
      {/each}
    {/if}
  </div>

  {#if active >= CUSTOM_TAB && (active === CUSTOM_TAB ? custom : stickers).length === 0}
    <p class="hint">пусто. добавить: <b>/эмодзи имя</b> или <b>/стикер имя</b></p>
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

  .hint {
    margin: 0;
    padding: var(--gap-3) var(--gap-4);
    border-top: 1px solid var(--line);
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
  }
  .hint b {
    color: var(--fg);
    font-weight: 400;
  }
</style>
