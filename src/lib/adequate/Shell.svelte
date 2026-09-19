<script lang="ts">
  // Каркас адекватного режима.
  //
  // Лента, звонок, ветка и настройки переиспользуются от терминала — они уже
  // умеют всё, что нужно, и переписывать их значило бы потерять превью, свои
  // эмодзи и вложения. Новое здесь то, ради чего режим и затевался: шапка,
  // колонки, поле ввода и диалоги вместо команд.

  import CallPanel from '../components/CallPanel.svelte';
  import Feed from '../components/Feed.svelte';
  import ThreadPanel from '../components/ThreadPanel.svelte';
  import Avatar from './Avatar.svelte';
  import Composer from './Composer.svelte';
  import Dialogs from './Dialogs.svelte';
  import type { Kind } from './kinds';
  import Icon from './Icon.svelte';
  import Members from './Members.svelte';
  import Sidebar from './Sidebar.svelte';
  import TopBar from './TopBar.svelte';
  import Welcome from './Welcome.svelte';
  import { session } from '../stores/session.svelte';

  interface Props {
    onsettings: () => void;
  }
  const { onsettings }: Props = $props();

  let dialog = $state<Kind | null>(null);
  let query = $state('');

  const online = $derived(session.members.filter((m) => m.online));
  const empty = $derived(session.spaces.length === 0);
</script>

<div class="shell">
  <TopBar
    {query}
    onquery={(value) => (query = value)}
    oninvite={() => (dialog = 'приглашение')}
    onnewspace={() => (dialog = 'пространство')}
    onjoin={() => (dialog = 'ссылка')}
    onleave={() => session.leaveSpace()}
    {onsettings}
    onprofile={() => (dialog = 'профиль')}
  />

  {#if empty}
    <Welcome onnewspace={() => (dialog = 'пространство')} onjoin={() => (dialog = 'ссылка')} />
  {:else}
    <div class="body">
      <Sidebar
        onnewchannel={() => (dialog = 'канал')}
        onprofile={() => (dialog = 'профиль')}
        onnewdirect={() => (dialog = 'личное')}
      />

      <main class="middle">
        <div class="feed-head">
          <div class="title">
            {#if session.channel}
              <Icon name={session.channel.voice ? 'speaker' : 'hash'} size={14} />
              <b>{session.channel.name}</b>
            {:else}
              <b class="dim">Канал не выбран</b>
            {/if}
            {#if session.space}<span class="where">{session.space.name}</span>{/if}
          </div>
          <span class="sp"></span>
          <div class="faces">
            {#each online.slice(0, 4) as member (member.id)}
              <Avatar id={member.id} nick={member.nick} size="sm" />
            {/each}
            <span class="count">{online.length} в сети</span>
          </div>
        </div>

        <CallPanel />
        <Feed head={false} filter={query} avatars />
        <ThreadPanel />
        <Composer />
      </main>

      <Members oninvite={() => (dialog = 'приглашение')} />
    </div>
  {/if}

  {#if dialog}
    <Dialogs kind={dialog} onclose={() => (dialog = null)} />
  {/if}

  {#if session.status}
    <!-- Ошибка не должна теряться внизу экрана: в терминале её показывала
         строка режима, здесь её место — над полем ввода, у глаз. -->
    <div class="toast" role="status">
      <span>{session.status}</span>
      <button class="icon-btn" onclick={() => (session.status = '')} aria-label="Скрыть">
        <Icon name="close" size={13} />
      </button>
    </div>
  {/if}
</div>

<style>
  .shell {
    height: 100vh;
    display: grid;
    grid-template-rows: auto minmax(0, 1fr);
  }

  .body {
    display: grid;
    grid-template-columns: 17rem minmax(0, 1fr) 18.5rem;
    min-height: 0;
  }

  .middle {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  /* Лента приезжает готовым компонентом и должна занять всё, что осталось
     между шапкой канала и полем ввода. */
  .middle :global(.pane) {
    flex: 1;
    min-height: 0;
  }

  .feed-head {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 11px 16px;
    border-bottom: 1px solid var(--line);
  }
  .title {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    color: var(--fg-dimmer);
  }
  .title b {
    color: var(--fg-hi);
    font-size: 15px;
    font-weight: 640;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .title b.dim {
    color: var(--fg-dimmer);
    font-weight: 400;
  }
  .where {
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
  }
  .sp {
    flex: 1;
  }

  .faces {
    display: flex;
    align-items: center;
  }
  .faces :global(.av) {
    margin-left: -7px;
    border: 2px solid var(--bg);
  }
  .faces :global(.av:first-child) {
    margin-left: 0;
  }
  .count {
    margin-left: 9px;
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
  }

  .toast {
    position: fixed;
    left: 50%;
    bottom: 5.5rem;
    z-index: 70;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 10px;
    max-width: min(34rem, calc(100vw - 3rem));
    padding: 8px 10px 8px 14px;
    border: 1px solid var(--edge);
    border-radius: 999px;
    background: var(--bg-raised);
    color: var(--fg-hi);
    font-size: var(--text-sm);
    box-shadow: 0 20px 44px -30px #000;
  }

  @media (max-width: 1180px) {
    .body {
      grid-template-columns: 17rem minmax(0, 1fr);
    }
    .body :global(.people) {
      display: none;
    }
  }
  @media (max-width: 860px) {
    .body {
      grid-template-columns: minmax(0, 1fr);
    }
    .body :global(.side) {
      display: none;
    }
  }
</style>
