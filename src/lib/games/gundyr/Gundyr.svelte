<script lang="ts">
  // Бой рисуется трёхмерной сценой, а считается движком: сюда приезжают только
  // клавиши и готовый мир. Разделение не ради красоты — симуляцию без окна
  // можно прогнать тысячу раз и убедиться, что босс проходим (pnpm test:games).

  import Screen from '../Screen.svelte';
  import {
    bossMaxHp,
    maxEstus,
    maxHp,
    maxStamina,
    newWorld,
    step,
    type Input,
  } from './engine';
  import { Fight } from './scene';

  interface Props {
    onwin: () => void;
    onback: () => void;
    /** В отдельном окне рамы зала нет: там бой занимает всё. */
    bare?: boolean;
  }
  const { onwin, onback, bare = false }: Props = $props();

  let canvas: HTMLCanvasElement | undefined = $state();
  let world = newWorld();
  let announced = false;

  let hud = $state({
    hp: maxHp(),
    stamina: maxStamina(),
    estus: maxEstus(),
    boss: 1,
    awake: false,
    phase: world.phase,
    message: world.message,
    flash: '',
    attempt: 1,
  });

  const keys = new Set<string>();

  function input(): Input {
    return {
      up: keys.has('w') || keys.has('ц') || keys.has('arrowup'),
      down: keys.has('s') || keys.has('ы') || keys.has('arrowdown'),
      left: keys.has('a') || keys.has('ф') || keys.has('arrowleft'),
      right: keys.has('d') || keys.has('в') || keys.has('arrowright'),
      roll: keys.has(' '),
      light: keys.has('j') || keys.has('о'),
      heavy: keys.has('k') || keys.has('л'),
      block: keys.has('l') || keys.has('д'),
      parry: keys.has('i') || keys.has('ш'),
      heal: keys.has('r') || keys.has('к'),
      sprint: keys.has('shift'),
    };
  }

  /** Смерть — не конец, а следующая попытка: счётчик так и живёт. */
  function retry(): void {
    world = newWorld(world.attempt + 1);
    announced = false;
    keys.clear();
  }

  $effect(() => {
    if (!canvas) return;
    const fight = new Fight(canvas);

    const fit = () => {
      const box = canvas!.getBoundingClientRect();
      fight.resize(Math.max(1, box.width), Math.max(1, box.height));
    };
    fit();
    const watcher = new ResizeObserver(fit);
    watcher.observe(canvas);

    let frame = 0;
    let last = performance.now();
    const loop = (now: number) => {
      const dt = Math.min((now - last) / 1000, 0.05);
      last = now;

      step(world, input(), dt);
      fight.render(world, dt);

      hud = {
        hp: Math.max(0, world.player.hp),
        stamina: Math.max(0, world.player.stamina),
        estus: world.player.estus,
        boss: Math.max(0, world.boss.hp) / bossMaxHp(),
        awake: world.boss.state !== 'спит',
        phase: world.phase,
        message: world.message,
        flash: world.flashLeft > 0 ? world.flash : '',
        attempt: world.attempt,
      };
      if (world.phase === 'победа' && !announced) {
        announced = true;
        onwin();
      }
      frame = requestAnimationFrame(loop);
    };
    frame = requestAnimationFrame(loop);

    return () => {
      cancelAnimationFrame(frame);
      watcher.disconnect();
      fight.dispose();
    };
  });
</script>

<svelte:window
  onkeydown={(event) => {
    if (event.key === 'Escape') return;
    if ([' ', 'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'].includes(event.key)) {
      event.preventDefault();
    }
    keys.add(event.key.toLowerCase());
  }}
  onkeyup={(event) => keys.delete(event.key.toLowerCase())}
  onblur={() => keys.clear()}
/>

