<script lang="ts">
  // Диалоги вместо команд с аргументами.
  //
  // Каждый отвечает на один вопрос и объясняет последствие прямо в тексте:
  // ошибиться синтаксисом здесь негде, потому что синтаксиса нет.

  import Avatar from './Avatar.svelte';
  import Dialog from './Dialog.svelte';
  import { api, errorText } from '../ipc';
  import { session } from '../stores/session.svelte';

  import type { Kind } from './kinds';

  interface Props {
    kind: Kind;
    onclose: () => void;
  }
  const { kind, onclose }: Props = $props();

  let name = $state('');
  let link = $state('');
  let voice = $state(false);
  let category = $state('общее');
  let ticket = $state('');
  let personal = $state('');
  let nick = $state(session.nick);
  let busy = $state(false);
  let error = $state('');
  let copied = $state('');

  $effect(() => {
    if (kind === 'приглашение' && session.spaceId && !ticket) {
      void api
        .spaceInvite(session.spaceId)
        .then((value) => (ticket = value))
        .catch((issue) => (error = errorText(issue)));
    }
    if (kind === 'профиль' && !personal) {
      void api
        .personalLink()
        .then((value) => (personal = value))
        .catch(() => undefined);
    }
  });

  async function copy(value: string, what: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(value);
      copied = what;
      setTimeout(() => (copied = ''), 1800);
    } catch (issue) {
      error = errorText(issue);
    }
  }

  /** Общая обёртка: любое действие либо закрывает диалог, либо показывает, что не так. */
  async function run(action: () => Promise<void>): Promise<void> {
    busy = true;
    error = '';
    try {
      await action();
      onclose();
    } catch (issue) {
      error = errorText(issue);
    } finally {
      busy = false;
    }
  }
</script>

