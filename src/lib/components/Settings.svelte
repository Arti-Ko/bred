<script lang="ts">
  import { session } from '../stores/session.svelte';
  import { updates } from '../stores/updates.svelte';

  interface Props {
    onclose: () => void;
  }

  const { onclose }: Props = $props();

  $effect(() => {
    void updates.init();
  });

  const stageText: Record<string, string> = {
    idle: 'не проверялось',
    checking: 'проверяем…',
    current: 'установлена последняя версия',
    available: 'доступно обновление',
    downloading: 'качаем…',
    ready: 'обновление установлено',
    failed: 'не удалось проверить',
  };
</script>

<svelte:window onkeydown={(event) => event.key === 'Escape' && onclose()} />

<div class="scrim" role="presentation" onclick={onclose}></div>

<div class="panel" role="dialog" aria-label="Настройки" tabindex="-1">
  <header>
    <span>настройки</span>
    <button onclick={onclose}>закрыть ✕</button>
  </header>

  <div class="body">
    <div class="group">профиль</div>
    <div class="row">
      <span class="k">имя</span>
      <span class="v">{session.nick}</span>
      <span class="hint">меняется командой <b>/имя</b></span>
    </div>
    <div class="row">
      <span class="k">картинка</span>
      <button class="act" onclick={() => session.setAvatar()}>выбрать…</button>
      <span class="hint">png, gif, webp</span>
    </div>
    <div class="row">
      <span class="k">ключ</span>
      <span class="v selectable">{session.me}</span>
    </div>

    <div class="group">сеть</div>
    <div class="row">
      <span class="k">узел</span>
      <span class="v selectable">{session.endpoint || '—'}</span>
    </div>
    <div class="row">
      <span class="k">состояние</span>
      <span class="v">{session.online ? 'в сети' : 'поднимается'}</span>
    </div>

    <div class="group">обновление</div>
    <div class="row">
      <span class="k">версия</span>
      <span class="v">{updates.version || '…'}</span>
      <button class="act" onclick={() => updates.check()} disabled={updates.stage === 'checking'}>
        проверить
      </button>
    </div>

    <div class="row">
      <span class="k">статус</span>
      <span class="v">
        {stageText[updates.stage] ?? updates.stage}
        {#if updates.stage === 'downloading' && updates.progress > 0}
          · {updates.progress}%
        {/if}
      </span>
    </div>

    {#if updates.stage === 'available'}
      <div class="notice">
        <b>версия {updates.next}</b>
        {#if updates.notes}
          <pre class="notes selectable">{updates.notes}</pre>
        {/if}
        <button class="act primary" onclick={() => updates.install()}>установить</button>
      </div>
    {/if}

    {#if updates.stage === 'ready'}
      <div class="notice">
        <b>готово, нужен перезапуск</b>
        <!-- Перезапуск отдельной кнопкой: человек может быть в звонке -->
        <button class="act primary" onclick={() => updates.restart()}>перезапустить</button>
      </div>
    {/if}

    {#if updates.error}
      <div class="row"><span class="k">ошибка</span><span class="v">{updates.error}</span></div>
    {/if}

    <div class="group">пространство</div>
    <div class="row">
      <span class="k">приглашение</span>
      <button class="act" onclick={() => session.invite()} disabled={!session.spaceId}>
        скопировать ссылку
      </button>
    </div>
    <div class="row">
      <span class="k">эмодзи</span>
      <span class="hint">
        добавить: <b>/эмодзи имя</b> · стикер: <b>/стикер имя</b>
      </span>
    </div>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 80;
    background: rgba(0, 0, 0, 0.55);
  }

  .panel {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    z-index: 81;
    width: min(38rem, calc(100vw - 3rem));
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    background: var(--bg-raised);
    border: 1px solid var(--fg-faint);
  }

  header {
    display: flex;
    justify-content: space-between;
    padding: 6px var(--gap-4);
    border-bottom: 1px solid var(--line);
    color: var(--fg-dimmer);
    font-size: var(--text-label);
    letter-spacing: 0.14em;
    text-transform: uppercase;
  }
  header button {
    color: var(--fg-dim);
    font-size: var(--text-xs);
  }
  header button:hover {
    color: var(--fg-hi);
  }

  .body {
    overflow-y: auto;
    padding-bottom: var(--gap-4);
  }

  .row {
    display: grid;
    grid-template-columns: 8rem minmax(0, 1fr) auto;
    gap: var(--gap-4);
    align-items: baseline;
    padding: 4px var(--gap-4);
  }
  .k {
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
  }
  .v {
    color: var(--fg);
    font-size: var(--text-sm);
    word-break: break-all;
  }
  .hint {
    color: var(--fg-faint);
    font-size: var(--text-xs);
    grid-column: 2 / -1;
  }
  .hint b,
  .notice b {
    color: var(--fg);
    font-weight: 400;
  }

  .act {
    border: 1px solid var(--fg-faint);
    color: var(--fg-dim);
    padding: 1px 10px;
    font-size: var(--text-xs);
    transition: color var(--fast) var(--ease), border-color var(--fast) var(--ease);
  }
  .act:hover:not(:disabled) {
    color: var(--fg-hi);
    border-color: var(--fg-dim);
  }
  .act:disabled {
    color: var(--fg-faint);
    cursor: default;
  }
  /* Главное действие — инверсией, как активный элемент везде в системе */
  .act.primary {
    background: var(--inv-bg);
    border-color: var(--inv-bg);
    color: var(--inv-fg);
    font-weight: 700;
  }

  .notice {
    margin: var(--gap-2) var(--gap-4) var(--gap-3);
    padding: var(--gap-3);
    border: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: var(--gap-3);
    align-items: flex-start;
    font-size: var(--text-sm);
    color: var(--fg-dim);
  }

  .notes {
    margin: 0;
    max-height: 8rem;
    overflow-y: auto;
    font-size: var(--text-xs);
    color: var(--fg-dimmer);
    white-space: pre-wrap;
  }
</style>
