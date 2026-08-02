<script lang="ts">
  import { call } from '../stores/call.svelte';
  import { session } from '../stores/session.svelte';
</script>

<aside class="pane left">
  <div class="pane-head">
    <span>каналы</span>
    <b>{String(session.channels.length).padStart(2, '0')}</b>
  </div>

  <div class="pane-body">
    {#if session.directs.length > 0}
      <div class="group">личные</div>
      {#each session.directs as space (space.id)}
        <button
          class="row"
          aria-current={space.id === session.spaceId ? 'true' : undefined}
          onclick={() => session.selectSpace(space.id)}
        >
          <span class="k">@</span>
          <span class="nm">{space.name}</span>
          {#if space.unread > 0}<span class="n">{space.unread}</span>{/if}
        </button>
      {/each}
    {/if}

    {#if session.rooms.length > 1}
      <div class="group">пространства</div>
      {#each session.rooms as space, index (space.id)}
        <button
          class="row"
          aria-current={space.id === session.spaceId ? 'true' : undefined}
          onclick={() => session.selectSpace(space.id)}
        >
          <span class="k">{index + 1}</span>
          <span class="nm">{space.name}</span>
          {#if space.unread > 0}<span class="n">{space.unread}</span>{/if}
        </button>
      {/each}
    {/if}

    {#each session.grouped as bucket (bucket.category)}
      <div class="group">{bucket.category}</div>
      {#each bucket.items as channel (channel.id)}
        <button
          class="row"
          aria-current={channel.id === session.channelId ? 'true' : undefined}
          onclick={() =>
            channel.voice ? session.joinVoice(channel.id) : session.selectChannel(channel.id)}
        >
          <span class="k">{channel.voice ? '>' : '#'}</span>
          <span class="nm">{channel.name}</span>
          {#if channel.voice && call.channel === channel.id}
            <span class="n">◉</span>
          {:else if channel.unread > 0}
            <span class="n">{channel.unread}</span>
          {/if}
        </button>
        {#if channel.voice}
          {#each session.voiceMembers(channel.id) as who (who)}
            <div class="in-voice">└ {who}</div>
          {/each}
        {/if}
      {/each}
    {/each}

    {#if session.spaces.length === 0}
      <p class="hint">
        нет пространств.<br />
        <code>/простор Орбита</code> — создать<br />
        <code>/войти ссылка</code> — присоединиться
      </p>
    {/if}

    <div class="rule"></div>
    <div class="row me">
      <span class="k">@</span>
      <span class="nm">{session.nick}</span>
      <span class="n">{session.online ? 'on' : '···'}</span>
    </div>
  </div>
</aside>

<style>
  .left {
    border-right: 1px solid var(--line);
  }

  .row {
    display: flex;
    align-items: center;
    gap: var(--gap-3);
    padding: 2px var(--gap-4);
    width: 100%;
    text-align: left;
    color: var(--fg-dim);
    white-space: nowrap;
    transition: background var(--fast) var(--ease), color var(--fast) var(--ease);
  }
  .row:hover {
    color: var(--fg);
    background: var(--bg-raised);
  }
  /* Активный элемент — полная инверсия. Единственный «акцент» системы */
  .row[aria-current='true'] {
    background: var(--inv-bg);
    color: var(--inv-fg);
  }
  .row[aria-current='true'] .k,
  .row[aria-current='true'] .n {
    color: var(--inv-fg);
    font-weight: 700;
  }

  .k {
    color: var(--fg-dimmer);
    width: 1.2rem;
  }
  .nm {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .n {
    color: var(--fg-hi);
    font-size: var(--text-xs);
  }

  .me {
    cursor: default;
  }
  .me:hover {
    background: none;
    color: var(--fg-dim);
  }

  .in-voice {
    padding: 1px var(--gap-4) 1px 30px;
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
  }

  .hint {
    color: var(--fg-dimmer);
    padding: var(--gap-3) var(--gap-4);
    line-height: 1.9;
    font-size: var(--text-sm);
  }
  .hint code {
    color: var(--fg);
    border: 1px solid var(--line);
    padding: 0 4px;
  }
</style>