{#if kind === 'приглашение'}
  <Dialog title={`Пригласить в «${session.space?.name ?? ''}»`} {onclose}>
    <p class="hint-text">
      Отправьте эту ссылку любым способом — мессенджером, почтой, как угодно. Кто её получил,
      тот участник: ни регистрации, ни почты, ни пароля не потребуется.
    </p>
    <div>
      <span class="field-label">Ссылка-приглашение</span>
      <div class="input mono">{ticket || 'готовим ссылку…'}</div>
    </div>
    {#if error}<p class="error">{error}</p>{/if}
    {#snippet footer()}
      <button class="btn quiet" onclick={onclose}>Закрыть</button>
      <button class="btn primary" disabled={!ticket} onclick={() => copy(ticket, 'ссылка')}>
        {copied === 'ссылка' ? 'Скопировано' : 'Скопировать'}
      </button>
    {/snippet}
  </Dialog>
{:else if kind === 'канал'}
  <Dialog title="Новый канал" {onclose}>
    <div>
      <span class="field-label">Название</span>
      <!-- svelte-ignore a11y_autofocus -->
      <input class="input" bind:value={name} placeholder="баги" autofocus />
    </div>
    <div>
      <span class="field-label">Тип</span>
      <div class="segmented">
        <button class:on={!voice} onclick={() => (voice = false)}># Текстовый</button>
        <button class:on={voice} onclick={() => (voice = true)}>Голосовой</button>
      </div>
    </div>
    <div>
      <span class="field-label">Раздел</span>
      <input class="input" bind:value={category} placeholder="общее" />
    </div>
    <p class="hint-text">
      {voice
        ? 'В голосовой канал заходят кликом — он и есть комната звонка.'
        : 'Текстовый канал увидят все участники пространства.'}
    </p>
    {#if error}<p class="error">{error}</p>{/if}
    {#snippet footer()}
      <button class="btn quiet" onclick={onclose}>Отмена</button>
      <button
        class="btn primary"
        disabled={busy || !name.trim()}
        onclick={() => run(() => session.createChannel(name, category, voice))}
      >
        Создать канал
      </button>
    {/snippet}
  </Dialog>
{:else if kind === 'пространство'}
  <Dialog title="Новое пространство" {onclose}>
    <p class="hint-text">
      Пространство — это место для компании или команды: каналы, файлы и звонки внутри него.
      Сервера у него нет, поэтому и названия хватит одного.
    </p>
    <div>
      <span class="field-label">Название</span>
      <!-- svelte-ignore a11y_autofocus -->
      <input class="input" bind:value={name} placeholder="Орбита" autofocus />
    </div>
    {#if error}<p class="error">{error}</p>{/if}
    {#snippet footer()}
      <button class="btn quiet" onclick={onclose}>Отмена</button>
      <button
        class="btn primary"
        disabled={busy || !name.trim()}
        onclick={() => run(() => session.createSpace(name))}
      >
        Создать
      </button>
    {/snippet}
  </Dialog>
{:else if kind === 'ссылка'}
  <Dialog title="Войти по ссылке" {onclose}>
    <p class="hint-text">
      Вставьте ссылку, которую вам прислали: приглашение в пространство или чью-то личную
      визитку. Разбираться, какая из них какая, не нужно.
    </p>
    <div>
      <span class="field-label">Ссылка</span>
      <!-- svelte-ignore a11y_autofocus -->
      <input class="input mono" bind:value={link} placeholder="bred://join/…" autofocus />
    </div>
    {#if error}<p class="error">{error}</p>{/if}
    {#snippet footer()}
      <button class="btn quiet" onclick={onclose}>Отмена</button>
      <button
        class="btn primary"
        disabled={busy || !link.trim()}
        onclick={() => run(() => session.joinByLink(link))}
      >
        Войти
      </button>
    {/snippet}
  </Dialog>
{:else if kind === 'профиль'}
  <Dialog title="Профиль" {onclose}>
    <div class="face-row">
      <Avatar id={session.me} nick={session.nick} size="lg" />
      <button class="btn small" onclick={() => session.setAvatar()}>Сменить картинку</button>
    </div>
    <div>
      <span class="field-label">Имя</span>
      <input class="input" bind:value={nick} placeholder="как вас зовут" />
    </div>
    <div>
      <span class="field-label">Личная ссылка</span>
      <div class="input mono">{personal || 'готовим ссылку…'}</div>
    </div>
    <p class="hint-text">
      По личной ссылке вам напишут один на один, даже если общих пространств у вас нет.
    </p>
    {#if error}<p class="error">{error}</p>{/if}
    {#snippet footer()}
      <button class="btn" disabled={!personal} onclick={() => copy(personal, 'визитка')}>
        {copied === 'визитка' ? 'Скопировано' : 'Скопировать ссылку'}
      </button>
      <button
        class="btn primary"
        disabled={busy || !nick.trim() || nick === session.nick}
        onclick={() => run(() => session.rename(nick))}
      >
        Сохранить имя
      </button>
    {/snippet}
  </Dialog>
{:else}
  <Dialog title="Написать одному человеку" {onclose}>
    <p class="hint-text">
      Переписка один на один: её ключ обе стороны вычисляют у себя, поэтому прочитать её
      посторонний не может в принципе.
    </p>
    <div class="who-list">
      {#each session.members.filter((m) => m.id !== session.me) as member (member.id)}
        <button
          class="who"
          onclick={() => {
            void session.openDirect(member.id);
            onclose();
          }}
        >
          <Avatar id={member.id} nick={member.nick} />
          <span class="nm">
            <b>{member.nick}</b>
            <span>{member.online ? 'в сети' : 'не в сети'}</span>
          </span>
        </button>
      {:else}
        <p class="hint-text">Кроме вас здесь пока никого — сперва позовите кого-нибудь.</p>
      {/each}
    </div>
  </Dialog>
{/if}

<style>
  .error {
    margin: 0;
    color: var(--fg-hi);
    font-size: var(--text-sm);
    padding: 7px 10px;
    border: 1px solid var(--edge);
    border-radius: var(--radius-sm);
  }

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
    padding: 6px 10px;
    border-radius: 6px;
    color: var(--fg-dim);
    font-size: var(--text-sm);
  }
  .segmented button:hover {
    color: var(--fg-hi);
  }
  .segmented button.on {
    background: var(--inv-bg);
    color: var(--inv-fg);
    font-weight: 620;
  }

  .face-row {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .who-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 18rem;
    overflow-y: auto;
  }
  .who {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 7px 8px;
    border-radius: var(--radius-sm);
    text-align: left;
  }
  .who:hover {
    background: var(--lift);
  }
  .who .nm b {
    display: block;
    color: var(--fg);
    font-weight: 560;
  }
  .who .nm span {
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
  }
</style>
