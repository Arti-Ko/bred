<script lang="ts">
  // Настройки в адекватном режиме: не список строк с подсказками «меняется
  // командой», а нормальные поля и переключатели. Всё, что раньше требовало
  // команды, здесь делается на месте.

  import Avatar from './Avatar.svelte';
  import Dialog from './Dialog.svelte';
  import Icon from './Icon.svelte';
  import { api, errorText } from '../ipc';
  import { governance } from '../stores/governance.svelte';
  import { privacy } from '../stores/privacy.svelte';
  import { news } from '../stores/news.svelte';
  import { prefs } from '../stores/prefs.svelte';
  import { report } from '../stores/report.svelte';
  import { session } from '../stores/session.svelte';
  import { updates } from '../stores/updates.svelte';

  interface Props {
    onclose: () => void;
    onarcade: () => void;
  }
  const { onclose, onarcade }: Props = $props();

  let nick = $state(session.nick);
  let personal = $state('');
  let copied = $state('');
  let error = $state('');
  let busy = $state(false);

  $effect(() => {
    void privacy.load();
  });

  $effect(() => {
    void updates.init();
    if (!personal) {
      void api
        .personalLink()
        .then((value) => (personal = value))
        .catch(() => undefined);
    }
  });

  const stageText: Record<string, string> = {
    idle: 'не проверялось',
    checking: 'проверяем…',
    current: 'установлена последняя версия',
    available: 'доступно обновление',
    downloading: 'качаем…',
    ready: 'обновление установлено',
    failed: 'не удалось проверить',
  };

  async function copy(value: string, what: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(value);
      copied = what;
      setTimeout(() => (copied = ''), 1800);
    } catch (issue) {
      error = errorText(issue);
    }
  }

  async function saveNick(): Promise<void> {
    busy = true;
    error = '';
    try {
      await session.rename(nick);
    } catch (issue) {
      error = errorText(issue);
    } finally {
      busy = false;
    }
  }
</script>

