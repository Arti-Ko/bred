<script lang="ts">
  import { call } from '../stores/call.svelte';
  import { prefs } from '../stores/prefs.svelte';
  import { session } from '../stores/session.svelte';
  import { previewUrl } from '../previews';

  let selfVideo: HTMLVideoElement | undefined = $state();
  /** Чья громкость настраивается: открывается правым кликом по плитке. */
  let tuning = $state<string | null>(null);
  /** Картинки профилей — их показываем вместо инициалов. */
  let faces = $state<Record<string, string>>({});
  const requested = new Set<string>();

  $effect(() => {
    if (selfVideo && call.stream) {
      selfVideo.srcObject = call.stream;
      void selfVideo.play().catch(() => undefined);
    }
  });

  $effect(() => {
    for (const member of session.members) {
      // Множество вне реактивности: иначе эффект будил бы сам себя.
      if (member.avatar && !requested.has(member.avatar)) {
        const hash = member.avatar;
        requested.add(hash);
        void previewUrl(hash, session.spaceId).then((url) => {
          if (url) faces = { ...faces, [hash]: url };
        });
      }
    }
  });

  function nick(id: string): string {
    return session.members.find((m) => m.id === id)?.nick ?? id.slice(0, 8);
  }

  function face(id: string): string | null {
    const hash = session.members.find((m) => m.id === id)?.avatar;
    return hash ? (faces[hash] ?? null) : null;
  }

  const others = $derived(call.participants.filter((p) => p.id !== session.me));
  const hasScreen = $derived(call.screens.length > 0);
</script>

<svelte:window
  onkeydown={(event) => {
    if (event.key === 'Escape' && call.expanded) call.toggleExpanded();
  }}
/>

