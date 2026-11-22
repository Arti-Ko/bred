<script lang="ts">
  // Настройки в адекватном режиме: разделы слева, содержимое справа.
  //
  // Раньше всё шло одной лентой, и микрофон соседствовал с код-паролем.
  // Теперь каждый раздел отвечает на один вопрос — «кто я», «как меня
  // слышно», «как выглядит», «кто может прочитать», «что за версия» — и
  // открытый раздел помещается в окно целиком.

  import { onDestroy, untrack } from 'svelte';
  import Avatar from './Avatar.svelte';
  import Dialog from './Dialog.svelte';
  import Icon from './Icon.svelte';
  import { api, errorText } from '../ipc';
  import { governance } from '../stores/governance.svelte';
  import { privacy } from '../stores/privacy.svelte';
  import { call } from '../stores/call.svelte';
  import { devices } from '../stores/devices.svelte';
  import type { DeviceKind } from '../stores/prefs.svelte';
  import { news } from '../stores/news.svelte';
  import { prefs } from '../stores/prefs.svelte';
  import { report } from '../stores/report.svelte';
  import { session } from '../stores/session.svelte';
  import { updates } from '../stores/updates.svelte';
  import { MicTest } from '../mictest';

  interface Props {
    onclose: () => void;
    onarcade: () => void;
  }
  const { onclose, onarcade }: Props = $props();

  type Tab = 'profile' | 'media' | 'look' | 'security' | 'space' | 'about';
  type IconName = 'person' | 'speaker' | 'screen' | 'lock' | 'people' | 'info';

  const tabs = $derived(
    (
      [
        { id: 'profile', label: 'Профиль', icon: 'person' },
        { id: 'media', label: 'Звук и видео', icon: 'speaker' },
        { id: 'look', label: 'Интерфейс', icon: 'screen' },
        { id: 'security', label: 'Безопасность', icon: 'lock' },
        { id: 'space', label: 'Пространство', icon: 'people' },
        { id: 'about', label: 'О БРЕДе', icon: 'info' },
      ] as Array<{ id: Tab; label: string; icon: IconName }>
    ).filter((tab) => tab.id !== 'space' || session.spaceId),
  );

  let tab = $state<Tab>('profile');
  let nick = $state(session.nick);
  let personal = $state('');
  let copied = $state('');
  let error = $state('');
  let busy = $state(false);

  const deviceRows: Array<{ kind: DeviceKind; label: string }> = [
    { kind: 'mic', label: 'Микрофон' },
    { kind: 'speaker', label: 'Колонки и наушники' },
    { kind: 'camera', label: 'Камера' },
  ];

  const stageText: Record<string, string> = {
    idle: 'не проверялось',
    checking: 'проверяем…',
    current: 'установлена последняя версия',
    available: 'доступно обновление',
    downloading: 'качаем…',
    ready: 'обновление установлено',
    failed: 'не удалось проверить',
  };

  $effect(() => {
    void privacy.load();
    void updates.init();
    if (!personal) {
      void api
        .personalLink()
        .then((value) => (personal = value))
        .catch(() => undefined);
    }
  });

  // Списки устройств — только когда открыт их раздел: там они и нужны.
  $effect(() => {
    if (tab === 'media') void devices.showAudio();
  });

  // Пространство закрыли, пока были в его разделе, — вернуться к началу.
  $effect(() => {
    if (tab === 'space' && !session.spaceId) tab = 'profile';
  });

  // Проверка микрофона: слышать себя и видеть уровень.
  let test = $state<MicTest | null>(null);
  let level = $state(0);
  let testError = $state('');

  async function startTest(): Promise<void> {
    stopTest();
    testError = '';
    const fresh = new MicTest();
    test = fresh;
    try {
      const [mic, speaker] = await Promise.all([devices.resolve('mic'), devices.resolve('speaker')]);
      await fresh.start(mic, speaker, (value) => (level = value));
    } catch (issue) {
      if (test === fresh) stopTest();
      testError = `микрофон не открылся: ${errorText(issue)}`;
    }
  }

  function stopTest(): void {
    test?.stop();
    test = null;
    level = 0;
  }

  async function pickDevice(kind: DeviceKind, id: string): Promise<void> {
    await call.chooseDevice(kind, id);
    // Сменили микрофон или колонки посреди проверки — слушать уже новые.
    if (test && kind !== 'camera') await startTest();
  }

  // Ушли из раздела или закрыли настройки — микрофон отпускаем.
  $effect(() => {
    if (tab !== 'media') untrack(stopTest);
  });
  onDestroy(() => test?.stop());

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