<Dialog title="Настройки" {onclose}>
  <section>
    <h3>Профиль</h3>
    <div class="face">
      <Avatar id={session.me} nick={session.nick} size="lg" />
      <div class="face-text">
        <b>{session.nick}</b>
        <span>картинку видят все, с кем вы в одном пространстве</span>
      </div>
      <button class="btn small" onclick={() => session.setAvatar()}>Сменить</button>
    </div>
    <div>
      <span class="field-label">Имя</span>
      <div class="pair">
        <input class="input" bind:value={nick} onkeydown={(e) => e.key === 'Enter' && saveNick()} />
        <button class="btn" disabled={busy || !nick.trim() || nick === session.nick} onclick={saveNick}>
          Сохранить
        </button>
      </div>
    </div>
    <div>
      <span class="field-label">Личная ссылка</span>
      <div class="pair">
        <div class="input mono">{personal || 'готовим…'}</div>
        <button class="btn" disabled={!personal} onclick={() => copy(personal, 'визитка')}>
          {copied === 'визитка' ? 'Скопировано' : 'Скопировать'}
        </button>
      </div>
      <p class="hint-text">По ней вам напишут один на один, даже без общих пространств.</p>
    </div>
    <div>
      <span class="field-label">Ключ устройства</span>
      <div class="input mono">{session.me}</div>
    </div>
  </section>

  <section>
    <h3>Оформление</h3>
    <div class="switch">
      <span class="label">
        <b>Адекватный режим</b>
        <span>кнопки вместо команд · трофеев: {prefs.trophies.length} из 3</span>
      </span>
      <button
        class="toggle"
        class:on={prefs.adequate}
        role="switch"
        aria-checked={prefs.adequate}
        aria-label="Адекватный режим"
        onclick={() => prefs.toggleAdequate()}
      ></button>
    </div>
    <button class="row-btn" onclick={onarcade}>
      <span class="label">
        <b>Зал с играми</b>
        <span>там же, где режим открывался</span>
      </span>
      <Icon name="caret" size={13} />
    </button>
  </section>

  <section>
    <h3>Звонки и уведомления</h3>
    <div class="switch">
      <span class="label">
        <b>Видео в цвете</b>
        <span>по умолчанию монохром — под остальной интерфейс</span>
      </span>
      <button
        class="toggle"
        class:on={prefs.colorVideo}
        role="switch"
        aria-checked={prefs.colorVideo}
        aria-label="Видео в цвете"
        onclick={() => prefs.toggleColorVideo()}
      ></button>
    </div>
    <div class="switch">
      <span class="label">
        <b>Звук нового сообщения</b>
        <span>из фона всегда, при открытом окне — только про другие каналы</span>
      </span>
      <button
        class="toggle"
        class:on={prefs.soundOnMessage}
        role="switch"
        aria-checked={prefs.soundOnMessage}
        aria-label="Звук нового сообщения"
        onclick={() => prefs.toggleSoundOnMessage()}
      ></button>
    </div>
    <p class="hint-text">
      Громкость каждого собеседника — правый клик по нему в звонке.
    </p>
  </section>

  <section>
    <h3>Помощь</h3>
    <button class="row-btn" onclick={() => report.show()}>
      <span class="label">
        <b>Сообщить о проблеме</b>
        <span>Описание, фото или видео — разработчику в Telegram</span>
      </span>
      <Icon name="send" size={14} />
    </button>
    <button class="row-btn" onclick={() => { void news.show(); onclose(); }}>
      <span class="label">
        <b>Обновления БРЕД</b>
        <span>Что нового в каждой версии</span>
      </span>
      <Icon name="bell" size={14} />
    </button>
  </section>

  <section>
    <h3>Безопасность</h3>
    {#if privacy.info}
      <div class="switch">
        <span class="label">
          <b>Скрывать мой IP</b>
          <span>
            собеседники видят адрес ретранслятора, а не ваш; звук идёт чуть дольше, без
            интернета связи нет — даже в одной сети
          </span>
        </span>
        <button
          class="toggle"
          class:on={privacy.info.hide_ip}
          role="switch"
          aria-checked={privacy.info.hide_ip}
          aria-label="Скрывать мой IP"
          onclick={() => privacy.toggleHideIp()}
        ></button>
      </div>
      {#if privacy.pendingRestart}
        <div class="notice">
          <b>Вступит в силу после перезапуска</b>
          <button class="btn primary small" onclick={() => updates.restart()}>Перезапустить</button>
        </div>
      {/if}

      <div class="switch">
        <span class="label">
          <b>Код-пароль {privacy.info.passcode ? '· включён' : ''}</b>
          <span>
            {privacy.info.passcode
              ? 'без кода база не откроется ни здесь, ни с копии диска'
              : privacy.info.key_storage === 'system'
                ? 'сейчас ключ базы защищает вход в Windows; с кодом — ещё и ваш код'
                : 'сейчас ключ базы лежит рядом с ней; с кодом прочитать базу без вас нельзя'}
          </span>
        </span>
        {#if privacy.info.passcode}
          <button class="btn small" onclick={() => privacy.open('change')}>Сменить</button>
          <button class="btn small quiet" onclick={() => privacy.open('clear')}>Убрать</button>
        {:else}
          <button class="btn small" onclick={() => privacy.open('set')}>Поставить</button>
        {/if}
      </div>
      {#if privacy.form}
        <form
          class="code"
          onsubmit={(event) => {
            event.preventDefault();
            void privacy.submit();
          }}
        >
          {#if privacy.form !== 'set'}
            <input class="input" type="password" bind:value={privacy.current} placeholder="нынешний код" autocomplete="current-password" />
          {/if}
          {#if privacy.form !== 'clear'}
            <input class="input" type="password" bind:value={privacy.fresh} placeholder="новый код — от 6 знаков" autocomplete="new-password" />
            <input class="input" type="password" bind:value={privacy.repeat} placeholder="ещё раз" autocomplete="new-password" />
            <p class="hint-text">
              Забудете код — переписку на этом устройстве не восстановить: обойти его нельзя,
              иначе он ничего бы не защищал. Четыре цифры подбираются за час, фраза из слов — никогда.
            </p>
          {/if}
          {#if privacy.error}<p class="error">{privacy.error}</p>{/if}
          <div class="code-acts">
            <button type="button" class="btn quiet small" onclick={() => privacy.open('')}>Отмена</button>
            <button type="submit" class="btn primary small" disabled={privacy.busy}>
              {privacy.form === 'clear' ? 'Убрать код' : 'Сохранить код'}
            </button>
          </div>
        </form>
      {/if}
      {#if privacy.info.passcode}
        <button class="row-btn" onclick={() => updates.restart()}>
          <span class="label">
            <b>Запереть сейчас</b>
            <span>приложение перезапустится и спросит код</span>
          </span>
          <Icon name="door" size={14} />
        </button>
      {/if}

      <div class="facts">
        <div><span>база на диске</span><b>зашифрована (SQLCipher)</b></div>
        <div>
          <span>ключ базы</span>
<b>{privacy.keyText}</b>
        </div>
        {#if governance.info && !governance.info.direct}
          <div><span>ключ пространства</span><b>№ {governance.info.epoch + 1}</b></div>
        {/if}
      </div>
    {/if}
  </section>

  <section>
    <h3>Сеть</h3>
    <div class="facts">
      <div><span>состояние</span><b>{session.online ? 'в сети' : 'поднимается'}</b></div>
      <div><span>соседей на связи</span><b>{session.neighbors}</b></div>
      <div><span>ретранслятор</span><b class="mono">{session.relay ?? 'нет'}</b></div>
      <div><span>внешний адрес</span><b class="mono">{session.external ?? 'не определён'}</b></div>
    </div>
  </section>

  <section>
    <h3>Обновление</h3>
    <div class="switch">
      <span class="label">
        <b>Версия {updates.version || '…'}</b>
        <span>
          {stageText[updates.stage] ?? updates.stage}{updates.stage === 'downloading' && updates.progress > 0
            ? ` · ${updates.progress}%`
            : ''}
        </span>
      </span>
      <button class="btn" disabled={updates.stage === 'checking'} onclick={() => updates.check()}>
        Проверить
      </button>
    </div>
    {#if updates.stage === 'available'}
      <div class="notice">
        <b>Доступна версия {updates.next}</b>
        {#if updates.notes}<pre class="notes selectable">{updates.notes}</pre>{/if}
        <button class="btn primary small" onclick={() => updates.install()}>Установить</button>
      </div>
    {/if}
    {#if updates.stage === 'ready'}
      <div class="notice">
        <b>Готово, нужен перезапуск</b>
        <button class="btn primary small" onclick={() => updates.restart()}>Перезапустить</button>
      </div>
    {/if}
    {#if updates.error}<p class="error">{updates.error}</p>{/if}
  </section>

  {#if session.spaceId}
    <section>
      <h3>Пространство «{session.space?.name}»</h3>
      <p class="hint-text">
        Выход стирает всю историю этого пространства на вашем устройстве. У остальных она
        останется.
      </p>
      <button class="btn danger" onclick={() => { void session.leaveSpace(); onclose(); }}>
        <Icon name="door" size={14} /> Покинуть пространство
      </button>
    </section>
  {/if}

  {#if error}<p class="error">{error}</p>{/if}
</Dialog>

<style>
  section {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding-bottom: 14px;
    border-bottom: 1px solid var(--line);
  }
  section:last-of-type {
    border-bottom: 0;
    padding-bottom: 0;
  }
  h3 {
    margin: 0;
    font-size: 11px;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--fg-dimmer);
    font-weight: 600;
  }

  .face {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .face-text {
    flex: 1;
    min-width: 0;
  }
  .face-text b {
    display: block;
    color: var(--fg-hi);
    font-weight: 620;
  }
  .face-text span {
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
  }

  .pair {
    display: flex;
    gap: 8px;
    align-items: stretch;
  }
  .pair .input {
    flex: 1;
    min-width: 0;
  }

  .switch,
  .row-btn {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 8px 10px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--bg);
    text-align: left;
  }
  .row-btn:hover {
    border-color: var(--fg-dim);
    color: var(--fg-hi);
  }
  .label {
    flex: 1;
    min-width: 0;
  }
  .label b {
    display: block;
    color: var(--fg);
    font-weight: 560;
  }
  .label span {
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
  }

  /* Переключатель: положение читается формой, а не цветом */
  .toggle {
    width: 42px;
    height: 24px;
    flex: none;
    position: relative;
    border: 1px solid var(--line);
    border-radius: 999px;
    background: var(--bg-raised);
    transition: background var(--fast) var(--ease), border-color var(--fast) var(--ease);
  }
  .toggle::after {
    content: '';
    position: absolute;
    top: 2px;
    left: 2px;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--fg-dim);
    transition: transform var(--fast) var(--ease), background var(--fast) var(--ease);
  }
  .toggle.on {
    background: var(--inv-bg);
    border-color: var(--inv-bg);
  }
  .toggle.on::after {
    transform: translateX(18px);
    background: var(--inv-fg);
  }

  .code {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
  }
  .code-acts {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .facts {
    display: grid;
    gap: 6px;
  }
  .facts div {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    font-size: var(--text-sm);
    color: var(--fg-dimmer);
  }
  .facts b {
    color: var(--fg);
    font-weight: 560;
    text-align: right;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .facts b.mono {
    font-family: var(--mono);
    font-size: var(--text-xs);
  }

  .notice {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
    padding: 10px 12px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--bg);
  }
  .notice b {
    color: var(--fg-hi);
  }
  .notes {
    margin: 0;
    max-height: 8rem;
    overflow-y: auto;
    font-size: var(--text-xs);
    color: var(--fg-dim);
    white-space: pre-wrap;
  }

  .error {
    margin: 0;
    padding: 7px 10px;
    border: 1px solid var(--edge);
    border-radius: var(--radius-sm);
    color: var(--fg-hi);
    font-size: var(--text-sm);
  }

  /* Опасное действие — не цветом, а контрастом рамки */
  .btn.danger {
    align-self: flex-start;
    border-color: var(--edge);
    color: var(--fg-hi);
  }
</style>
