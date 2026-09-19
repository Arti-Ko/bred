<script lang="ts">
  // Общая рама диалога: затемнение, Esc, заголовок, кнопка закрытия.
  import type { Snippet } from 'svelte';
  import Icon from './Icon.svelte';

  interface Props {
    title: string;
    onclose: () => void;
    children: Snippet;
    footer?: Snippet;
  }
  const { title, onclose, children, footer }: Props = $props();
</script>

<svelte:window
  onkeydown={(event) => {
    if (event.key !== 'Escape') return;
    event.stopPropagation();
    onclose();
  }}
/>

<div class="scrim" role="presentation" onclick={onclose}></div>
<div class="dialog" role="dialog" aria-label={title} aria-modal="true">
  <header>
    <b>{title}</b>
    <button class="icon-btn" onclick={onclose} aria-label="Закрыть"><Icon name="close" /></button>
  </header>
  <div class="inner">
    {@render children()}
  </div>
  {#if footer}
    <footer>{@render footer()}</footer>
  {/if}
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 90;
    background: rgba(0, 0, 0, 0.62);
  }
  .dialog {
    position: fixed;
    z-index: 91;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: min(28rem, calc(100vw - 2.5rem));
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--line);
    border-radius: 16px;
    background: var(--bg-raised);
    box-shadow: 0 40px 80px -50px #000;
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 16px 10px;
  }
  header b {
    flex: 1;
    color: var(--fg-hi);
    font-size: 15px;
    font-weight: 640;
  }
  .inner {
    padding: 0 16px 16px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 12px 16px;
    border-top: 1px solid var(--line);
  }
</style>