{#snippet toggle(on: boolean, label: string, flip: () => void)}
  <button class="toggle" class:on role="switch" aria-checked={on} aria-label={label} onclick={flip}></button>
{/snippet}

<Dialog title="Настройки" {onclose} wide>
  <div class="layout">
    <nav aria-label="Разделы настроек">
      {#each tabs as item (item.id)}
        <button class="tab" class:active={tab === item.id} aria-current={tab === item.id ? 'page' : undefined} onclick={() => (tab = item.id)}>
          <Icon name={item.icon} size={14} />
          <span>{item.label}</span>
        </button>
      {/each}
    </nav>

    <div class="page">
      {#if tab === 'profile'}
        <h2>Профиль</h2>
        <div class="face">
          <Avatar id={session.me} nick={session.nick} size="lg" />
          <div class="face-text">
            <b>{session.nick}</b>
            <span>картинку видят все, с кем вы в одном пространстве</span>
          </div>
          <button class="btn small" onclick={() => session.setAvatar()}>Сменить</button>
        </div>
        <div class="block">
          <span class="field-label">Имя</span>
          <div class="pair">
            <input class="input" bind:value={nick} onkeydown={(e) => e.key === 'Enter' && saveNick()} />
            <button class="btn" disabled={busy || !nick.trim() || nick === session.nick} onclick={saveNick}>
              Сохранить
            </button>
          </div>
        </div>
        <div class="block">
          <span class="field-label">Личная ссылка</span>
          <div class="pair">
            <div class="input mono">{personal || 'готовим…'}</div>
            <button class="btn" disabled={!personal} onclick={() => copy(personal, 'визитка')}>
              {copied === 'визитка' ? 'Скопировано' : 'Скопировать'}
            </button>
          </div>
          <p class="hint-text">По ней вам напишут один на один, даже без общих пространств.</p>
        </div>
        <div class="block">
          <span class="field-label">Ключ устройства</span>
          <div class="pair">
            <div class="input mono">{session.me}</div>
            <button class="btn" onclick={() => copy(session.me, 'ключ')}>
              {copied === 'ключ' ? 'Скопировано' : 'Скопировать'}
            </button>
          </div>
        </div>
      {:else if tab === 'media'}
        <h2>Звук и видео</h2>
        <div class="block">
          {#each deviceRows as row (row.kind)}
            <label class="field">
              <span class="field-label">{row.label}</span>
              {#if row.kind === 'speaker' && !devices.canPickSpeaker}
                <div class="input muted">Как в системе — здесь выбрать нельзя</div>
              {:else}
                <select
                  class="input"
                  value={devices.selected(row.kind)}
                  onchange={(event) => void pickDevice(row.kind, event.currentTarget.value)}
                >
                  <option value="">Как в системе</option>
                  {#each devices.choices(row.kind) as device (device.id)}
                    <option value={device.id}>{device.label}</option>
                  {/each}
                </select>
              {/if}
            </label>
          {/each}
          {#if devices.hidden('camera')}
            <button class="row-btn" onclick={() => devices.reveal()}>
              <span class="label">
                <b>Показать камеры</b>
                <span>камера включится на миг — без этого система не называет их</span>
              </span>
            </button>
          {/if}
          {#if devices.error}<p class="error">{devices.error}</p>{/if}
        </div>
        <div class="block">
          <span class="field-label">Проверка микрофона</span>
          <div class="mic-test">
            <button class="btn" class:primary={!test} onclick={() => (test ? stopTest() : void startTest())}>
              {test ? 'Остановить' : 'Послушать себя'}
            </button>
            <div class="meter" role="meter" aria-label="Уровень микрофона" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(level * 100)}>
              <div class="meter-fill" style:transform="scaleX({level})"></div>
            </div>
          </div>
          <p class="hint-text">
            Звук пойдёт в выбранные колонки — так, как его слышат в звонке. Лучше в наушниках:
            из колонок он вернётся в микрофон и засвистит.
          </p>
          {#if testError}<p class="error">{testError}</p>{/if}
        </div>
        <div class="block">
          <div class="switch">
            <span class="label">
              <b>Видео в цвете</b>
              <span>по умолчанию монохром — под остальной интерфейс</span>
            </span>
            {@render toggle(prefs.colorVideo, 'Видео в цвете', () => prefs.toggleColorVideo())}
          </div>
          <p class="hint-text">Громкость каждого собеседника — правый клик по нему в звонке.</p>
        </div>
      {:else if tab === 'look'}
        <h2>Интерфейс</h2>
        <div class="block">
          <div class="switch">
            <span class="label">
              <b>Адекватный режим</b>
              <span>кнопки вместо команд · трофеев: {prefs.trophies.length} из 3</span>
            </span>
            {@render toggle(prefs.adequate, 'Адекватный режим', () => prefs.toggleAdequate())}
          </div>
          <div class="switch">
            <span class="label">
              <b>Звук нового сообщения</b>
              <span>из фона всегда, при открытом окне — только про другие каналы</span>
            </span>
            {@render toggle(prefs.soundOnMessage, 'Звук нового сообщения', () => prefs.toggleSoundOnMessage())}
          </div>
        </div>
        <button class="row-btn" onclick={onarcade}>
          <span class="label">
            <b>Зал с играми</b>
            <span>там же, где режим открывался</span>
          </span>
          <Icon name="caret" size={13} />
        </button>
      {:else if tab === 'security'}
        <h2>Безопасность</h2>
        {#if privacy.info}
          <div class="block">
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
          </div>
          <div class="block">
            <div class="switch">
              <span class="label">
                <b>Скрывать мой IP</b>
                <span>
                  собеседники видят адрес ретранслятора, а не ваш; звук идёт чуть дольше, без
                  интернета связи нет — даже в одной сети
                </span>
              </span>
              {@render toggle(privacy.info.hide_ip, 'Скрывать мой IP', () => privacy.toggleHideIp())}
            </div>
            {#if privacy.pendingRestart}
              <div class="notice">
                <b>Вступит в силу после перезапуска</b>
                <button class="btn primary small" onclick={() => updates.restart()}>Перезапустить</button>
              </div>
            {/if}
          </div>
          <div class="facts">
            <div><span>база на диске</span><b>зашифрована (SQLCipher)</b></div>
            <div><span>ключ базы</span><b>{privacy.keyText}</b></div>
            {#if governance.info && !governance.info.direct}
              <div><span>ключ пространства</span><b>№ {governance.info.epoch + 1}</b></div>
            {/if}
          </div>
        {:else}
          <p class="hint-text">Загружаем…</p>
        {/if}
      {:else if tab === 'space' && session.spaceId}
        <h2>Пространство «{session.space?.name}»</h2>
        <p class="hint-text">
          Выход стирает всю историю этого пространства на вашем устройстве. У остальных она
          останется.
        </p>
        <button class="btn danger" onclick={() => { void session.leaveSpace(); onclose(); }}>
          <Icon name="door" size={14} /> Покинуть пространство
        </button>
      {:else if tab === 'about'}
        <h2>О БРЕДе</h2>
        <div class="block">
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
          <button class="row-btn" onclick={() => { void news.show(); onclose(); }}>
            <span class="label">
              <b>Что нового</b>
              <span>изменения каждой версии</span>
            </span>
            <Icon name="bell" size={14} />
          </button>
          <button class="row-btn" onclick={() => report.show()}>
            <span class="label">
              <b>Сообщить о проблеме</b>
              <span>описание, фото или видео — разработчику в Telegram</span>
            </span>
            <Icon name="send" size={14} />
          </button>
        </div>
        <span class="field-label">Сеть</span>
        <div class="facts">
          <div><span>состояние</span><b>{session.online ? 'в сети' : 'поднимается'}</b></div>
          <div><span>соседей на связи</span><b>{session.neighbors}</b></div>
          <div><span>ретранслятор</span><b class="mono">{session.relay ?? 'нет'}</b></div>
          <div><span>внешний адрес</span><b class="mono">{session.external ?? 'не определён'}</b></div>
        </div>
      {/if}

      {#if error}<p class="error">{error}</p>{/if}
    </div>
  </div>
</Dialog>

<style>
  .layout {
    display: grid;
    grid-template-columns: 11.5rem 1fr;
    height: 100%;
    min-height: 0;
    border-top: 1px solid var(--line);
  }

  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 10px 8px;
    border-right: 1px solid var(--line);
    overflow-y: auto;
  }
  .tab {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 7px 10px;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--fg-dim);
    text-align: left;
    white-space: nowrap;
    transition: background var(--fast) var(--ease), color var(--fast) var(--ease);
  }
  .tab:hover {
    background: var(--bg);
    color: var(--fg-hi);
  }
  .tab:focus-visible {
    outline: 1px solid var(--fg-dim);
    outline-offset: -1px;
  }
  .tab.active {
    background: var(--inv-bg);
    color: var(--inv-fg);
  }

  .page {
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 14px 18px 18px;
    overflow-y: auto;
    min-width: 0;
  }
  h2 {
    margin: 0;
    color: var(--fg-hi);
    font-size: 17px;
    font-weight: 660;
    overflow-wrap: anywhere;
  }

  /* Узкое окно: разделы — строкой сверху */
  @media (max-width: 560px) {
    .layout {
      grid-template-columns: 1fr;
      grid-template-rows: auto 1fr;
    }
    nav {
      flex-direction: row;
      border-right: 0;
      border-bottom: 1px solid var(--line);
      overflow-x: auto;
    }
    .tab {
      width: auto;
    }
  }

  .block {
    display: flex;
    flex-direction: column;
    gap: 8px;
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

  .field {
    display: block;
  }
  .field select.input {
    width: 100%;
    cursor: pointer;
  }
  .mic-test {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .meter {
    flex: 1;
    height: 8px;
    border-radius: 999px;
    background: var(--bg);
    border: 1px solid var(--line);
    overflow: hidden;
  }
  .meter-fill {
    height: 100%;
    background: var(--fg-hi);
    transform-origin: left center;
    transition: transform 60ms linear;
  }
  .input.muted {
    color: var(--fg-dimmer);
  }

  .pair {
    display: flex;
    gap: 8px;
    align-items: stretch;
  }
  .pair .input {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .switch,
  .row-btn {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
    padding: 9px 11px;
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
    padding: 10px 12px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
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
  .field-label + .facts {
    margin-top: -8px;
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
