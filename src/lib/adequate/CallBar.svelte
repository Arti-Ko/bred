<script lang="ts">
  // Панель звонка в адекватном режиме.
  //
  // Слева — где идёт разговор, сколько людей и как связь: значок связи сразу
  // показывает худшую задержку и открывает окно «Связь». Справа — управление
  // иконками с подписями, как в привычных звонилках: состояние читается по
  // значку (перечёркнутый микрофон), а не по зачёркнутому тексту.

  import CallLinks from '../components/CallLinks.svelte';
  import Icon from './Icon.svelte';
  import { api, type LinkReport } from '../ipc';
  import { lines, worse, type Grade } from '../linkquality';
  import { call, SHARE_PLAYER } from '../stores/call.svelte';
  import { session } from '../stores/session.svelte';

  interface Props {
    hasScreen: boolean;
  }
  const { hasScreen }: Props = $props();

  /** Значок связи обновляется реже окна: он лишь зовёт его открыть. */
  const NET_REFRESH_MS = 2000;

  let linksOpen = $state(false);
  let report = $state<LinkReport | null>(null);

  $effect(() => {
    if (linksOpen) return; // окно спрашивает само
    let alive = true;
    const pull = (): void => {
      void api
        .callLinks()
        .then((fresh) => alive && (report = fresh))
        .catch(() => undefined);
    };
    pull();
    const timer = setInterval(pull, NET_REFRESH_MS);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  });

  const channel = $derived(session.channels.find((c) => c.id === call.channel)?.name ?? 'звонок');
  const count = $derived(call.participants.length);
  const others = $derived(call.participants.filter((p) => p.id !== session.me).map((p) => p.id));

  /** Своя связь — худшая оценка и самая долгая задержка. */
  const net = $derived.by(() => {
    if (!report || others.length === 0) return { grade: 'good' as Grade, label: 'Связь' };
    const present = call.participants.map((p) => p.id);
    const mine = lines(report, session.me, present).filter((l) => l.a === session.me || l.b === session.me);
    const grade = mine.reduce<Grade>((acc, l) => worse(acc, l.grade), 'good');
    const rtt = Math.max(0, ...mine.map((l) => l.rtt));
    return { grade, label: grade === 'none' ? 'Нет связи' : rtt > 0 ? `${rtt} мс` : 'Связь' };
  });

  function people(n: number): string {
    const tail = n % 100;
    if (tail >= 11 && tail <= 14) return `${n} человек`;
    if (n % 10 === 1) return `${n} человек`;
    if (n % 10 >= 2 && n % 10 <= 4) return `${n} человека`;
    return `${n} человек`;
  }
</script>

<div class="call-top">
  <span class="live" aria-hidden="true"></span>
  <b>{channel}</b>
  <span class="count">{people(count)}</span>
  <button class="net {net.grade}" onclick={() => (linksOpen = true)} title="Связь в звонке: кто до кого как дотягивается">
    <Icon name="signal" size={13} />
    <span>{net.label}</span>
  </button>
</div>

