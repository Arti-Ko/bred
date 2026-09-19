<script lang="ts">
  // Правая колонка: кто здесь, кто где и как написать любому из них.
  import Avatar from './Avatar.svelte';
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

  function ago(at: number): string {
    if (!at) return 'давно';
    const minutes = Math.max(1, Math.round((Date.now() - at) / 60000));
    if (minutes < 60) return `был ${minutes} мин назад`;
    const hours = Math.round(minutes / 60);
    if (hours < 24) return `был ${hours} ч назад`;
    return `был ${Math.round(hours / 24)} дн назад`;
  }
</script>

<aside class="people">
  <div class="head"><h3>Участники · {session.members.length}</h3></div>

  <div class="list">
    <div class="group">В сети · {online.length}</div>
    {#each online as member (member.id)}
      <div class="row">
        <Avatar id={member.id} nick={member.nick} />
        <span class="nm">
          <b>{member.nick}</b>
          <span>{status(member.id)}</span>
        </span>
        {#if member.id !== session.me}
          <button class="btn small write" onclick={() => session.openDirect(member.id)}>
            Написать
          </button>
        {/if}
      </div>
    {/each}

    {#if offline.length > 0}
      <div class="group">Не в сети · {offline.length}</div>
      {#each offline as member (member.id)}
        <div class="row away">
          <Avatar id={member.id} nick={member.nick} dim />
          <span class="nm">
            <b>{member.nick}</b>
            <span>{ago(member.last_seen)}</span>
          </span>
          <button class="btn small write" onclick={() => session.openDirect(member.id)}>
            Написать
          </button>
        </div>
      {/each}
    {/if}
  </div>

  {#if session.spaceId}
    <div class="invite">
      <b>{session.members.length === 1 ? 'Здесь пока только вы' : `Здесь ${session.members.length}`}</b>
      <p>Позовите остальных — ссылка работает без регистрации и почты.</p>
      <button class="btn primary small" onclick={oninvite}>Скопировать приглашение</button>
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
