<script lang="ts">
  // База закрыта код-паролем: до него ядро даже не поднимается.
  //
  // Один экран на оба оформления — как и форма отчёта: он нужен до того, как
  // что-либо ещё загрузилось, и выглядеть должен одинаково понятно.
  import { api, errorText } from '../ipc';
  import { prefs } from '../stores/prefs.svelte';

  interface Props {
    onunlock: () => void;
  }
  const { onunlock }: Props = $props();

  let code = $state('');
  let busy = $state(false);
  let error = $state('');
  let forgot = $state(false);
  let input: HTMLInputElement | undefined = $state();

  $effect(() => {
    input?.focus();
  });

  async function open(): Promise<void> {
    if (!code) return;
    busy = true;
    error = '';
    try {
      await api.unlock(code);
      onunlock();
    } catch (issue) {
      error = errorText(issue);
      code = '';
      input?.focus();
    } finally {
      busy = false;
    }
  }

  async function wipe(): Promise<void> {
    busy = true;
    error = '';
    try {
      await api.wipeLocked();
      onunlock();
    } catch (issue) {
      error = errorText(issue);
    } finally {
      busy = false;
    }
  }
</script>

<main class="lock" class:soft={prefs.adequate}>
  <div class="card">
    <div class="mark" aria-hidden="true">
      <svg viewBox="0 0 24 24" width="28" height="28" fill="none" stroke="currentColor" stroke-width="1.6">
        <rect x="5" y="10.5" width="14" height="9.5" rx="2" />
        <path d="M8 10.5V8a4 4 0 0 1 8 0v2.5" />
      </svg>
    </div>
    <h1>{prefs.adequate ? 'БРЕД заперт' : 'бред заперт'}</h1>

    {#if forgot}
      <p>
        Обойти код нельзя — иначе он ничего бы не защищал. Можно только стереть данные этого
        устройства и начать заново: вы станете новым участником, в пространства вернётесь по
        новым приглашениям, а переписку подтянете у собеседников. То, что было только здесь,
        пропадёт.
      </p>
      {#if error}<p class="error" role="alert">{error}</p>{/if}
      <div class="acts">
        <button class="ghost" disabled={busy} onclick={() => (forgot = false)}>Назад</button>
        <button class="danger" disabled={busy} onclick={wipe}>
          {busy ? 'Стираем…' : 'Стереть и начать заново'}
        </button>
      </div>
    {:else}
      <p>Переписка и ключи на этом устройстве закрыты код-паролем.</p>
      <form
        onsubmit={(event) => {
          event.preventDefault();
          void open();
        }}
      >
        <input
          bind:this={input}
          bind:value={code}
          type="password"
          placeholder="код-пароль"
          autocomplete="current-password"
          aria-label="Код-пароль"
          disabled={busy}
        />
        <button class="primary" type="submit" disabled={busy || !code}>
          {busy ? 'Открываем…' : 'Открыть'}
        </button>
      </form>
      {#if error}<p class="error" role="alert">{error}</p>{/if}
      <button class="forgot" onclick={() => (forgot = true)}>Забыли код?</button>
    {/if}
  </div>
</main>

<style>
  .lock {
    position: fixed;
    inset: 0;
    display: grid;
    place-items: center;
    padding: 1rem;
    background:
      radial-gradient(60rem 40rem at 50% 120%, var(--lift, rgba(255, 255, 255, 0.04)), transparent 70%),
      var(--bg);
  }

  .card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
    width: min(24rem, 100%);
    padding: 28px 24px 22px;
    border: 1px solid var(--fg-faint);
    background: var(--bg-raised);
    box-shadow: 0 40px 80px -40px rgba(0, 0, 0, 0.95);
    text-align: center;
  }
  .soft .card {
    border-color: var(--edge, var(--fg-faint));
    border-radius: 18px;
  }

  .mark {
    display: grid;
    place-items: center;
    width: 52px;
    height: 52px;
    border: 1px solid var(--fg-faint);
    color: var(--fg-hi);
  }
  .soft .mark {
    border-radius: 50%;
  }

  h1 {
    margin: 0;
    color: var(--fg-hi);
    font-size: 20px;
    font-weight: 700;
    letter-spacing: 0.01em;
  }
  p {
    margin: 0;
    color: var(--fg-dim);
    font-size: var(--text-sm);
    line-height: 1.55;
  }

  form {
    display: flex;
    gap: 8px;
    width: 100%;
  }
  input {
    flex: 1;
    min-width: 0;
    padding: 8px 10px;
    border: 1px solid var(--fg-faint);
    background: var(--bg);
    color: var(--fg);
    font: inherit;
    letter-spacing: 0.08em;
  }
  .soft input {
    border-radius: 10px;
  }
  input:focus {
    outline: none;
    border-color: var(--fg-dim);
  }

  button {
    padding: 8px 14px;
    border: 1px solid var(--fg-faint);
    color: var(--fg-dim);
    font-size: var(--text-sm);
  }
  .soft button {
    border-radius: 10px;
  }
  button:hover:not(:disabled) {
    color: var(--fg-hi);
    border-color: var(--fg-dim);
  }
  button:disabled {
    opacity: 0.45;
  }
  .primary,
  .danger:hover:not(:disabled) {
    background: var(--inv-bg);
    border-color: var(--inv-bg);
    color: var(--inv-fg);
    font-weight: 700;
  }
  .primary:hover:not(:disabled) {
    color: var(--inv-fg);
  }
  .danger {
    color: var(--fg-hi);
    border-color: var(--fg-dim);
  }

  .acts {
    display: flex;
    justify-content: center;
    gap: 8px;
  }

  .forgot {
    padding: 2px 4px;
    border: 0;
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
    text-decoration: underline;
    text-underline-offset: 3px;
  }

  .error {
    color: var(--fg-hi);
    font-weight: 700;
  }
</style>
