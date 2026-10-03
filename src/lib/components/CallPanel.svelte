<script lang="ts">
  import { call, SHARE_PLAYER } from '../stores/call.svelte';
  import PlayerPanel from './PlayerPanel.svelte';
  import { prefs } from '../stores/prefs.svelte';
  import { session } from '../stores/session.svelte';
  import { previewUrl } from '../previews';

  let selfVideo: HTMLVideoElement | undefined = $state();
  let selfScreen: HTMLVideoElement | undefined = $state();
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
    if (selfScreen && call.screenStream) {
      selfScreen.srcObject = call.screenStream;
      void selfScreen.play().catch(() => undefined);
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
  /** Своя демонстрация идёт маленьким превью среди лиц: большой она не нужна,
   *  а на весь экран превращалась бы в бесконечное зеркало самой себя. */
  const showOwnScreen = $derived(call.screenOn && call.screenStream !== null);
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
      {#if prefs.adequate}
        <b>Идёт разговор</b>
        <span class="count">
          {session.channels.find((c) => c.id === call.channel)?.name ?? 'звонок'} ·
          {call.participants.length} в комнате
        </span>
      {:else}
        <b>{session.channels.find((c) => c.id === call.channel)?.name ?? 'звонок'}</b>
        <span class="count">{call.participants.length} в комнате</span>
      {/if}
      <span class="sp"></span>
      <button class:off={call.micMuted} onclick={() => call.toggleMic()}>
        {#if prefs.adequate}
          {call.micMuted ? 'Включить микрофон' : 'Микрофон'}
        {:else}
          {call.micMuted ? 'микрофон выкл' : 'микрофон вкл'}
        {/if}
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
        {#if prefs.adequate}
          {call.camOn ? 'Камера' : 'Включить камеру'}
        {:else}
          {call.camOn ? 'камера вкл' : 'камера выкл'}
        {/if}
      </button>
      <button class:off={!call.screenOn} onclick={() => call.toggleScreen()}>
        {#if prefs.adequate}
          {call.screenOn ? 'Экран' : 'Показать экран'}
        {:else}
          {call.screenOn ? 'экран вкл' : 'экран выкл'}
        {/if}
      </button>
      {#if call.screenOn && call.screenAudioAvailable}
        <button class:off={!call.screenAudio} onclick={() => call.toggleScreenAudio()}>
          {call.screenAudio ? 'звук экрана вкл' : 'звук экрана выкл'}
        </button>
      {/if}
      <!-- Позвать друзей: у тех, кто в сети, зазвонит звонок с кнопкой
           «подключиться». Сервера, который придержал бы зов для тех, кто не в
           сети, у нас нет — поэтому и число позванных честное. -->
      <button onclick={async () => (call.status = await call.ring())}>
        {prefs.adequate ? 'Позвать всех' : 'позвать всех'}
      </button>
      {#if SHARE_PLAYER}
        <button
          class:off={!call.sharingMusic}
          onclick={() => (call.sharingMusic ? call.stopMusic() : call.toggleMusicPicker())}
        >
          {call.sharingMusic ? 'плеер вкл' : 'слушать вместе'}
        </button>
      {/if}
      {#if hasScreen && call.expanded}
        <!-- В свёрнутом звонке эти кнопки живут на самой демонстрации: в шапке
             они переносили её на две строки и терялись среди остальных. -->
        <button aria-pressed={call.screenOnly} onclick={() => call.toggleScreenOnly()}>
          {prefs.adequate ? 'Только экран' : 'только экран'}
        </button>
        <button onclick={() => call.toggleFullscreen()}>
          {#if prefs.adequate}
            {call.fullscreen ? 'Из полного экрана' : 'Во весь экран'}
          {:else}
            {call.fullscreen ? 'из полного экрана' : 'во весь экран'}
          {/if}
        </button>
      {/if}
      <button onclick={() => call.toggleExpanded()}>
        {#if prefs.adequate}
          {call.expanded ? 'Свернуть' : 'Развернуть'}
        {:else}
          {call.expanded ? 'свернуть' : 'развернуть'}
        {/if}
      </button>
      {#if call.expanded && !call.fullscreen}<span class="tip">Esc — свернуть</span>{/if}
      <button class="leave" onclick={() => call.leave()}>
        {prefs.adequate ? 'Выйти' : 'выйти [^E]'}
      </button>
    </header>

    <PlayerPanel />

    <div
      class="grid"
      class:color={prefs.colorVideo}
      class:with-screen={hasScreen}
      class:only-screen={call.screenOnly && hasScreen}
    >
      {#if hasScreen}
        <!-- Демонстрация — отдельной сценой, целиком и по центру. Раньше она
             была плиткой в общей сетке с потолком по высоте и прокруткой
             внутри: в свёрнутом звонке её низ уезжал под ленту, а сама она
             прижималась к левому краю. -->
        <div class="stage" class:split={call.screens.length > 1}>
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
              <!-- Кнопки прямо на демонстрации: разворачивают чужой экран,
                   глядя на сам экран. -->
              <div class="tools">
                <button onclick={() => call.toggleScreenOnly()}>
                  {call.screenOnly ? 'показать всех' : 'только экран'}
                </button>
                <button onclick={() => call.toggleFullscreen()}>
                  {#if prefs.adequate}
                    {call.fullscreen ? 'Из полного экрана' : 'Во весь экран'}
                  {:else}
                    {call.fullscreen ? 'из полного экрана' : 'во весь экран'}
                  {/if}
                </button>
              </div>
              <figcaption>{nick(sharer)} · экран · двойной щелчок — во весь экран</figcaption>
            </figure>
          {/each}
        </div>
      {/if}

      <div class="people">
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

        {#if showOwnScreen}
          <figure class="tile own-screen">
            <!-- svelte-ignore a11y_media_has_caption -->
            <video bind:this={selfScreen} muted playsinline autoplay></video>
            <figcaption>ваш экран · идёт показ</figcaption>
          </figure>
        {/if}

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

        {#if others.length === 0 && !hasScreen}
          <p class="empty">
            никого больше нет. позовите: <b>^I</b> — ссылка в пространство,
            <b>/визитка</b> — ссылка для связи один на один
          </p>
        {/if}
      </div>
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
    min-height: 0;
    padding: var(--gap-4);
  }
  /* Развёрнутый звонок без демонстрации — это лица во весь экран. */
  .call.expanded .grid:not(.with-screen) .people {
    flex: 1;
    grid-template-columns: repeat(auto-fit, minmax(min(28rem, 46%), 1fr));
    align-content: center;
    gap: var(--gap-4);
    max-height: none;
  }
  /* А с демонстрацией — она и есть главное: сцена забирает всё место, лица
     уходят полоской вниз. */
  .call.expanded .grid.with-screen .stage {
    flex: 1;
    min-height: 0;
  }
  .call.expanded .grid.with-screen .stage .tile.wide {
    height: 100%;
    max-height: none;
  }
  .call.expanded .grid.with-screen .people .tile {
    width: 176px;
  }
  .grid.only-screen .people {
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
    height: 100vh;
    padding: 0;
    gap: 0;
  }
  .call.fullscreen .stage {
    gap: 0;
  }
  .call.fullscreen .grid.only-screen .tile.wide {
    height: 100vh;
    border: none;
    border-radius: 0;
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

  /* Кнопок много, а колонка звонка бывает узкой: пусть шапка переносится
     целыми кнопками, а не рвёт подпись «микрофон вкл» пополам. */
  header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px var(--gap-4);
    padding: 5px var(--gap-4);
    font-size: var(--text-sm);
    border-bottom: 1px solid var(--line);
  }
  header > * {
    white-space: nowrap;
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
  /* Режим, который включают и выключают, — инверсией, а не зачёркиванием:
     зачёркнутое «только экран» читалось как сломанная кнопка. */
  header button[aria-pressed='true'] {
    background: var(--fg);
    border-color: var(--fg);
    color: var(--bg);
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
    display: flex;
    flex-direction: column;
    gap: var(--gap-3);
    padding: var(--gap-3) var(--gap-4);
  }

  .people {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(148px, 1fr));
    gap: var(--gap-3);
    max-height: 42vh;
    overflow-y: auto;
  }

  /* Сцена с демонстрацией: экран целиком и по центру. Высота задана явно, а
     не выводится из пропорций: так плитка не прижимается к краю и не уезжает
     под ленту, а картинка внутри вписывается без обрезки. */
  .stage {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--gap-3);
  }
  .stage.split {
    grid-template-columns: repeat(auto-fit, minmax(min(100%, 22rem), 1fr));
  }
  .stage .tile.wide {
    width: 100%;
    height: clamp(200px, 40vh, 560px);
    aspect-ratio: auto;
    background: #000;
  }

  /* С демонстрацией лица уходят полоской под неё: они здесь затем, чтобы
     видеть, кто говорит, а не чтобы их разглядывать. */
  .grid.with-screen .people {
    display: flex;
    gap: var(--gap-3);
    overflow-x: auto;
    overflow-y: hidden;
    max-height: none;
  }
  .grid.with-screen .people .tile {
    flex: 0 0 auto;
    width: 132px;
    aspect-ratio: 16 / 10;
  }
  .grid.with-screen .people figcaption {
    font-size: 10px;
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
  /* Чужой экран нельзя обрезать под пропорции плитки: смысл демонстрации в
     том, чтобы прочитать, что на ней, а `cover` срезает края — как раз там,
     где панели и меню. Своё превью — так же. */
  .tile.wide canvas,
  .tile.own-screen video {
    object-fit: contain;
  }
  .tile.own-screen video {
    filter: none;
    background: #000;
  }
  /* Подпись поверх чужого экрана закрывала его нижнюю строку — ту самую, где
     у редактора курсор, а у терминала приглашение. Появляется по наведению. */
  .tile.wide figcaption {
    opacity: 0;
    transition: opacity var(--fast) var(--ease);
  }
  .tile.wide:hover figcaption,
  .tile.wide:focus-within figcaption {
    opacity: 1;
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
