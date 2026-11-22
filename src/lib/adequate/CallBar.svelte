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

<div class="bar">
  <div class="where">
    <span class="live" aria-hidden="true"></span>
    <div class="where-text">
      <b>{channel}</b>
      <span>{people(count)} в разговоре</span>
    </div>
    <button class="net {net.grade}" onclick={() => (linksOpen = true)} title="Связь в звонке: кто до кого как дотягивается">
      <Icon name="signal" size={14} />
      <span>{net.label}</span>
    </button>
  </div>

  <div class="controls" role="toolbar" aria-label="Управление звонком">
    <button class="ctl" class:alert={call.micMuted} aria-pressed={!call.micMuted} onclick={() => call.toggleMic()}>
      <span class="ic">
        <Icon name={call.micMuted ? 'mic-off' : 'mic'} size={17} />
        <!-- Своего голоса не слышно — эхоподавление на то и стоит. Полоска
             в кнопке показывает, что звук идёт именно от вас. -->
        <span class="lvl" class:live={call.speakingSelf}>
          <i style="transform: scaleX({call.micMuted ? 0 : call.level})"></i>
        </span>
      </span>
      <span class="cap">{call.micMuted ? 'Без звука' : 'Микрофон'}</span>
    </button>
    <button class="ctl" aria-pressed={call.camOn} onclick={() => call.toggleCamera()}>
      <span class="ic"><Icon name={call.camOn ? 'camera' : 'camera-off'} size={17} /></span>
      <span class="cap">Камера</span>
    </button>
    <button class="ctl" aria-pressed={call.screenOn} onclick={() => call.toggleScreen()}>
      <span class="ic"><Icon name="screen" size={17} /></span>
      <span class="cap">{call.screenOn ? 'Идёт показ' : 'Экран'}</span>
    </button>
    {#if call.screenOn && call.screenAudioAvailable}
      <button class="ctl" aria-pressed={call.screenAudio} onclick={() => call.toggleScreenAudio()}>
        <span class="ic"><Icon name="speaker" size={17} /></span>
        <span class="cap">Звук экрана</span>
      </button>
    {/if}

    <span class="sep" aria-hidden="true"></span>

    <!-- У тех, кто в сети, зазвонит звонок с кнопкой «подключиться». -->
    <button class="ctl" onclick={async () => (call.status = await call.ring())}>
      <span class="ic"><Icon name="bell" size={17} /></span>
      <span class="cap">Позвать</span>
    </button>
    {#if SHARE_PLAYER}
      <button
        class="ctl"
        aria-pressed={call.sharingMusic}
        onclick={() => (call.sharingMusic ? call.stopMusic() : call.toggleMusicPicker())}
      >
        <span class="ic"><Icon name="speaker" size={17} /></span>
        <span class="cap">Плеер</span>
      </button>
    {/if}
    {#if hasScreen && call.expanded}
      <button class="ctl" aria-pressed={call.screenOnly} onclick={() => call.toggleScreenOnly()}>
        <span class="ic"><Icon name="screen" size={17} /></span>
        <span class="cap">Только экран</span>
      </button>
      <button class="ctl" aria-pressed={call.fullscreen} onclick={() => call.toggleFullscreen()}>
        <span class="ic"><Icon name="expand" size={17} /></span>
        <span class="cap">{call.fullscreen ? 'Из полного' : 'Весь экран'}</span>
      </button>
    {/if}
    <button class="ctl" onclick={() => call.toggleExpanded()} title={call.expanded ? 'Esc — свернуть' : undefined}>
      <span class="ic"><Icon name={call.expanded ? 'collapse' : 'expand'} size={17} /></span>
      <span class="cap">{call.expanded ? 'Свернуть' : 'Развернуть'}</span>
    </button>
    <button class="hangup" onclick={() => call.leave()}>
      <Icon name="hangup" size={17} />
      <span>Выйти</span>
    </button>
  </div>
</div>

{#if linksOpen}
  <CallLinks onclose={() => (linksOpen = false)} />
{/if}

<style>
  .bar {
    --q-good: #7bd393;
    --q-fair: #e6bd5c;
    --q-bad: #f07467;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 10px 16px;
    padding: 10px 14px;
    border-bottom: 1px solid var(--line);
  }

  .where {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }
  .live {
    flex: none;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--q-good);
    box-shadow: 0 0 0 0 color-mix(in srgb, var(--q-good) 60%, transparent);
    animation: pulse 2s ease-out infinite;
  }
  @keyframes pulse {
    to {
      box-shadow: 0 0 0 7px transparent;
    }
  }
  .where-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
    line-height: 1.25;
  }
  .where-text b {
    color: var(--fg-hi);
    font-weight: 640;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .where-text span {
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
  }

  .net {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-left: 4px;
    padding: 4px 10px;
    border: 1px solid var(--line);
    border-radius: 999px;
    color: var(--fg-dim);
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    transition: border-color var(--fast) var(--ease), color var(--fast) var(--ease);
  }
  .net:hover {
    border-color: var(--fg-dim);
    color: var(--fg-hi);
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

  .controls {
    display: flex;
    flex-wrap: wrap;
    align-items: stretch;
    gap: 4px;
  }
  .sep {
    width: 1px;
    margin: 4px 6px;
    background: var(--line);
  }

  .ctl {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 3px;
    min-width: 62px;
    padding: 6px 6px 5px;
    border: 1px solid transparent;
    border-radius: 10px;
    color: var(--fg-dim);
    transition: background var(--fast) var(--ease), color var(--fast) var(--ease), border-color var(--fast) var(--ease);
  }
  .ctl:hover {
    background: var(--lift, var(--bg));
    color: var(--fg-hi);
  }
  .ctl:focus-visible,
  .net:focus-visible,
  .hangup:focus-visible {
    outline: 2px solid var(--edge);
    outline-offset: 1px;
  }
  .ic {
    position: relative;
    display: grid;
    place-items: center;
    width: 34px;
    height: 28px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--bg);
  }
  .cap {
    font-size: 11px;
    white-space: nowrap;
  }
  /* Включённое — инверсией значка */
  .ctl[aria-pressed='true'] .ic {
    background: var(--inv-bg);
    border-color: var(--inv-bg);
    color: var(--inv-fg);
  }
  .ctl[aria-pressed='true'] {
    color: var(--fg-hi);
  }
  /* Микрофон выключен — это предупреждение: говорите, а вас не слышно */
  .ctl.alert .ic {
    border-color: color-mix(in srgb, var(--q-bad) 60%, var(--line));
    color: var(--q-bad);
  }
  .ctl.alert .cap {
    color: var(--q-bad);
  }

  /* Уровень своего голоса — тонкой полоской под значком микрофона */
  .lvl {
    position: absolute;
    left: 5px;
    right: 5px;
    bottom: 3px;
    height: 2px;
    border-radius: 2px;
    overflow: hidden;
    background: color-mix(in srgb, currentColor 15%, transparent);
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

  .hangup {
    display: inline-flex;
    align-items: center;
    align-self: center;
    gap: 8px;
    margin-left: 6px;
    padding: 9px 14px;
    border: 1px solid color-mix(in srgb, var(--q-bad) 70%, transparent);
    border-radius: 10px;
    background: color-mix(in srgb, var(--q-bad) 16%, transparent);
    color: var(--fg-hi);
    font-weight: 620;
    transition: background var(--fast) var(--ease);
  }
  .hangup:hover {
    background: color-mix(in srgb, var(--q-bad) 32%, transparent);
  }

  @media (prefers-reduced-motion: reduce) {
    .live {
      animation: none;
    }
  }
</style>
