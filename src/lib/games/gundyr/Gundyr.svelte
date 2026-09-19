<script lang="ts">
  // Бой рисуется здесь, а считается в engine.ts. Разделение не ради красоты:
  // симуляцию без холста можно прогнать тысячу раз и убедиться, что босс
  // проходим, — что и делает pnpm test:games.

  import Screen from '../Screen.svelte';
  import {
    ARENA,
    bossMaxHp,
    bossRadius,
    maxHp,
    maxStamina,
    newWorld,
    playerRadius,
    step,
    type Input,
    type Shape,
  } from './engine';

  interface Props {
    onwin: () => void;
    onback: () => void;
  }
  const { onwin, onback }: Props = $props();

  const WIDTH = 920;
  const HEIGHT = 600;

  let canvas: HTMLCanvasElement | undefined = $state();
  let world = newWorld();
  let phase = $state(world.phase);
  let hud = $state({ hp: 100, stamina: 100, estus: 3, boss: 1, message: '' });
  let announced = false;

  /** Щебень на полу. Считается один раз: дрожать под ногами он не должен. */
  const RUBBLE: [number, number, number][] = Array.from({ length: 34 }, (_, i) => {
    const angle = (i * 2.399) % (Math.PI * 2);
    const radius = 30 + ((i * 97) % 210);
    return [angle, radius, 0.8 + ((i * 13) % 5) * 0.34];
  });

  const keys = new Set<string>();
  /** Вспышки попаданий живут дольше одного кадра, иначе их не видно. */
  const flashes: { shape: Shape; life: number }[] = [];
  /** Плавно догоняющая полоса урона — так виден размер откушенного куска. */
  let ghost = 1;

  function input(): Input {
    return {
      up: keys.has('w') || keys.has('ц') || keys.has('arrowup'),
      down: keys.has('s') || keys.has('ы') || keys.has('arrowdown'),
      left: keys.has('a') || keys.has('ф') || keys.has('arrowleft'),
      right: keys.has('d') || keys.has('в') || keys.has('arrowright'),
      roll: keys.has(' '),
      attack: keys.has('j') || keys.has('о') || keys.has('enter'),
      heal: keys.has('r') || keys.has('к'),
    };
  }

  function restart(): void {
    world = newWorld();
    phase = world.phase;
    announced = false; // победа во втором заходе — такая же победа
    ghost = 1;
    flashes.length = 0;
    keys.clear();
  }

  function path(ctx: CanvasRenderingContext2D, shape: Shape): void {
    ctx.beginPath();
    if (shape.kind === 'circle') {
      ctx.arc(shape.x, shape.y, shape.r, 0, Math.PI * 2);
      return;
    }
    if (shape.kind === 'arc') {
      ctx.moveTo(shape.x, shape.y);
      ctx.arc(shape.x, shape.y, shape.r, shape.facing - shape.spread / 2, shape.facing + shape.spread / 2);
      ctx.closePath();
      return;
    }
    const dx = Math.cos(shape.facing);
    const dy = Math.sin(shape.facing);
    const nx = -dy * shape.half;
    const ny = dx * shape.half;
    ctx.moveTo(shape.x + nx, shape.y + ny);
    ctx.lineTo(shape.x + dx * shape.len + nx, shape.y + dy * shape.len + ny);
    ctx.lineTo(shape.x + dx * shape.len - nx, shape.y + dy * shape.len - ny);
    ctx.lineTo(shape.x - nx, shape.y - ny);
    ctx.closePath();
  }

  function draw(ctx: CanvasRenderingContext2D, dt: number): void {
    const { player, boss } = world;
    ctx.clearRect(0, 0, WIDTH, HEIGHT);

    // Пол арены.
    const floor = ctx.createRadialGradient(ARENA.x, ARENA.y, 20, ARENA.x, ARENA.y, ARENA.r);
    floor.addColorStop(0, '#242424');
    floor.addColorStop(1, '#0d0d0d');
    ctx.fillStyle = floor;
    ctx.beginPath();
    ctx.arc(ARENA.x, ARENA.y, ARENA.r, 0, Math.PI * 2);
    ctx.fill();
    ctx.strokeStyle = 'rgba(255, 255, 255, 0.22)';
    ctx.lineWidth = 2;
    ctx.stroke();

    // Фактура пола: кольца и щебень. Заодно по ним видно собственное движение.
    ctx.strokeStyle = 'rgba(255, 255, 255, 0.035)';
    ctx.lineWidth = 1;
    for (const ring of [0.42, 0.68, 0.88]) {
      ctx.beginPath();
      ctx.arc(ARENA.x, ARENA.y, ARENA.r * ring, 0, Math.PI * 2);
      ctx.stroke();
    }
    ctx.fillStyle = 'rgba(255, 255, 255, 0.05)';
    for (const [angle, radius, size] of RUBBLE) {
      ctx.beginPath();
      ctx.arc(
        ARENA.x + Math.cos(angle) * radius,
        ARENA.y + Math.sin(angle) * radius,
        size,
        0,
        Math.PI * 2,
      );
      ctx.fill();
    }

    // Замахи: видно, куда прилетит, и сколько времени осталось.
    for (const tell of world.tells) {
      if (tell.hot) {
        flashes.push({ shape: tell.shape, life: 0.16 });
        continue;
      }
      // Замах наливается светом: чем ближе удар, тем ярче пятно под ногами.
      path(ctx, tell.shape);
      ctx.fillStyle = `rgba(255, 255, 255, ${0.05 + 0.16 * tell.progress})`;
      ctx.fill();
      ctx.strokeStyle = `rgba(255, 255, 255, ${0.25 + 0.6 * tell.progress})`;
      ctx.lineWidth = 1.5 + 2 * tell.progress;
      ctx.stroke();
    }

    for (let i = flashes.length - 1; i >= 0; i--) {
      const flash = flashes[i];
      flash.life -= dt;
      if (flash.life <= 0) {
        flashes.splice(i, 1);
        continue;
      }
      path(ctx, flash.shape);
      ctx.fillStyle = `rgba(255, 255, 255, ${0.55 * (flash.life / 0.16)})`;
      ctx.fill();
    }

    const shadow = (x: number, y: number, r: number) => {
      ctx.fillStyle = 'rgba(0, 0, 0, 0.42)';
      ctx.beginPath();
      ctx.ellipse(x, y + r * 0.72, r * 1.15, r * 0.42, 0, 0, Math.PI * 2);
      ctx.fill();
    };

    // Босс.
    shadow(boss.at.x, boss.at.y, bossRadius());
    ctx.save();
    ctx.translate(boss.at.x, boss.at.y);
    ctx.rotate(boss.facing);
    // Древко и лезвие алебарды — по ним и читается, куда он смотрит.
    ctx.strokeStyle = boss.phase === 2 ? '#d8d8d8' : '#8b8b8b';
    ctx.lineWidth = 5;
    ctx.lineCap = 'round';
    ctx.beginPath();
    ctx.moveTo(-10, 0);
    ctx.lineTo(62, 0);
    ctx.stroke();
    ctx.fillStyle = boss.phase === 2 ? '#ffffff' : '#b5b5b5';
    ctx.beginPath();
    ctx.moveTo(62, -3);
    ctx.lineTo(84, -14);
    ctx.lineTo(80, 0);
    ctx.lineTo(84, 14);
    ctx.lineTo(62, 3);
    ctx.closePath();
    ctx.fill();

    // Во второй фазе из него лезут отростки.
    if (boss.phase === 2) {
      ctx.strokeStyle = 'rgba(255, 255, 255, 0.7)';
      ctx.lineWidth = 3;
      for (let i = 0; i < 3; i++) {
        const wave = Math.sin(world.time * 6 + i * 2.1) * 9;
        ctx.beginPath();
        ctx.moveTo(-6, 0);
        ctx.quadraticCurveTo(-26, wave, -42 - i * 5, wave * 1.6);
        ctx.stroke();
      }
    }

    // Туловище и щель забрала.
    ctx.fillStyle = boss.phase === 2 ? '#2e2e2e' : '#242424';
    ctx.strokeStyle = boss.phase === 2 ? '#ffffff' : '#6d6d6d';
    ctx.lineWidth = 3;
    ctx.beginPath();
    ctx.arc(0, 0, bossRadius(), 0, Math.PI * 2);
    ctx.fill();
    ctx.stroke();
    ctx.strokeStyle = boss.phase === 2 ? '#ffffff' : '#9a9a9a';
    ctx.lineWidth = 3;
    ctx.beginPath();
    ctx.moveTo(bossRadius() * 0.35, -7);
    ctx.lineTo(bossRadius() * 0.35, 7);
    ctx.stroke();
    ctx.restore();

    // Игрок. В перекате — след и светлое кольцо неуязвимости.
    if (player.state === 'перекат') {
      ctx.fillStyle = 'rgba(255, 255, 255, 0.14)';
      ctx.beginPath();
      ctx.arc(
        player.at.x - player.roll.x * 26,
        player.at.y - player.roll.y * 26,
        playerRadius(),
        0,
        Math.PI * 2,
      );
      ctx.fill();
    }
    shadow(player.at.x, player.at.y, playerRadius());
    ctx.fillStyle = player.state === 'лечится' ? '#bdbdbd' : '#ffffff';
    ctx.beginPath();
    ctx.arc(player.at.x, player.at.y, playerRadius(), 0, Math.PI * 2);
    ctx.fill();
    // Щит со стороны, противоположной клинку: видно, где перёд.
    ctx.strokeStyle = 'rgba(130, 130, 130, 0.9)';
    ctx.lineWidth = 4;
    ctx.beginPath();
    ctx.arc(
      player.at.x,
      player.at.y,
      playerRadius() + 3,
      player.facing + 1.15,
      player.facing + 2.35,
    );
    ctx.stroke();
    if (player.iframes > 0) {
      ctx.strokeStyle = 'rgba(255, 255, 255, 0.85)';
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(player.at.x, player.at.y, playerRadius() + 5, 0, Math.PI * 2);
      ctx.stroke();
    }
    ctx.strokeStyle = '#ffffff';
    ctx.lineWidth = 3;
    ctx.beginPath();
    ctx.moveTo(player.at.x, player.at.y);
    ctx.lineTo(
      player.at.x + Math.cos(player.facing) * 22,
      player.at.y + Math.sin(player.facing) * 22,
    );
    ctx.stroke();

    // Полоса здоровья босса — поперёк экрана, как положено.
    const share = Math.max(0, boss.hp) / bossMaxHp();
    ghost += (share - ghost) * Math.min(1, dt * 3);
    const barX = (WIDTH - 560) / 2;
    ctx.fillStyle = 'rgba(0, 0, 0, 0.55)';
    ctx.fillRect(barX - 2, 26, 564, 14);
    // Догоняющая полоса — тусклая, живое здоровье — белое: видно откушенный кусок.
    ctx.fillStyle = 'rgba(255, 255, 255, 0.28)';
    ctx.fillRect(barX, 28, 560 * ghost, 10);
    ctx.fillStyle = '#ffffff';
    ctx.fillRect(barX, 28, 560 * share, 10);
    ctx.fillStyle = 'rgba(255, 255, 255, 0.75)';
    ctx.font = '12px ui-sans-serif, system-ui, sans-serif';
    ctx.textAlign = 'center';
    ctx.fillText(boss.phase === 2 ? 'СУДИЯ ГУНДИР · ВТОРАЯ ФАЗА' : 'СУДИЯ ГУНДИР', WIDTH / 2, 18);
  }

  $effect(() => {
    const context = canvas?.getContext('2d');
    if (!canvas || !context) return;

    const ratio = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = WIDTH * ratio;
    canvas.height = HEIGHT * ratio;
    context.scale(ratio, ratio);

    let frame = 0;
    let last = performance.now();
    const loop = (now: number) => {
      const dt = Math.min((now - last) / 1000, 0.05);
      last = now;
      step(world, input(), dt);
      draw(context, dt);

      if (world.phase !== phase) phase = world.phase;
      hud = {
        hp: world.player.hp,
        stamina: world.player.stamina,
        estus: world.player.estus,
        boss: Math.max(0, world.boss.hp) / bossMaxHp(),
        message: world.message,
      };
      if (world.phase === 'победа' && !announced) {
        announced = true;
        onwin();
      }
      frame = requestAnimationFrame(loop);
    };
    frame = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(frame);
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

<Screen
  title="Судия Гундир"
  hint="WASD — двигаться · пробел — перекат · J — удар · R — глоток"
  status={phase === 'победа'
    ? 'враг повержен — замок открыт'
    : phase === 'смерть'
      ? 'вы погибли'
      : hud.message}
  tone={phase === 'победа' ? 'победа' : phase === 'смерть' ? 'поражение' : 'обычный'}
  {onback}
>
  <div class="arena">
    <!-- Содержимое внутри холста — то, что достаётся экранному диктору:
         сам рисунок ему недоступен, а происходящее словами есть в строке
         состояния под ареной. -->
    <canvas
      bind:this={canvas}
      style="aspect-ratio: {WIDTH} / {HEIGHT}"
      aria-label="Арена боя с Судией Гундиром"
    >
      Бой с Судией Гундиром. Что происходит — сказано в строке состояния под ареной.
    </canvas>

    <div class="hud">
      <div class="line">
        <span class="label">жизнь</span>
        <div class="bar hp"><i style="width: {Math.max(0, hud.hp / maxHp()) * 100}%"></i></div>
      </div>
      <div class="line">
        <span class="label">сила</span>
        <div class="bar stamina">
          <i style="width: {Math.max(0, hud.stamina / maxStamina()) * 100}%"></i>
        </div>
      </div>
      <div class="line">
        <span class="label">фляга</span>
        <span class="estus">
          {#each Array.from({ length: 3 }) as _, i (i)}
            <svg class="flask" class:empty={i >= hud.estus} viewBox="0 0 12 16" aria-hidden="true">
              <path d="M4.4 1h3.2v3.6l3 5.9a2.3 2.3 0 0 1-2 3.4H3.4a2.3 2.3 0 0 1-2-3.4l3-5.9z" />
            </svg>
          {/each}
          <em>R</em>
        </span>
      </div>
    </div>

    {#if phase === 'смерть' || phase === 'победа'}
      <div class="veil" class:victory={phase === 'победа'}>
        <b>{phase === 'победа' ? 'ВРАГ ПОВЕРЖЕН' : 'ВЫ ПОГИБЛИ'}</b>
        <div class="row">
          <button class="ghost" onclick={restart}>
            {phase === 'победа' ? 'ещё раз' : 'попробовать снова'}
          </button>
          <button class="ghost" onclick={onback}>в зал</button>
        </div>
      </div>
    {/if}
  </div>
</Screen>

<style>
  .arena {
    position: relative;
    width: min(100%, 58rem);
  }

  canvas {
    width: 100%;
    display: block;
    border: 1px solid var(--edge);
    border-radius: 16px;
    background: #0d0b10;
    box-shadow: 0 30px 60px -40px rgba(0, 0, 0, 0.95);
  }

  .hud {
    position: absolute;
    left: 1.1rem;
    bottom: 1rem;
    display: grid;
    gap: 0.3rem;
    width: min(17rem, 52%);
  }

  .line {
    display: grid;
    grid-template-columns: 3.2rem 1fr;
    align-items: center;
    gap: 0.5rem;
  }

  .label {
    font-size: 0.62rem;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: var(--muted);
  }

  .bar {
    position: relative;
    height: 0.62rem;
    border: 1px solid rgba(0, 0, 0, 0.6);
    background: rgba(0, 0, 0, 0.55);
    border-radius: 3px;
    overflow: hidden;
  }
  .bar i {
    display: block;
    height: 100%;
    transition: width 120ms linear;
  }
  /* Жизнь и сила различаются светлотой и штриховкой, а не цветом */
  .hp i {
    background: linear-gradient(90deg, #9a9a9a, #ffffff);
  }
  .stamina i {
    background: repeating-linear-gradient(
      -45deg,
      #7d7d7d 0 4px,
      #5e5e5e 4px 8px
    );
  }
  .estus {
    display: flex;
    align-items: center;
    gap: 0.3rem;
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

  .veil {
    position: absolute;
    inset: 0;
    display: grid;
    place-content: center;
    justify-items: center;
    gap: 1.2rem;
    border-radius: 16px;
    background: rgba(8, 6, 10, 0.78);
    backdrop-filter: blur(2px);
  }
  /* «Вы погибли» — тусклым, «враг повержен» — в полный свет: разница читается
     без цвета, как и всё остальное в этой системе. */
  .veil b {
    font-size: clamp(1.6rem, 1rem + 3vw, 3rem);
    letter-spacing: 0.22em;
    color: #8b8b8b;
    text-shadow: 0 0 28px rgba(0, 0, 0, 0.8);
  }
  .veil.victory b {
    color: #ffffff;
    text-shadow: 0 0 28px rgba(255, 255, 255, 0.35);
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
    transition: border-color 160ms ease, background 160ms ease;
  }
  .ghost:hover {
    border-color: var(--paper);
    background: var(--lift-2);
  }
</style>
