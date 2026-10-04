<script lang="ts">
  // Правая колонка: кто здесь, кто где и как написать любому из них.
  import Avatar from './Avatar.svelte';
  import { errorText, type MemberRow } from '../ipc';
  import { call } from '../stores/call.svelte';
  import { governance, ROLE_LABEL } from '../stores/governance.svelte';
  import { session } from '../stores/session.svelte';

  interface Props {
    oninvite: () => void;
  }
  const { oninvite }: Props = $props();

  const online = $derived(session.members.filter((m) => m.online));
  const offline = $derived(session.members.filter((m) => !m.online));

  /** Чем человек занят прямо сейчас — из присутствия, а не из догадок. */
  function status(id: string): string {
    if (id === session.me) return 'это вы';
    const room = session.channels.find(
      (channel) => channel.voice && session.voiceIds(channel.id).includes(id),
    );
    if (room) return `в звонке · ${room.name}`;
    return 'в сети';
  }

  /** Чей ряд раскрыт под действия и ждёт ли он подтверждения исключения. */
  let opened = $state<string | null>(null);
  let confirming = $state(false);
  let failure = $state('');

  function manageable(member: MemberRow): boolean {
    return (
      member.id !== session.me &&
      (governance.canRemove(member) || (governance.isOwner && member.role !== 'owner'))
    );
  }

  function toggle(member: MemberRow): void {
    opened = opened === member.id ? null : member.id;
    confirming = false;
    failure = '';
  }

  async function act(action: () => Promise<void>): Promise<void> {
    failure = '';
    try {
      await action();
      opened = null;
      confirming = false;
    } catch (issue) {
      failure = errorText(issue);
    }
  }

  function ago(at: number): string {
    if (!at) return 'давно';
    const minutes = Math.max(1, Math.round((Date.now() - at) / 60000));
    if (minutes < 60) return `был ${minutes} мин назад`;
    const hours = Math.round(minutes / 60);
    if (hours < 24) return `был ${hours} ч назад`;
    return `был ${Math.round(hours / 24)} дн назад`;
  }
</script>