{#if call.active}
  <section class="call" class:expanded={call.expanded} aria-label="Звонок">
    <header>
      <span class="dot blink">◉</span>
      <b>{session.channels.find((c) => c.id === call.channel)?.name ?? 'звонок'}</b>
      <span class="count">{call.participants.length} в комнате</span>
      <span class="sp"></span>
      <button class:off={call.micMuted} onclick={() => call.toggleMic()}>
        {call.micMuted ? 'микрофон выкл' : 'микрофон вкл'}
      </button>
      <button class:off={!call.camOn} onclick={() => call.toggleCamera()}>
        {call.camOn ? 'камера вкл' : 'камера выкл'}
      </button>
      <button class:off={!call.screenOn} onclick={() => call.toggleScreen()}>
        {call.screenOn ? 'экран вкл' : 'экран выкл'}
      </button>
      {#if call.screenOn && call.screenAudioAvailable}
        <button class:off={!call.screenAudio} onclick={() => call.toggleScreenAudio()}>
          {call.screenAudio ? 'звук экрана вкл' : 'звук экрана выкл'}
        </button>
      {/if}
      {#if hasScreen}
        <button class:off={!call.screenOnly} onclick={() => call.toggleScreenOnly()}>
          только экран
        </button>
      {/if}
      <button onclick={() => call.toggleExpanded()}>
        {call.expanded ? 'свернуть' : 'развернуть'}
      </button>
      <button class="leave" onclick={() => call.leave()}>выйти [^E]</button>
    </header>

    <div
      class="grid"
      class:color={prefs.colorVideo}
      class:with-screen={hasScreen}
      class:only-screen={call.screenOnly && hasScreen}
    >
      <figure class="tile self">
        <!-- svelte-ignore a11y_media_has_caption -->
        <video bind:this={selfVideo} muted playsinline autoplay class:hidden={!call.camOn}></video>
        {#if !call.camOn}
          {#if face(session.me)}
            <img class="face" src={face(session.me)} alt="" />
          {:else}
            <div class="placeholder">{session.nick.slice(0, 2)}</div>
          {/if}
        {/if}
        <figcaption>{session.nick} · вы{call.micMuted ? ' · без звука' : ''}</figcaption>
      </figure>

      {#each others as participant (participant.id)}
        <figure
          class="tile"
          class:speaking={call.speaking(participant.id)}
          oncontextmenu={(event) => {
            // Правый клик по человеку — его громкость. Общего регулятора мало:
            // в одном звонке один шепчет, другой перекрикивает.
            event.preventDefault();
            tuning = tuning === participant.id ? null : participant.id;
          }}
        >
          {#if face(participant.id)}
            <img class="face" src={face(participant.id)} alt="" />
          {:else}
            <div class="placeholder">{nick(participant.id).slice(0, 2)}</div>
          {/if}
          <canvas
            {@attach (node) => {
              call.attachCanvas(participant.id, node as HTMLCanvasElement, 'video');
              return () => call.attachCanvas(participant.id, null, 'video');
            }}
          ></canvas>
          {#if tuning === participant.id}
            <div class="volume">
              <label>
                громкость
                <input
                  type="range"
                  min="0"
                  max="4"
                  step="0.05"
                  value={call.volume(participant.id)}
                  oninput={(event) =>
                    call.setVolume(participant.id, Number(event.currentTarget.value))}
                />
              </label>
              <div class="volume-row">
                <span>{Math.round(call.volume(participant.id) * 100)}%</span>
                <button onclick={() => call.setVolume(participant.id, 1)}>сбросить</button>
                <button onclick={() => (tuning = null)}>закрыть</button>
              </div>
            </div>
          {/if}

          <figcaption>
            {nick(participant.id)}{call.volume(participant.id) !== 1
              ? ` · ${Math.round(call.volume(participant.id) * 100)}%`
              : ''}
          </figcaption>
        </figure>
      {/each}

      <!-- Экран идёт отдельной плиткой и шире: на него, как правило, и смотрят -->
      {#each call.screens as sharer (sharer)}
        <figure class="tile wide">
          <div class="placeholder">экран</div>
          <canvas
            {@attach (node) => {
              call.attachCanvas(sharer, node as HTMLCanvasElement, 'screen');
              return () => call.attachCanvas(sharer, null, 'screen');
            }}
          ></canvas>
          <figcaption>{nick(sharer)} · экран</figcaption>
        </figure>
      {/each}

      {#if others.length === 0}
        <p class="empty">
          никого больше нет. позовите: <b>^I</b> — ссылка в пространство,
          <b>/визитка</b> — ссылка для связи один на один
        </p>
      {/if}
    </div>

    {#if call.status}
      <div class="err">{call.status}</div>
    {/if}
  </section>
{/if}

<style>
  .call {
    border-bottom: 1px solid var(--line);
    background: var(--bg-raised);
  }

  /* Развёрнутый звонок закрывает окно целиком: когда смотрят демонстрацию,
     лента только отнимает место. */
  .call.expanded {
    position: fixed;
    inset: 0;
    z-index: 70;
    display: flex;
    flex-direction: column;
    background: var(--bg);
  }
  .call.expanded .grid {
    flex: 1;
    max-height: none;
    align-content: start;
  }
  /* Демонстрация занимает основную часть, камеры уходят полосой вниз */
  .call.expanded .grid.with-screen .tile.wide {
    grid-column: 1 / -1;
    height: 78vh;
    aspect-ratio: auto;
  }
  .grid.only-screen .tile:not(.wide) {
    display: none;
  }

  header {
    display: flex;
    align-items: center;
    gap: var(--gap-4);
    padding: 5px var(--gap-4);
    font-size: var(--text-sm);
    border-bottom: 1px solid var(--line);
  }
  header b {
    color: var(--fg-hi);
  }
  .dot {
    color: var(--fg-hi);
  }
  .count {
    color: var(--fg-dimmer);
  }
  .sp {
    flex: 1;
  }

  header button {
    border: 1px solid var(--fg-faint);
    color: var(--fg-dim);
    padding: 1px 9px;
    font-size: var(--text-xs);
    transition: color var(--fast) var(--ease), border-color var(--fast) var(--ease);
  }
  header button:hover {
    color: var(--fg-hi);
    border-color: var(--fg-dim);
  }
  /* Выключенное состояние — зачёркнуто, а не «серое»: цвета в системе нет */
  header button.off {
    color: var(--fg-faint);
    text-decoration: line-through;
  }
  header button.leave {
    background: var(--inv-bg);
    border-color: var(--inv-bg);
    color: var(--inv-fg);
    font-weight: 700;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(148px, 1fr));
    gap: var(--gap-3);
    padding: var(--gap-3) var(--gap-4);
    max-height: 42vh;
    overflow-y: auto;
  }

  .tile {
    position: relative;
    margin: 0;
    aspect-ratio: 4 / 3;
    border: 1px solid var(--line);
    background: var(--bg);
    overflow: hidden;
  }
  /* Говорящего выделяем рамкой — единственный доступный акцент */
  .tile.speaking {
    border-color: var(--fg);
  }
  /* Демонстрация экрана занимает две колонки: мелкий текст иначе не прочесть */
  .tile.wide {
    grid-column: span 2;
    aspect-ratio: 16 / 9;
  }

  .tile video,
  .tile canvas {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    z-index: 1;
    /* Монохром по умолчанию — но переключаемо в настройках */
    filter: grayscale(1) contrast(1.05);
  }
  .grid.color .tile video,
  .grid.color .tile canvas,
  .grid.color .face {
    filter: none;
  }

  /* Аватар вместо инициалов, когда камера выключена */
  .face {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    filter: grayscale(1) contrast(1.05);
  }

  .volume {
    position: absolute;
    inset: auto 0 0 0;
    z-index: 3;
    padding: var(--gap-3);
    background: var(--bg-raised);
    border-top: 1px solid var(--fg-faint);
    font-size: var(--text-xs);
    color: var(--fg-dim);
  }
  .volume label {
    display: block;
  }
  .volume input {
    width: 100%;
    margin-top: 4px;
    accent-color: var(--fg);
  }
  .volume-row {
    display: flex;
    align-items: center;
    gap: var(--gap-3);
    margin-top: 4px;
  }
  .volume-row span {
    color: var(--fg-hi);
  }
  .volume-row button {
    color: var(--fg-dimmer);
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .volume-row button:hover {
    color: var(--fg-hi);
  }
  .tile.self video {
    transform: scaleX(-1);
  }
  /* Экран — исключение из монохрома: на нём читают текст и код */
  .tile.wide canvas {
    filter: none;
    object-fit: contain;
  }
  .tile video.hidden {
    display: none;
  }

  .placeholder {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    color: var(--fg-faint);
    font-size: 22px;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }

  figcaption {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    z-index: 2;
    padding: 2px 6px;
    background: var(--inv-bg);
    color: var(--inv-fg);
    font-size: var(--text-xs);
    font-weight: 700;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .empty {
    grid-column: 1 / -1;
    margin: 0;
    padding: var(--gap-4);
    color: var(--fg-dimmer);
    font-size: var(--text-sm);
  }
  .empty b {
    color: var(--fg);
    font-weight: 400;
  }

  .err {
    padding: 4px var(--gap-4);
    border-top: 1px solid var(--line);
    color: var(--fg-hi);
    font-size: var(--text-sm);
    font-weight: 700;
  }
</style>
