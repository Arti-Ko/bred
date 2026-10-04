<script lang="ts">
  import { session } from '../stores/session.svelte';
  import { governance } from '../stores/governance.svelte';
  import { ago } from '../format';
  import { previewUrl } from '../previews';

  /** Ссылки на аватары, подтягиваются по мере скачивания файлов. */
  let faces = $state<Record<string, string>>({});

  $effect(() => {
    for (const member of session.members) {
      if (member.avatar && !faces[member.avatar]) {
        const hash = member.avatar;
        void previewUrl(hash, session.spaceId).then((url) => {
          if (url) faces = { ...faces, [hash]: url };
        });
      }
    }
  });

  /** Роль — коротко, как всё в этом оформлении. */
  const TAG: Record<string, string> = { owner: 'влад', admin: 'адм', member: '' };

  const online = $derived(session.members.filter((m) => m.online));
  const offline = $derived(session.members.filter((m) => !m.online));
</script>

<aside class="pane right">
  <div class="pane-head">
    <span>участники</span>
    <b>{session.members.length}</b>
  </div>

  <div class="pane-body">
    <div class="group">online {online.length}</div>
    {#each online as member (member.id)}
      <button
        class="m"
        title={member.id === session.me ? 'это вы' : 'написать лично'}
        disabled={member.id === session.me || !member.dh}
        onclick={() => session.openDirect(member.id)}
      >
        {#if member.avatar && faces[member.avatar]}
          <img class="face" src={faces[member.avatar]} alt="" />
        {:else}
          <span class="i">●</span>
        {/if}
        <span class="nm">{member.nick}{#if TAG[member.role]}<i class="tag">{TAG[member.role]}</i>{/if}</span>
        <span class="st">{member.id === session.me ? 'вы' : 'on'}</span>
      </button>
    {/each}

    {#if offline.length > 0}
      <div class="group">offline {offline.length}</div>
      {#each offline as member (member.id)}
        <button
          class="m off"
          title="написать лично"
          disabled={!member.dh}
          onclick={() => session.openDirect(member.id)}
        >
          {#if member.avatar && faces[member.avatar]}
            <img class="face" src={faces[member.avatar]} alt="" />
          {:else}
            <span class="i">○</span>
          {/if}
          <span class="nm">{member.nick}{#if TAG[member.role]}<i class="tag">{TAG[member.role]}</i>{/if}</span>
          <span class="st">{ago(member.last_seen)}</span>
        </button>
      {/each}
    {/if}

    {#if governance.canRekey}
      <p class="powers">
        вы {governance.isOwner ? 'владелец' : 'администратор'}: /исключить ник ·
        {governance.isOwner ? '/админ ник · /неадмин ник · ' : ''}/ключ · /приглашения
      </p>
    {:else if governance.legacy}
      <p class="powers">
        пространство из версии до 0.8: владелец {governance.info?.owner ? 'на слове' : 'не определён'},
        исключать и менять ключ нельзя — только в новом пространстве
      </p>
    {/if}

    <div class="rule"></div>

    <div class="gauge">
      <div class="l"><span>узел</span><span>{session.online ? 'в сети' : 'поднимается'}</span></div>
      <div class="b selectable">{session.endpoint.slice(0, 16) || '—'}</div>
    </div>
    <div class="gauge">
      <div class="l"><span>непрочитано</span><span>{session.unreadTotal}</span></div>
    </div>
  </div>
</aside>

<style>
  .right {
    border-left: 1px solid var(--line);
  }

  .m {
    width: 100%;
    text-align: left;
    display: grid;
    grid-template-columns: 1.1rem minmax(0, 1fr) auto;
    gap: var(--gap-3);
    align-items: baseline;
    padding: 2px var(--gap-4);
    color: var(--fg-dim);
    white-space: nowrap;
  }
  .m:hover:not(:disabled) {
    background: var(--bg-raised);
    color: var(--fg);
  }
  .m:disabled {
    cursor: default;
  }
  .i {
    color: var(--fg-hi);
  }
  /* Аватар занимает то же место, что и точка статуса: колонки не должны ехать */
  .face {
    width: 1.1rem;
    height: 1.1rem;
    object-fit: cover;
    align-self: center;
    filter: grayscale(1) contrast(1.05);
  }
  .nm {
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--fg);
  }
  .st {
    color: var(--fg-faint);
    font-size: var(--text-xs);
  }

  .off {
    color: var(--fg-faint);
  }
  .off .i,
  .off .nm {
    color: var(--fg-dimmer);
  }

  .gauge {
    padding: 6px var(--gap-4);
  }
  .gauge .l {
    display: flex;
    justify-content: space-between;
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
  }
  .gauge .b {
    color: var(--fg);
    font-size: var(--text-sm);
    margin-top: 2px;
    word-break: break-all;
  }
  .tag {
    margin-left: 6px;
    color: var(--fg-dim);
    font-size: var(--text-xs);
    font-style: normal;
    letter-spacing: 0.08em;
  }
  .powers {
    margin: 8px 6px 0;
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
    line-height: 1.5;
  }
</style>
