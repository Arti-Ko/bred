<script lang="ts">
  // Левая колонка: каналы, голосовые комнаты, личные переписки и своя карточка.
  //
  // Всё, что в терминале делалось командой, здесь — кнопка рядом с тем местом,
  // о котором человек в этот момент думает: «＋» у списка каналов создаёт канал,
  // «＋» у личных открывает выбор собеседника, карточка внизу ведёт в профиль.

  import Avatar from './Avatar.svelte';
  import Icon from './Icon.svelte';
  import { call } from '../stores/call.svelte';
  import { session } from '../stores/session.svelte';

  interface Props {
    onnewchannel: () => void;
    onprofile: () => void;
    onnewdirect: () => void;
  }
  const { onnewchannel, onprofile, onnewdirect }: Props = $props();

  const text = $derived(session.channels.filter((c) => !c.voice));
  const voice = $derived(session.channels.filter((c) => c.voice));
</script>

<aside class="side">
  <div class="head">
    <h3>Каналы</h3>
    <button class="icon-btn" onclick={onnewchannel} title="Создать канал" aria-label="Создать канал">
      <Icon name="plus" />
    </button>
  </div>

  <div class="list">
    {#each text as channel (channel.id)}
      <button
        class="row"
        class:on={channel.id === session.channelId}
        class:unread={channel.unread > 0}
        onclick={() => session.selectChannel(channel.id)}
      >
        <span class="ico"><Icon name="hash" size={13} /></span>
        <span class="nm">{channel.name}</span>
        {#if channel.unread > 0}<span class="badge">{channel.unread}</span>{/if}
      </button>
    {/each}

    {#each voice as room (room.id)}
      {@const people = session.voiceIds(room.id)}
      <div class="room" class:live={call.channel === room.id}>
        <button class="row" onclick={() => session.joinVoice(room.id)}>
          <span class="ico"><Icon name="speaker" size={14} /></span>
          <span class="nm">{room.name}</span>
          {#if people.length > 0}<span class="badge mute">{people.length}</span>{/if}
        </button>
        {#each people as who (who)}
          <div class="inside">
            <Avatar id={who} size="sm" />
            <span class="nm">{session.members.find((m) => m.id === who)?.nick ?? who.slice(0, 8)}</span>
          </div>
        {/each}
        {#if call.channel === room.id}
          <button class="btn small wide" onclick={() => call.leave()}>Выйти из разговора</button>
        {:else}
          <button class="btn small wide" onclick={() => session.joinVoice(room.id)}>
            Присоединиться{people.length > 0 ? ' к разговору' : ''}
          </button>
        {/if}
      </div>
    {/each}

    {#if session.directs.length > 0 || session.members.length > 1}
      <div class="head inner">
        <h3>Личные</h3>
        <button class="icon-btn" onclick={onnewdirect} title="Написать человеку" aria-label="Написать человеку">
          <Icon name="plus" />
        </button>
      </div>
      {#each session.directs as room (room.id)}
        <button
          class="row"
          class:on={room.id === session.spaceId}
          class:unread={room.unread > 0}
          onclick={() => session.selectSpace(room.id)}
        >
          <span class="ico"><Icon name="person" size={13} /></span>
          <span class="nm">{room.name}</span>
          {#if room.unread > 0}<span class="badge">{room.unread}</span>{/if}
        </button>
      {/each}
    {/if}
  </div>

  <button class="me" onclick={onprofile}>
    <Avatar id={session.me} nick={session.nick} />
    <span class="who">
      <b>{session.nick}</b>
      <span>{session.online ? 'в сети' : 'поднимается'}</span>
    </span>
    <span class="icon-btn" aria-hidden="true"><Icon name="pencil" /></span>
  </button>
</aside>

<style>
  .side {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-right: 1px solid var(--line);
    background: rgba(0, 0, 0, 0.35);
  }

  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 10px 6px;
  }
  .head.inner {
    padding-top: 16px;
  }
  h3 {
    margin: 0;
    flex: 1;
    font-size: 11px;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--fg-dimmer);
    font-weight: 600;
  }

  .list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0 8px 8px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 7px 10px;
    border-radius: var(--radius-sm);
    color: var(--fg-dim);
    text-align: left;
    transition: background var(--fast) var(--ease), color var(--fast) var(--ease);
  }
  .row:hover {
    background: var(--lift);
    color: var(--fg);
  }
  .row .ico {
    display: grid;
    place-items: center;
    color: var(--fg-dimmer);
  }
  .row .nm {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row.unread {
    color: var(--fg-hi);
    font-weight: 560;
  }
  /* Выбранное — инверсией полосой слева: цвета в системе нет */
  .row.on {
    background: var(--lift-2);
    color: var(--fg-hi);
    box-shadow: inset 2px 0 0 var(--fg-hi);
  }
  .row.on .ico {
    color: var(--fg-hi);
  }

  .badge {
    min-width: 20px;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--inv-bg);
    color: var(--inv-fg);
    font-size: 11px;
    font-weight: 700;
    text-align: center;
    line-height: 18px;
  }
  .badge.mute {
    background: var(--fg-faint);
    color: var(--fg-dim);
  }

  .room {
    margin-top: 2px;
    padding: 2px 4px 6px;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
  }
  .room.live {
    border-color: var(--line);
    background: var(--lift);
  }
  .inside {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 3px 10px;
    color: var(--fg-dim);
    font-size: var(--text-sm);
    min-width: 0;
  }
  .inside .nm {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .wide {
    width: calc(100% - 8px);
    margin: 4px;
    justify-content: center;
  }

  .me {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 8px;
    padding: 9px 10px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--bg-raised);
    text-align: left;
  }
  .me:hover {
    border-color: var(--fg-dim);
  }
  .me .who {
    flex: 1;
    min-width: 0;
  }
  .me .who b {
    display: block;
    color: var(--fg-hi);
    font-weight: 600;
    font-size: var(--text);
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .me .who span {
    color: var(--fg-dim);
    font-size: var(--text-sm);
  }
</style>
