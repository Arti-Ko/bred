<script lang="ts">
  // Шапка: где я, что ищу и что могу сделать с пространством.
  import Avatar from './Avatar.svelte';
  import Icon from './Icon.svelte';
  import { session } from '../stores/session.svelte';

  interface Props {
    query: string;
    onquery: (value: string) => void;
    oninvite: () => void;
    onnewspace: () => void;
    onjoin: () => void;
    onleave: () => void;
    onsettings: () => void;
    onprofile: () => void;
  }
  const {
    query,
    onquery,
    oninvite,
    onnewspace,
    onjoin,
    onleave,
    onsettings,
    onprofile,
  }: Props = $props();

  let menu = $state(false);
</script>

<header class="top">
  <div class="switcher">
    <button class="pick" onclick={() => (menu = !menu)} aria-expanded={menu}>
      {#if session.space}
        <Avatar id={session.space.id} nick={session.space.name} size="sm" />
        <span class="nm">{session.space.name}</span>
      {:else}
        <span class="nm dim">пространство не выбрано</span>
      {/if}
      <Icon name="caret" size={13} />
    </button>

    {#if menu}
      <!-- Клик мимо закрывает меню: в терминале это делал Esc, здесь ждут мышь -->
      <div class="veil" role="presentation" onclick={() => (menu = false)}></div>
      <div class="menu" role="menu">
        {#each session.rooms as space (space.id)}
          <button
            role="menuitem"
            class:on={space.id === session.spaceId}
            onclick={() => {
              menu = false;
              void session.selectSpace(space.id);
            }}
          >
            <Avatar id={space.id} nick={space.name} size="sm" />
            <span class="nm">{space.name}</span>
            {#if space.unread > 0}<span class="n">{space.unread}</span>{/if}
          </button>
        {/each}
        <div class="rule"></div>
        <button role="menuitem" onclick={() => { menu = false; onnewspace(); }}>
          <Icon name="home" size={14} /> Создать пространство
        </button>
        <button role="menuitem" onclick={() => { menu = false; onjoin(); }}>
          <Icon name="link" size={14} /> Войти по ссылке
        </button>
        {#if session.spaceId}
          <button role="menuitem" class="danger" onclick={() => { menu = false; onleave(); }}>
            <Icon name="door" size={14} /> Покинуть пространство
          </button>
        {/if}
      </div>
    {/if}
  </div>

  <label class="search">
    <Icon name="search" size={14} />
    <input
      value={query}
      oninput={(event) => onquery(event.currentTarget.value)}
      placeholder="Поиск в этом канале"
      aria-label="Поиск в этом канале"
    />
    {#if query}
      <button class="clear" onclick={() => onquery('')} aria-label="Очистить">
        <Icon name="close" size={12} />
      </button>
    {/if}
  </label>

  <span class="sp"></span>

  <button class="btn primary" onclick={oninvite} disabled={!session.spaceId}>
    <Icon name="plus" size={14} /> Пригласить
  </button>
  <button class="icon-btn bordered" onclick={onsettings} title="Настройки" aria-label="Настройки">
    <Icon name="gear" />
  </button>
  <button class="face" onclick={onprofile} title="Профиль" aria-label="Профиль">
    <Avatar id={session.me} nick={session.nick} size="sm" />
  </button>
</header>

<style>
  .top {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--line);
    background: linear-gradient(180deg, rgba(255, 255, 255, 0.035), transparent);
  }

  /* Не `.picker`: так называется корень выбора эмодзи, и общее правило из
     adequate.css с `overflow: hidden` срезало бы выпадающий список. */
  .switcher {
    position: relative;
    flex: none;
  }
  .pick {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 5px 10px 5px 6px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--bg-raised);
    color: var(--fg-dim);
    max-width: 14rem;
  }
  .pick:hover {
    border-color: var(--fg-dim);
  }
  .pick .nm {
    color: var(--fg-hi);
    font-weight: 620;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pick .nm.dim {
    color: var(--fg-dimmer);
    font-weight: 400;
  }

  .veil {
    position: fixed;
    inset: 0;
    z-index: 40;
  }
  .menu {
    position: absolute;
    z-index: 41;
    top: calc(100% + 6px);
    left: 0;
    width: 17rem;
    padding: 6px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--bg-raised);
    box-shadow: 0 30px 60px -40px #000;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .menu button {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 7px 9px;
    border-radius: var(--radius-sm);
    color: var(--fg-dim);
    text-align: left;
  }
  .menu button:hover {
    background: var(--lift);
    color: var(--fg-hi);
  }
  .menu button.on {
    color: var(--fg-hi);
    background: var(--lift-2);
  }
  .menu .nm {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .menu .n {
    color: var(--fg-hi);
    font-size: var(--text-xs);
  }
  .menu .danger:hover {
    color: var(--fg-hi);
    background: var(--lift-2);
  }
  .rule {
    height: 1px;
    margin: 5px 4px;
    background: var(--line);
  }

  .search {
    flex: 1;
    min-width: 0;
    max-width: 26rem;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px;
    border: 1px solid var(--line);
    border-radius: 999px;
    background: var(--bg);
    color: var(--fg-dimmer);
  }
  .search:focus-within {
    border-color: var(--edge);
  }
  .search input {
    flex: 1;
    min-width: 0;
    color: var(--fg);
    font-size: var(--text-sm);
  }
  .search input::placeholder {
    color: var(--fg-faint);
  }
  .clear {
    color: var(--fg-dimmer);
    display: grid;
    place-items: center;
  }
  .clear:hover {
    color: var(--fg-hi);
  }

  .sp {
    flex: 1;
  }
  .face {
    display: grid;
    place-items: center;
    padding: 2px;
    border-radius: 999px;
  }
  .face:hover {
    box-shadow: 0 0 0 2px var(--edge);
  }

  @media (max-width: 900px) {
    .search {
      display: none;
    }
  }
</style>
