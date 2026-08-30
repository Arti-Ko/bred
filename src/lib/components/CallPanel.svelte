<script lang="ts">
  import { call } from '../stores/call.svelte';
  import PlayerPanel from './PlayerPanel.svelte';
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
    if (event.key !== 'Escape') return;
    // Полный экран снимается первым: он «глубже» развёрнутого, и один Escape
    // должен возвращать на один шаг, а не сразу к ленте.
    if (call.fullscreen) void call.exitFullscreen();
    else if (call.expanded) call.toggleExpanded();
  }}
/>

{#if call.active}
  <section
    class="call"
    class:expanded={call.expanded}
    class:fullscreen={call.fullscreen}
    aria-label="Звонок"
  >
    <header>
      <span class="dot blink">◉</span>
      <b>{session.channels.find((c) => c.id === call.channel)?.name ?? 'звонок'}</b>
      <span class="count">{call.participants.length} в комнате</span>
      <span class="sp"></span>
      <button class:off={call.micMuted} onclick={() => call.toggleMic()}>
        {call.micMuted ? 'микрофон выкл' : 'микрофон вкл'}
      </button>
      <!-- Своего голоса в звонке не слышно: эхоподавление на то и стоит.
           Полоска рядом с кнопкой — единственный способ увидеть, что звук
           идёт именно от тебя, и увидеть его до того, как переспросят. -->
      <span
        class="meter"
        class:live={call.speakingSelf}
        title={call.micMuted ? 'микрофон выключен' : 'вас слышно'}
        aria-hidden="true"
      >
        <i style="transform: scaleX({call.micMuted ? 0 : call.level})"></i>
      </span>
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
      <button
        class:off={!call.sharingMusic}
        onclick={() => (call.sharingMusic ? call.stopMusic() : call.toggleMusicPicker())}
      >
        {call.sharingMusic ? 'плеер вкл' : 'слушать вместе'}
      </button>
      {#if hasScreen}
        <button class:off={!call.screenOnly} onclick={() => call.toggleScreenOnly()}>
          только экран
        </button>
        <button onclick={() => call.toggleFullscreen()}>
          {call.fullscreen ? 'из полного экрана' : 'во весь экран'}
        </button>
      {/if}
      <button onclick={() => call.toggleExpanded()}>
        {call.expanded ? 'свернуть' : 'развернуть'}
      </button>
      {#if call.expanded && !call.fullscreen}<span class="tip">Esc — свернуть</span>{/if}
      <button class="leave" onclick={() => call.leave()}>выйти [^E]</button>
    </header>

    <PlayerPanel />

    <div
      class="grid"
      class:color={prefs.colorVideo}
      class:with-screen={hasScreen}
      class:only-screen={call.screenOnly && hasScreen}
    >
      <figure class="tile self" class:speaking={call.speakingSelf}>
        <!-- svelte-ignore a11y_media_has_caption -->
        <video bind:this={selfVideo} muted playsinline autoplay class:hidden={!call.camOn}></video>
        {#if !call.camOn}
          {#if face(session.me)}
            <img class="face" src={face(session.me)} alt="" />
          {:else}
            <div class="placeholder">{session.nick.slice(0, 2)}</div>
          {/if}
        {/if}
        <figcaption>
          {session.nick} · вы{call.micMuted
            ? ' · без звука'
            : call.speakingSelf
              ? ' · говорите'
              : ''}
        </figcaption>
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
        <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
        <figure class="tile wide" ondblclick={() => call.toggleFullscreen()}>
          <div class="placeholder">экран</div>
          <canvas
            {@attach (node) => {
              call.attachCanvas(sharer, node as HTMLCanvasElement, 'screen');
              return () => call.attachCanvas(sharer, null, 'screen');
            }}
          ></canvas>
          <!-- Кнопки прямо на демонстрации: в шапке они теряются среди восьми
               других, а разворачивают чужой экран, глядя на сам экран. -->
          <div class="tools">
            <button onclick={() => call.toggleScreenOnly()}>
              {call.screenOnly ? 'показать всех' : 'только экран'}
            </button>
            <button onclick={() => call.toggleFullscreen()}>
              {call.fullscreen ? 'из полного экрана' : 'во весь экран'}
            </button>
          </div>
          <figcaption>{nick(sharer)} · экран · двойной щелчок — во весь экран</figcaption>
        </figure>
      {/each}

      {#if others.length === 0}
        <p class="empty">
          никого больше нет. позовите: <b>^I</b> — ссылка в пространство,
          <b>/визитка</b> — ссылка для связи один на один
        </p>
      {/if}
    </div>

    {#if call.fullscreen}
      <!-- Подсказка живёт пару секунд и уходит: постоянная надпись поверх
           чужого экрана закрывает как раз ту строчку, ради которой смотрят. -->
      <span class="fs-hint">Esc — выйти · панель вверху экрана</span>
    {/if}

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
  /* Развёрнутый звонок без демонстрации — это лица во весь экран. */
  .call.expanded .grid {
    grid-template-columns: repeat(auto-fit, minmax(min(28rem, 46%), 1fr));
    align-content: center;
    gap: var(--gap-4);
    padding: var(--gap-4);
  }

  /* А с демонстрацией — она и есть главное. Раньше здесь стоял `display: none`
     на плитке экрана: развернуть можно было только камеры, а то единственное,
     ради чего звонок разворачивают, из развёрнутого вида пропадало. */
  .call.expanded .grid.with-screen {
    grid-template-columns: repeat(auto-fit, minmax(min(11rem, 20%), 1fr));
    align-content: start;
    gap: var(--gap-3);
  }
  .call.expanded .grid.with-screen .tile.wide {
    /* Первым в потоке, хотя в разметке экран идёт после камер: смотрят на него. */
    order: -1;
    grid-column: 1 / -1;
    height: min(78vh, calc(100vh - 11rem));
    /* Потолок из свёрнутой ленты здесь только мешает: он и держал бы экран
       на трети окна ровно тогда, когда его развернули. */
    max-height: none;
    aspect-ratio: auto;
  }
  /* Лица под демонстрацией — полосой: они здесь для того, чтобы видеть, кто
     кивает, а не чтобы разглядывать. */
  .call.expanded .grid.with-screen .tile:not(.wide) {
    aspect-ratio: 16 / 9;
    max-height: 15vh;
  }
  .call.expanded .grid.only-screen .tile.wide {
    height: 88vh;
  }
  .grid.only-screen .tile:not(.wide) {
    display: none;
  }

  /* Полный экран — это уже не «звонок на всё окно»: окно ушло в полноэкранный
     режим средствами системы, и всё, что не чужой экран, обязано уйти с
     дороги. Фон чёрный, а не по теме: вокруг картинки должно быть ничто, а не
     «наш тёмный». */
  .call.fullscreen {
    position: fixed;
    inset: 0;
    z-index: 90;
    background: #000;
  }
  .call.fullscreen .grid {
    padding: 0;
    gap: 0;
    align-content: stretch;
  }
  .call.fullscreen .grid.only-screen .tile.wide {
    height: 100vh;
    border: none;
    background: #000;
  }
  /* Панель не занимает место постоянно: всплывает, когда курсор подводят к
     верхней кромке, и не мешает, пока не нужна. Прозрачная она остаётся
     кликабельной, поэтому наведение работает и на невидимой. */
  .call.fullscreen header {
    position: absolute;
    inset: 0 0 auto 0;
    z-index: 3;
    background: var(--bg);
    border-bottom: 1px solid var(--line);
    opacity: 0;
    transition: opacity var(--fast) var(--ease);
  }
  .call.fullscreen header:hover,
  .call.fullscreen header:focus-within {
    opacity: 1;
  }
  /* Имя показывающего — тоже помеха поверх текста. Появляется по наведению. */
  .call.fullscreen figcaption {
    opacity: 0;
    transition: opacity var(--fast) var(--ease);
  }
  .call.fullscreen .tile:hover figcaption {
    opacity: 1;
  }

  .fs-hint {
    position: absolute;
    right: var(--gap-4);
    bottom: var(--gap-4);
    z-index: 4;
    padding: 0.2rem 0.6rem;
    background: var(--bg);
    border: 1px solid var(--line);
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
    animation: fs-hint-out var(--fast) var(--ease) 2.6s forwards;
  }
  @keyframes fs-hint-out {
    to {
      opacity: 0;
      visibility: hidden;
    }
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
  .count,
  .tip {
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
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

  /* Полоска собственного голоса. Двигается transform'ом, а не шириной: она
     обновляется несколько раз в секунду всё время звонка, и раскладка при
     каждом обновлении — это работа на ровном месте. */
  .meter {
    position: relative;
    width: 44px;
    height: 6px;
    border: 1px solid var(--fg-faint);
    overflow: hidden;
  }
  .meter i {
    position: absolute;
    inset: 0;
    background: var(--fg-dimmer);
    transform-origin: left center;
    transform: scaleX(0);
    transition: transform var(--fast) linear;
  }
  .meter.live i {
    background: var(--fg-hi);
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
  /* Демонстрация экрана занимает всю строку: мелкий текст иначе не прочесть.
     Даже в свёрнутом виде — это предпросмотр, но по нему решают, разворачивать
     или нет, а по плитке в две колонки не решишь ничего. */
  .tile.wide {
    grid-column: 1 / -1;
    aspect-ratio: 16 / 9;
    max-height: 34vh;
  }
  /* Чужой экран нельзя обрезать под пропорции плитки: смысл демонстрации в
     том, чтобы прочитать, что на ней, а `cover` срезает края — как раз там,
     где панели и меню. */
  .tile.wide canvas {
    object-fit: contain;
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

  /* Появляются по наведению: постоянные кнопки поверх чужого экрана закрывают
     ровно ту строчку, ради которой на него и смотрят. */
  .tools {
    position: absolute;
    top: 0;
    right: 0;
    z-index: 3;
    display: flex;
    gap: 1px;
    opacity: 0;
    transition: opacity var(--fast) var(--ease);
  }
  .tile.wide:hover .tools,
  .tools:focus-within {
    opacity: 1;
  }
  .tools button {
    padding: 2px 8px;
    background: var(--inv-bg);
    color: var(--inv-fg);
    font-size: var(--text-xs);
  }
  .tools button:hover {
    text-decoration: underline;
    text-underline-offset: 2px;
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