{#snippet manage(member: MemberRow)}
  <div class="manage">
    {#if confirming}
      <p>
        Исключить {member.nick}? Ключ пространства сменится, и новое ему(ей) будет не прочитать.
        Вернуться по старым приглашениям не получится.
      </p>
      <div class="acts">
        <button class="btn small quiet" onclick={() => (confirming = false)}>Отмена</button>
        <button class="btn small danger" onclick={() => act(() => governance.remove(member))}>
          Исключить
        </button>
      </div>
    {:else}
      <div class="acts">
        {#if governance.isOwner && member.role !== 'owner'}
          <button
            class="btn small"
            onclick={() => act(() => governance.setAdmin(member, member.role !== 'admin'))}
          >
            {member.role === 'admin' ? 'Снять админа' : 'Сделать админом'}
          </button>
        {/if}
        {#if governance.canRemove(member)}
          <button class="btn small danger" onclick={() => (confirming = true)}>Исключить…</button>
        {/if}
      </div>
    {/if}
    {#if failure}<p class="fail">{failure}</p>{/if}
  </div>
{/snippet}

<aside class="people">
  <div class="head">
    <h3>Участники · {session.members.length}</h3>
    {#if governance.legacy}
      <p class="ownerless">
        {governance.info?.owner
          ? 'Пространство создано до 0.8: владелец здесь на слове, а не доказан, поэтому исключать и менять ключ нельзя. В новом пространстве — можно.'
          : 'Владелец не определён: пространство создано до 0.8, и запись о его создании сюда не дошла. Исключать здесь некому — в новом пространстве владельцем будете вы.'}
      </p>
    {/if}
  </div>

  <div class="list">
    <div class="group">В сети · {online.length}</div>
    {#each online as member (member.id)}
      <div class="row">
        <Avatar id={member.id} nick={member.nick} />
        <span class="nm">
          <b><span class="name">{member.nick}</span>{#if ROLE_LABEL[member.role]}<i class="role">{ROLE_LABEL[member.role]}</i>{/if}</b>
          <span>{status(member.id)}</span>
        </span>
        {#if member.id !== session.me}
          {#if call.active && call.space === session.spaceId && !call.participants.some((p) => p.id === member.id)}
            <!-- Вы в комнате, а он нет — позвать его туда звонком -->
            <button
              class="btn small write"
              onclick={async () => (session.status = await call.ring([member.id]))}
            >
              Позвать
            </button>
          {/if}
          <button class="btn small write" onclick={() => session.openDirect(member.id)}>
            Написать
          </button>
          {#if manageable(member)}
            <button class="btn small write more" aria-label="Права участника" onclick={() => toggle(member)}>⋯</button>
          {/if}
        {/if}
      </div>
      {#if opened === member.id}{@render manage(member)}{/if}
    {/each}

    {#if offline.length > 0}
      <div class="group">Не в сети · {offline.length}</div>
      {#each offline as member (member.id)}
        <div class="row away">
          <Avatar id={member.id} nick={member.nick} dim />
          <span class="nm">
            <b><span class="name">{member.nick}</span>{#if ROLE_LABEL[member.role]}<i class="role">{ROLE_LABEL[member.role]}</i>{/if}</b>
            <span>{ago(member.last_seen)}</span>
          </span>
          <button class="btn small write" onclick={() => session.openDirect(member.id)}>
            Написать
          </button>
          {#if manageable(member)}
            <button class="btn small write more" aria-label="Права участника" onclick={() => toggle(member)}>⋯</button>
          {/if}
        </div>
        {#if opened === member.id}{@render manage(member)}{/if}
      {/each}
    {/if}
  </div>

  {#if session.spaceId}
    <div class="invite">
      <b>{session.members.length === 1 ? 'Здесь пока только вы' : `Здесь ${session.members.length}`}</b>
      <p>Ссылка без регистрации и почты: со сроком, числом входов и кнопкой «погасить».</p>
      <button class="btn primary small" onclick={oninvite}>Пригласить</button>
    </div>
  {/if}
</aside>

<style>
  .people {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-left: 1px solid var(--line);
    background: rgba(0, 0, 0, 0.35);
  }
  .head {
    padding: 12px 12px 6px;
  }
  h3 {
    margin: 0;
    font-size: 11px;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--fg-dimmer);
    font-weight: 600;
  }
  .ownerless {
    margin: 8px 0 0;
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
    line-height: 1.45;
  }
  .group {
    padding: 10px 10px 4px;
    font-size: 11px;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: var(--fg-dimmer);
  }

  .list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0 8px 8px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px;
    border-radius: var(--radius-sm);
  }
  .row:hover {
    background: var(--lift);
  }
  .nm {
    flex: 1;
    min-width: 0;
  }
  .nm b {
    display: block;
    color: var(--fg);
    font-weight: 560;
    font-size: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .nm span {
    display: block;
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .away .nm b {
    color: var(--fg-dim);
  }

  /* Кнопка появляется по наведению: в покое список должен читаться, а не пестрить */
  .write {
    opacity: 0;
    transition: opacity var(--fast) var(--ease);
  }
  .row:hover .write,
  .write:focus-visible {
    opacity: 1;
  }

  .nm b {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .nm b .name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .role {
    flex: none;
    padding: 0 6px;
    border: 1px solid var(--edge, var(--line));
    border-radius: 999px;
    color: var(--fg-dim);
    font-size: 10px;
    font-style: normal;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    vertical-align: 1px;
  }

  .more {
    padding-inline: 8px;
    letter-spacing: 0.1em;
  }

  .manage {
    margin: 0 8px 6px 46px;
    padding: 8px 10px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--lift);
  }
  .manage p {
    margin: 0 0 8px;
    color: var(--fg-dim);
    font-size: var(--text-sm);
    line-height: 1.45;
  }
  .manage .acts {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 6px;
  }
  .manage .fail {
    margin: 8px 0 0;
    color: var(--fg-hi);
    font-weight: 600;
  }
  .danger {
    border-color: var(--fg-dim);
    color: var(--fg-hi);
  }
  .danger:hover {
    background: var(--inv-bg);
    color: var(--inv-fg);
  }

  .invite {
    margin: 8px;
    padding: 12px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: linear-gradient(160deg, var(--lift), transparent 70%);
  }
  .invite b {
    display: block;
    color: var(--fg-hi);
    font-size: var(--text);
    font-weight: 620;
  }
  .invite p {
    margin: 4px 0 10px;
    color: var(--fg-dim);
    font-size: var(--text-sm);
  }
</style>