{#snippet arena()}
  <div class="arena" class:bare>
    <canvas bind:this={canvas} aria-label="Арена боя с Судией Гундиром">
      Бой с Судией Гундиром. Что происходит — сказано в строке состояния.
    </canvas>

    <!-- Полосы игрока слева сверху, полоса босса снизу: как в первоисточнике -->
    <div class="mine">
      <div class="bar hp"><i style="width: {(hud.hp / maxHp()) * 100}%"></i></div>
      <div class="bar stamina"><i style="width: {(hud.stamina / maxStamina()) * 100}%"></i></div>
      <div class="estus">
        {#each Array.from({ length: maxEstus() }) as _, i (i)}
          <svg class="flask" class:empty={i >= hud.estus} viewBox="0 0 12 16" aria-hidden="true">
            <path d="M4.4 1h3.2v3.6l3 5.9a2.3 2.3 0 0 1-2 3.4H3.4a2.3 2.3 0 0 1-2-3.4l3-5.9z" />
          </svg>
        {/each}
        <em>R</em>
      </div>
    </div>

    {#if hud.awake && hud.phase !== 'завеса'}
      <div class="boss">
        <span class="name">Судия Гундир</span>
        <div class="bar boss-hp"><i style="width: {hud.boss * 100}%"></i></div>
      </div>
    {/if}

    {#if hud.flash}<div class="flash">{hud.flash}</div>{/if}

    <div class="keys">
      <span><b>WASD</b> двигаться</span>
      <span><b>пробел</b> перекат</span>
      <span><b>J</b> удар</span>
      <span><b>K</b> тяжёлый</span>
      <span><b>L</b> блок</span>
      <span><b>I</b> парировать</span>
      <span><b>R</b> глоток</span>
    </div>

    {#if hud.phase === 'завеса'}
      <div class="veil">
        <b>Туманная завеса</b>
        <span>За ней на коленях сидит судия. W — войти.</span>
      </div>
    {:else if hud.phase === 'смерть' || hud.phase === 'победа'}
      <div class="veil end" class:victory={hud.phase === 'победа'}>
        <b>{hud.phase === 'победа' ? 'ВРАГ ПОВЕРЖЕН' : 'ВЫ ПОГИБЛИ'}</b>
        <span>попытка {hud.attempt}</span>
        <div class="row">
          <button class="ghost" onclick={retry}>
            {hud.phase === 'победа' ? 'ещё раз' : 'к костру'}
          </button>
          {#if !bare}<button class="ghost" onclick={onback}>в зал</button>{/if}
        </div>
      </div>
    {/if}
  </div>
{/snippet}

{#if bare}
  {@render arena()}
{:else}
  <Screen
    title="Судия Гундир"
    hint="перекат уходит от всего · первая фаза парируется"
    status={hud.phase === 'победа'
      ? 'враг повержен — замок открыт'
      : hud.phase === 'смерть'
        ? 'вы погибли'
        : hud.message}
    tone={hud.phase === 'победа' ? 'победа' : hud.phase === 'смерть' ? 'поражение' : 'обычный'}
    {onback}
  >
    {@render arena()}
  </Screen>
{/if}

<style>
  .arena {
    position: relative;
    width: min(100%, 64rem);
    aspect-ratio: 16 / 9;
    border: 1px solid var(--edge);
    border-radius: 16px;
    overflow: hidden;
    background: #050505;
    box-shadow: 0 30px 60px -40px rgba(0, 0, 0, 0.95);
  }
  /* В отдельном окне рамки не нужны: бой занимает всё, что дали */
  .arena.bare {
    width: 100%;
    height: 100%;
    aspect-ratio: auto;
    border: 0;
    border-radius: 0;
    box-shadow: none;
  }
  canvas {
    display: block;
    width: 100%;
    height: 100%;
  }

  .mine {
    position: absolute;
    top: 1.1rem;
    left: 1.1rem;
    display: grid;
    gap: 5px;
    width: min(22rem, 44%);
  }
  .bar {
    height: 0.75rem;
    border: 1px solid rgba(0, 0, 0, 0.75);
    background: rgba(0, 0, 0, 0.6);
    border-radius: 2px;
    overflow: hidden;
  }
  .bar i {
    display: block;
    height: 100%;
    transition: width 120ms linear;
  }
  .hp i {
    background: linear-gradient(90deg, #9a9a9a, #ffffff);
  }
  .stamina i {
    background: repeating-linear-gradient(-45deg, #7d7d7d 0 4px, #5e5e5e 4px 8px);
  }
  .stamina {
    height: 0.5rem;
    width: 80%;
  }

  .estus {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-top: 2px;
  }
  .flask {
    width: 0.8rem;
    height: 1.05rem;
    fill: var(--paper);
  }
  .flask.empty {
    fill: none;
    stroke: var(--paper);
    stroke-width: 1.2;
    opacity: 0.35;
  }
  .estus em {
    margin-left: 0.3rem;
    font-size: 0.62rem;
    font-style: normal;
    letter-spacing: 0.12em;
    color: var(--muted);
  }

  .boss {
    position: absolute;
    left: 50%;
    bottom: 2.2rem;
    transform: translateX(-50%);
    width: min(38rem, 70%);
    display: grid;
    gap: 4px;
    justify-items: center;
  }
  .boss .name {
    font-size: 0.78rem;
    letter-spacing: 0.24em;
    text-transform: uppercase;
    color: var(--paper);
    text-shadow: 0 2px 8px #000;
  }
  .boss-hp {
    width: 100%;
    height: 0.62rem;
  }
  .boss-hp i {
    background: linear-gradient(90deg, #bdbdbd, #ffffff);
  }

  .flash {
    position: absolute;
    left: 50%;
    top: 24%;
    transform: translateX(-50%);
    letter-spacing: 0.18em;
    text-transform: uppercase;
    font-size: 0.82rem;
    color: var(--paper);
    text-shadow: 0 2px 10px #000;
  }

  .keys {
    position: absolute;
    right: 1rem;
    top: 1.1rem;
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 4px 10px;
    max-width: 22rem;
    font-size: 0.66rem;
    color: var(--muted);
  }
  .keys b {
    color: var(--paper);
    font-weight: 600;
  }

  .veil {
    position: absolute;
    inset: 0;
    display: grid;
    place-content: center;
    justify-items: center;
    gap: 0.9rem;
    background: rgba(5, 5, 5, 0.72);
    backdrop-filter: blur(2px);
    text-align: center;
  }
  .veil b {
    font-size: clamp(1.4rem, 1rem + 2.4vw, 2.6rem);
    letter-spacing: 0.24em;
    color: var(--muted);
  }
  .veil.end b {
    color: #8b8b8b;
    text-shadow: 0 0 30px rgba(0, 0, 0, 0.9);
  }
  .veil.victory b {
    color: #ffffff;
    text-shadow: 0 0 30px rgba(255, 255, 255, 0.35);
  }
  .veil span {
    color: var(--muted);
    font-size: 0.86rem;
  }
  .row {
    display: flex;
    gap: 0.6rem;
  }

  .ghost {
    padding: 0.4rem 0.9rem;
    border: 1px solid var(--edge);
    border-radius: 8px;
    font-size: 0.84rem;
    color: var(--paper);
    background: rgba(0, 0, 0, 0.35);
  }
  .ghost:hover {
    border-color: var(--paper);
    background: var(--lift-2);
  }
</style>
