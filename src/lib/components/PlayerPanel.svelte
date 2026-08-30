<script lang="ts">
  // Общий плеер комнаты: пульт от чужого магнитофона.
  //
  // Синхронизировать здесь нечего. Звук идёт живым потоком от ведущего, и
  // пауза — не общий таймлайн, а нажатие, которое доезжает до самого
  // приложения-источника. Поэтому кнопки одинаковые у всех, а громкость —
  // у каждого своя: она единственное, что имеет смысл держать локально.

  import { call } from '../stores/call.svelte';
  import { session } from '../stores/session.svelte';

  function nick(id: string): string {
    return session.members.find((m) => m.id === id)?.nick ?? id.slice(0, 8);
  }

  const host = $derived(call.player ? nick(call.player.host) : '');
  const mine = $derived(call.player?.host === session.me);
</script>

{#if call.playerHere && call.player}
  <div class="player" aria-label="Общий плеер">
    <span class="mark" class:playing={call.player.playing}>♪</span>
    <b>{call.player.source}</b>
    <span class="who">{mine ? 'ведёте вы' : `ведёт ${host}`}</span>

    <button title="предыдущий трек" onclick={() => call.musicCommand('previous')}>‹‹</button>
    <button class="wide" onclick={() => call.musicCommand('toggle')}>
      {call.player.playing ? 'пауза' : 'играть'}
    </button>
    <button title="следующий трек" onclick={() => call.musicCommand('next')}>››</button>

    <label class="vol">
      громкость
      <input
        type="range"
        min="0"
        max="2"
        step="0.05"
        value={call.musicVolume}
        oninput={(event) => call.setMusicVolume(Number(event.currentTarget.value))}
      />
      <span>{Math.round(call.musicVolume * 100)}%</span>
    </label>

    {#if mine}
      <button class="off" onclick={() => call.stopMusic()}>выключить</button>
    {/if}
  </div>
{/if}

{#if call.musicPicker && !call.sharingMusic}
  <div class="picker">
    <span class="hint">чей звук включить на комнату:</span>
    {#each call.sources as source (source.id)}
      <button onclick={() => call.startMusic(source.id)}>{source.name}</button>
    {/each}
    {#if call.sources.length === 0}
      <span class="hint">
        система не отдала ни одного источника — проверьте разрешение на запись экрана
      </span>
    {/if}
    <button class="off" onclick={() => call.toggleMusicPicker()}>отмена</button>
  </div>
{/if}

<style>
  .player,
  .picker {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: var(--gap-4);
    padding: 4px var(--gap-4);
    border-bottom: 1px solid var(--line);
    font-size: var(--text-sm);
  }

  .player b {
    color: var(--fg-hi);
  }
  .who,
  .hint {
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
  }

  /* Значок дышит, только когда звук и правда идёт. Состояние берётся из
     самого потока, а не из того, что кто-то нажал: музыку могли остановить
     и мимо нас, прямо в приложении-источнике. */
  .mark {
    color: var(--fg-faint);
  }
  .mark.playing {
    color: var(--fg-hi);
    animation: pulse 1.8s var(--ease) infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }

  button {
    border: 1px solid var(--fg-faint);
    color: var(--fg-dim);
    padding: 1px 9px;
    font-size: var(--text-xs);
    transition: color var(--fast) var(--ease), border-color var(--fast) var(--ease);
  }
  button:hover {
    color: var(--fg-hi);
    border-color: var(--fg-dim);
  }
  button.wide {
    min-width: 5.5rem;
  }
  button.off {
    color: var(--fg-dimmer);
    border-color: transparent;
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  .vol {
    display: flex;
    align-items: center;
    gap: var(--gap-3);
    margin-left: auto;
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
  }
  .vol input {
    width: 7rem;
    accent-color: var(--fg);
  }
  .vol span {
    color: var(--fg-dim);
    min-width: 2.5rem;
    text-align: right;
  }
</style>
