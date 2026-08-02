<script lang="ts">
  import { call } from '../stores/call.svelte';
  import { session } from '../stores/session.svelte';

  let selfVideo: HTMLVideoElement | undefined = $state();

  $effect(() => {
    if (selfVideo && call.localStream) {
      selfVideo.srcObject = call.localStream;
      void selfVideo.play().catch(() => undefined);
    }
  });

  function nick(id: string): string {
    return session.members.find((m) => m.id === id)?.nick ?? id.slice(0, 8);
  }

  const others = $derived(call.participants.filter((p) => p.id !== session.me));
</script>

{#if call.active}
  <section class="call" aria-label="Звонок">
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
      <button class="leave" onclick={() => call.leave()}>выйти [^E]</button>
    </header>

    <div class="grid">
      <figure class="tile self">
        <!-- svelte-ignore a11y_media_has_caption -->
        <video bind:this={selfVideo} muted playsinline class:hidden={!call.camOn}></video>
        {#if !call.camOn}<div class="placeholder">{session.nick.slice(0, 2)}</div>{/if}
        <figcaption>{session.nick} · вы{call.micMuted ? ' · без звука' : ''}</figcaption>
      </figure>

      {#each others as participant (participant.id)}
        <figure class="tile" class:speaking={call.speaking(participant.id)}>
          <div class="placeholder">{nick(participant.id).slice(0, 2)}</div>
          <canvas
            {@attach (node) => {
              call.attachCanvas(participant.id, node as HTMLCanvasElement, 'video');
              return () => call.attachCanvas(participant.id, null, 'video');
            }}
          ></canvas>
          <figcaption>{nick(participant.id)}</figcaption>
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
        <p class="empty">никого больше нет. позовите: <b>^I</b> копирует ссылку</p>
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
    /* Монохром распространяется и на видео: иначе картинка выпадает из системы */
    filter: grayscale(1) contrast(1.05);
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
