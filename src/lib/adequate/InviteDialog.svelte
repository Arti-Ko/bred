<script lang="ts">
  // Приглашение нового образца: срок, число входов, отзыв.
  //
  // Ключа в ссылке больше нет — только секрет, который любой участник в сети
  // сверит с логом. Поэтому ссылка перестала быть «навсегда»: здесь видно,
  // какие действуют, и любую можно погасить.
  import Dialog from './Dialog.svelte';
  import { errorText } from '../ipc';
  import {
    INVITE_TTLS,
    INVITE_USES,
    expiresText,
    governance,
    usesText,
  } from '../stores/governance.svelte';
  import { session } from '../stores/session.svelte';

  interface Props {
    onclose: () => void;
  }
  const { onclose }: Props = $props();

  let ttl = $state(INVITE_TTLS[2].ms);
  let uses = $state(0);
  let link = $state('');
  let busy = $state(false);
  let error = $state('');
  let copied = $state(false);
  let confirmRotate = $state(false);

  const live = $derived(governance.invites.filter((invite) => invite.live));

  $effect(() => {
    void governance.load(session.spaceId);
  });

  async function create(): Promise<void> {
    busy = true;
    error = '';
    try {
      link = await governance.createInvite({ expiresIn: ttl, uses });
      await copy();
    } catch (issue) {
      error = errorText(issue);
    } finally {
      busy = false;
    }
  }

  async function copy(): Promise<void> {
    try {
      await navigator.clipboard.writeText(link);
      copied = true;
      setTimeout(() => (copied = false), 1800);
    } catch (issue) {
      error = errorText(issue);
    }
  }

  async function act(action: () => Promise<void>): Promise<void> {
    error = '';
    try {
      await action();
    } catch (issue) {
      error = errorText(issue);
    }
  }
</script>

<Dialog title={`Пригласить в «${session.space?.name ?? ''}»`} {onclose}>
  <p class="hint-text">
    В ссылке нет ключа пространства — только пропуск. Гостя впустит любой участник, который
    будет в сети, а погасить ссылку можно в любой момент.
  </p>

  <div>
    <span class="field-label">Сколько действует</span>
    <div class="segmented">
      {#each INVITE_TTLS as option (option.ms)}
        <button class:on={ttl === option.ms} onclick={() => (ttl = option.ms)}>{option.label}</button>
      {/each}
    </div>
  </div>
  <div>
    <span class="field-label">Сколько раз можно войти</span>
    <div class="segmented">
      {#each INVITE_USES as option (option.uses)}
        <button class:on={uses === option.uses} onclick={() => (uses = option.uses)}>
          {option.label}
        </button>
      {/each}
    </div>
  </div>

  {#if link}
    <div>
      <span class="field-label">Ссылка — уже в буфере обмена</span>
      <div class="input mono selectable">{link}</div>
    </div>
  {/if}

  {#if live.length > 0}
    <div>
      <span class="field-label">Действующие приглашения · {live.length}</span>
      <ul class="invites">
        {#each live as invite (invite.id)}
          <li>
            <span class="who">{invite.mine ? 'ваше' : invite.author_nick}</span>
            <span class="meta">{expiresText(invite.expires)} · {usesText(invite)}</span>
            <button class="btn small quiet" onclick={() => act(() => governance.revoke(invite.id))}>
              Погасить
            </button>
          </li>
        {/each}
      </ul>
    </div>
  {/if}

  {#if governance.canRekey}
    <div class="rotate">
      {#if confirmRotate}
        <p class="hint-text">
          Всем участникам разложим новый ключ, каждому — свой экземпляр. Ссылки старого образца,
          в которых лежал прежний ключ, перестанут работать. Те, кто сейчас не в сети, получат
          ключ, когда зайдут.
        </p>
        <div class="row">
          <button class="btn small quiet" onclick={() => (confirmRotate = false)}>Отмена</button>
          <button
            class="btn small"
            onclick={() =>
              act(async () => {
                await governance.rotate();
                confirmRotate = false;
              })}
          >
            Сменить ключ
          </button>
        </div>
      {:else}
        <button class="link-btn" onclick={() => (confirmRotate = true)}>
          Сменить ключ пространства…
        </button>
        <span class="epoch">ключ № {(governance.info?.epoch ?? 0) + 1}</span>
      {/if}
    </div>
  {/if}

  {#if error}<p class="error">{error}</p>{/if}

  {#snippet footer()}
    <button class="btn quiet" onclick={onclose}>Закрыть</button>
    {#if link}
      <button class="btn" onclick={copy}>{copied ? 'Скопировано' : 'Скопировать ещё раз'}</button>
    {/if}
    <button class="btn primary" disabled={busy} onclick={create}>
      {busy ? 'Готовим…' : link ? 'Новая ссылка' : 'Создать ссылку'}
    </button>
  {/snippet}
</Dialog>

<style>
  .segmented {
    display: flex;
    gap: 4px;
    padding: 4px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--bg);
  }
  .segmented button {
    flex: 1;
    padding: 6px 8px;
    border-radius: 6px;
    color: var(--fg-dim);
    font-size: var(--text-sm);
    white-space: nowrap;
  }
  .segmented button:hover {
    color: var(--fg-hi);
  }
  .segmented button.on {
    background: var(--inv-bg);
    color: var(--inv-fg);
    font-weight: 620;
  }

  .invites {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin: 0;
    padding: 0;
    list-style: none;
    max-height: 10rem;
    overflow-y: auto;
  }
  .invites li {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 10px;
    padding: 5px 8px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
  }
  .who {
    overflow: hidden;
    color: var(--fg);
    font-weight: 560;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
  }

  .rotate {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding-top: 10px;
    border-top: 1px solid var(--line);
  }
  .rotate .row {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .link-btn {
    align-self: flex-start;
    color: var(--fg-dim);
    font-size: var(--text-sm);
    text-decoration: underline;
    text-underline-offset: 3px;
  }
  .link-btn:hover {
    color: var(--fg-hi);
  }
  .epoch {
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
  }

  .error {
    margin: 0;
    color: var(--fg-hi);
    font-size: var(--text-sm);
    padding: 7px 10px;
    border: 1px solid var(--edge);
    border-radius: var(--radius-sm);
  }
</style>