<div class="call-dock" role="toolbar" aria-label="Управление звонком">
  <button
    class="round"
    class:alert={call.micMuted}
    aria-pressed={!call.micMuted}
    aria-label={call.micMuted ? 'Включить микрофон' : 'Выключить микрофон'}
    data-tip={call.micMuted ? 'Включить микрофон' : 'Выключить микрофон'}
    onclick={() => call.toggleMic()}
  >
    <Icon name={call.micMuted ? 'mic-off' : 'mic'} size={18} />
    <!-- Своего голоса не слышно — эхоподавление на то и стоит. Полоска в
         кнопке показывает, что звук идёт именно от вас. -->
    <span class="lvl" class:live={call.speakingSelf}>
      <i style="transform: scaleX({call.micMuted ? 0 : call.level})"></i>
    </span>
  </button>
  <button
    class="round"
    aria-pressed={call.camOn}
    aria-label={call.camOn ? 'Выключить камеру' : 'Включить камеру'}
    data-tip={call.camOn ? 'Выключить камеру' : 'Включить камеру'}
    onclick={() => call.toggleCamera()}
  >
    <Icon name={call.camOn ? 'camera' : 'camera-off'} size={18} />
  </button>
  <button
    class="round"
    aria-pressed={call.screenOn}
    aria-label={call.screenOn ? 'Остановить показ' : 'Показать экран'}
    data-tip={call.screenOn ? 'Остановить показ' : 'Показать экран'}
    onclick={() => call.toggleScreen()}
  >
    <Icon name="screen" size={18} />
  </button>
  {#if call.screenOn && call.screenAudioAvailable}
    <button
      class="round"
      aria-pressed={call.screenAudio}
      aria-label="Звук экрана"
      data-tip={call.screenAudio ? 'Звук экрана идёт' : 'Звук экрана выключен'}
      onclick={() => call.toggleScreenAudio()}
    >
      <Icon name="speaker" size={18} />
    </button>
  {/if}

  <span class="sep" aria-hidden="true"></span>

  <!-- У тех, кто в сети, зазвонит звонок с кнопкой «подключиться». -->
  <button class="round" aria-label="Позвать всех" data-tip="Позвать всех" onclick={async () => (call.status = await call.ring())}>
    <Icon name="bell" size={18} />
  </button>
  {#if SHARE_PLAYER}
    <button
      class="round"
      aria-pressed={call.sharingMusic}
      aria-label="Слушать вместе"
      data-tip="Слушать вместе"
      onclick={() => (call.sharingMusic ? call.stopMusic() : call.toggleMusicPicker())}
    >
      <Icon name="speaker" size={18} />
    </button>
  {/if}
  {#if hasScreen && call.expanded}
    <button class="round" aria-pressed={call.screenOnly} aria-label="Только экран" data-tip="Только экран" onclick={() => call.toggleScreenOnly()}>
      <Icon name="screen" size={18} />
    </button>
    <button
      class="round"
      aria-pressed={call.fullscreen}
      aria-label={call.fullscreen ? 'Из полного экрана' : 'Во весь экран'}
      data-tip={call.fullscreen ? 'Из полного экрана' : 'Во весь экран'}
      onclick={() => call.toggleFullscreen()}
    >
      <Icon name="expand" size={18} />
    </button>
  {/if}
  <button
    class="round"
    aria-label={call.expanded ? 'Свернуть' : 'Развернуть'}
    data-tip={call.expanded ? 'Свернуть · Esc' : 'Развернуть'}
    onclick={() => call.toggleExpanded()}
  >
    <Icon name={call.expanded ? 'collapse' : 'expand'} size={18} />
  </button>
  <button class="hangup" aria-label="Выйти из звонка" data-tip="Выйти из звонка" onclick={() => call.leave()}>
    <Icon name="hangup" size={20} />
  </button>
</div>

{#if linksOpen}
  <CallLinks onclose={() => (linksOpen = false)} />
{/if}

<style>
  .call-top,
  .call-dock {
    --q-good: #7bd393;
    --q-fair: #e6bd5c;
    --q-bad: #f07467;
    position: absolute;
    z-index: 4;
  }

  /* Где идёт разговор — мелко в углу: это справка, а не главное */
  .call-top {
    top: 10px;
    left: 14px;
    right: 14px;
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    pointer-events: none;
  }
  .call-top > * {
    pointer-events: auto;
  }
  .live {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--q-good);
    animation: pulse 2s ease-out infinite;
  }
  @keyframes pulse {
    0% {
      box-shadow: 0 0 0 0 color-mix(in srgb, var(--q-good) 60%, transparent);
    }
    100% {
      box-shadow: 0 0 0 7px transparent;
    }
  }
  .call-top b {
    color: var(--fg-hi);
    font-weight: 640;
    font-size: var(--text-sm);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .count {
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
    white-space: nowrap;
  }

  .net {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-left: auto;
    padding: 3px 10px;
    border: 1px solid var(--line);
    border-radius: 999px;
    background: color-mix(in srgb, var(--bg) 80%, transparent);
    color: var(--fg-dim);
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    transition: border-color var(--fast) var(--ease), color var(--fast) var(--ease);
  }
  .net:hover {
    border-color: var(--fg-dim);
  }
  .net.good {
    color: var(--q-good);
  }
  .net.fair {
    color: var(--q-fair);
    border-color: color-mix(in srgb, var(--q-fair) 40%, var(--line));
  }
  .net.bad,
  .net.none {
    color: var(--q-bad);
    border-color: color-mix(in srgb, var(--q-bad) 50%, var(--line));
  }

  /* Полоска управления — плавает внизу по центру, над плитками */
  .call-dock {
    bottom: 12px;
    left: 50%;
    transform: translateX(-50%);
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px;
    border: 1px solid var(--line);
    border-radius: 999px;
    background: color-mix(in srgb, var(--bg-raised) 92%, transparent);
    backdrop-filter: blur(12px);
    box-shadow: 0 18px 40px -20px #000;
    max-width: calc(100% - 24px);
  }
  .sep {
    width: 1px;
    height: 24px;
    margin: 0 4px;
    background: var(--line);
  }

  .round,
  .hangup {
    position: relative;
    display: grid;
    place-items: center;
    flex: none;
    height: 42px;
    border-radius: 999px;
    transition: background var(--fast) var(--ease), color var(--fast) var(--ease), transform var(--fast) var(--ease);
  }
  .round {
    width: 42px;
    background: var(--bg);
    color: var(--fg-dim);
  }
  .round:hover {
    background: var(--lift, var(--bg));
    color: var(--fg-hi);
  }
  .round:active,
  .hangup:active {
    transform: scale(0.94);
  }
  /* Включённое — светлым кругом */
  .round[aria-pressed='true'] {
    background: var(--inv-bg);
    color: var(--inv-fg);
  }
  /* Микрофон выключен — предупреждение: говорите, а вас не слышно */
  .round.alert {
    background: color-mix(in srgb, var(--q-bad) 18%, var(--bg));
    color: var(--q-bad);
  }
  .hangup {
    width: 58px;
    margin-left: 4px;
    background: #d9534a;
    color: #fff;
  }
  .hangup:hover {
    background: #e8605a;
  }
  .round:focus-visible,
  .hangup:focus-visible,
  .net:focus-visible {
    outline: 2px solid var(--edge);
    outline-offset: 2px;
  }

  /* Подпись кнопки — над ней при наведении */
  [data-tip]::after {
    content: attr(data-tip);
    position: absolute;
    bottom: calc(100% + 10px);
    left: 50%;
    transform: translate(-50%, 4px);
    padding: 4px 8px;
    border-radius: 6px;
    background: var(--inv-bg);
    color: var(--inv-fg);
    font-size: 11px;
    font-weight: 560;
    white-space: nowrap;
    opacity: 0;
    pointer-events: none;
    transition: opacity var(--fast) var(--ease), transform var(--fast) var(--ease);
  }
  [data-tip]:hover::after,
  [data-tip]:focus-visible::after {
    opacity: 1;
    transform: translate(-50%, 0);
  }

  /* Уровень своего голоса — дужкой внизу кнопки микрофона */
  .lvl {
    position: absolute;
    left: 11px;
    right: 11px;
    bottom: 7px;
    height: 2px;
    border-radius: 2px;
    overflow: hidden;
    background: color-mix(in srgb, currentColor 18%, transparent);
  }
  .lvl i {
    position: absolute;
    inset: 0;
    background: currentColor;
    opacity: 0.55;
    transform-origin: left center;
    transition: transform var(--fast) linear;
  }
  .lvl.live i {
    opacity: 1;
  }

  @media (prefers-reduced-motion: reduce) {
    .live {
      animation: none;
    }
  }
</style>
