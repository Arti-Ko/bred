<script lang="ts">
  // Бой рисуется здесь, а считается в engine.ts. Разделение не ради красоты:
  // симуляцию без холста можно прогнать тысячу раз и убедиться, что босс
  // проходим, — что и делает pnpm test:games.
  //
  // Вид двумерный, сверху, но механика — та, что была написана под трёхмерный
  // бой: настоящий мувсет, парирование с добиванием, стойкость, блок, эстус,
  // туманная завеса. И интерфейс персонажа как в первоисточнике: полосы жизни
  // и выносливости слева сверху, крестовина снаряжения с флягой слева снизу,
  // полоса босса внизу по центру.

  import Screen from '../Screen.svelte';
  import {
    ARENA,
    bossMaxHp,
    bossRadius,
    maxEstus,
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
    /** В отдельном окне рамы зала нет: там бой занимает всё. */
    bare?: boolean;
  }
  const { onwin, onback, bare = false }: Props = $props();

  const WIDTH = 920;
  const HEIGHT = 600;

  let canvas: HTMLCanvasElement | undefined = $state();
  let world = newWorld();
  let announced = false;

  let hud = $state({
    hp: maxHp(),
    hpGhost: maxHp(),
    stamina: maxStamina(),
    estus: maxEstus(),
    boss: 1,
    bossGhost: 1,
    awake: false,
    bossPhase: 1 as 1 | 2,
    phase: world.phase,
    message: world.message,
    flash: '',
    attempt: 1,
  });

  const keys = new Set<string>();
  /** Кнопки мыши: левая — удар, правая — блок, средняя — парирование. */
  const mouse = new Set<number>();
  /** Захват цели: взгляд приклеен к судии. Q — снять, чтобы убежать. */
  let lockOn = $state(true);

  /** Щебень на полу. Считается один раз: дрожать под ногами он не должен. */
  const RUBBLE: [number, number, number][] = Array.from({ length: 40 }, (_, i) => {
    const angle = (i * 2.399) % (Math.PI * 2);
    const radius = 30 + ((i * 97) % 215);
    return [angle, radius, 0.8 + ((i * 13) % 5) * 0.34];
  });
  /** Вспышки попаданий живут дольше одного кадра, иначе их не видно. */
  const flashes: { shape: Shape; life: number }[] = [];

  function input(): Input {
    return {
      // Камера смотрит сверху и не вращается: «вперёд» — это вверх по экрану.
      yaw: 0,
      lockOn,
      up: keys.has('w') || keys.has('ц') || keys.has('arrowup'),
      down: keys.has('s') || keys.has('ы') || keys.has('arrowdown'),
      left: keys.has('a') || keys.has('ф') || keys.has('arrowleft'),
      right: keys.has('d') || keys.has('в') || keys.has('arrowright'),
      roll: keys.has(' '),
      light: keys.has('j') || keys.has('о') || keys.has('enter') || mouse.has(0),
      heavy: keys.has('k') || keys.has('л'),
      block: keys.has('l') || keys.has('д') || mouse.has(2),
      parry: keys.has('i') || keys.has('ш') || mouse.has(1),
      heal: keys.has('r') || keys.has('к'),
      sprint: keys.has('shift'),
    };
  }

  /** Смерть — не конец, а следующая попытка: счётчик так и живёт. */
  function retry(): void {
    world = newWorld(world.attempt + 1);
    announced = false;
    flashes.length = 0;
    keys.clear();
    mouse.clear();
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

  function drawFloor(ctx: CanvasRenderingContext2D): void {
    const floor = ctx.createRadialGradient(ARENA.x, ARENA.y, 20, ARENA.x, ARENA.y, ARENA.r);
    floor.addColorStop(0, '#262626');
    floor.addColorStop(1, '#0c0c0c');
    ctx.fillStyle = floor;
    ctx.beginPath();
    ctx.arc(ARENA.x, ARENA.y, ARENA.r, 0, Math.PI * 2);
    ctx.fill();
    ctx.strokeStyle = 'rgba(255, 255, 255, 0.22)';
    ctx.lineWidth = 2;
    ctx.stroke();

    // Плиты двора храма: кольца и швы. По ним же видно собственное движение.
    ctx.strokeStyle = 'rgba(255, 255, 255, 0.04)';
    ctx.lineWidth = 1;
    for (const ring of [0.3, 0.52, 0.72, 0.9]) {
      ctx.beginPath();
      ctx.arc(ARENA.x, ARENA.y, ARENA.r * ring, 0, Math.PI * 2);
      ctx.stroke();
    }
    for (let i = 0; i < 16; i++) {
      const angle = (i / 16) * Math.PI * 2 + (i % 2) * 0.1;
      ctx.beginPath();
      ctx.moveTo(ARENA.x + Math.cos(angle) * ARENA.r * 0.3, ARENA.y + Math.sin(angle) * ARENA.r * 0.3);
      ctx.lineTo(ARENA.x + Math.cos(angle) * ARENA.r, ARENA.y + Math.sin(angle) * ARENA.r);
      ctx.stroke();
    }
    ctx.fillStyle = 'rgba(255, 255, 255, 0.05)';
    for (const [angle, radius, size] of RUBBLE) {
      ctx.beginPath();
      ctx.arc(ARENA.x + Math.cos(angle) * radius, ARENA.y + Math.sin(angle) * radius, size, 0, Math.PI * 2);
      ctx.fill();
    }
  }

  function drawTells(ctx: CanvasRenderingContext2D, dt: number): void {
    for (const tell of world.tells) {
      if (tell.hot) {
        flashes.push({ shape: tell.shape, life: 0.16 });
        continue;
      }
      // Замах наливается светом: чем ближе удар, тем ярче пятно под ногами.
      // Парируемый — пунктиром: по нему и ловят, его надо узнавать сразу.
      path(ctx, tell.shape);
      ctx.fillStyle = `rgba(255, 255, 255, ${0.05 + 0.16 * tell.progress})`;
      ctx.fill();
      ctx.setLineDash(tell.parryable ? [7, 5] : []);
      ctx.strokeStyle = `rgba(255, 255, 255, ${0.25 + 0.6 * tell.progress})`;
      ctx.lineWidth = 1.5 + 2 * tell.progress;
      ctx.stroke();
      ctx.setLineDash([]);
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
  }

  function shadow(ctx: CanvasRenderingContext2D, x: number, y: number, r: number): void {
    ctx.fillStyle = 'rgba(0, 0, 0, 0.42)';
    ctx.beginPath();
    ctx.ellipse(x, y + r * 0.72, r * 1.15, r * 0.42, 0, 0, Math.PI * 2);
    ctx.fill();
  }

  function drawBoss(ctx: CanvasRenderingContext2D): void {
    const boss = world.boss;
    const r = bossRadius();
    const second = boss.phase === 2;
    shadow(ctx, boss.at.x, boss.at.y, r);

    ctx.save();
    ctx.translate(boss.at.x, boss.at.y);

    if (boss.state === 'спит' || boss.state === 'пробуждается') {
      // На коленях, с мечом в груди: меч торчит вверх, тело осело.
      const rising = boss.state === 'пробуждается' ? 1 - Math.max(0, boss.timer) / 1.1 : 0;
      const scale = 0.82 + 0.18 * rising;
      ctx.scale(scale, scale);
      ctx.fillStyle = '#1d1d1d';
      ctx.strokeStyle = '#5a5a5a';
      ctx.lineWidth = 3;
      ctx.beginPath();
      ctx.arc(0, 0, r, 0, Math.PI * 2);
      ctx.fill();
      ctx.stroke();
      // Свёрнутый меч — тлеющая точка в груди.
      const glow = 0.35 + 0.25 * Math.sin(world.time * 2.2);
      ctx.fillStyle = `rgba(255, 255, 255, ${glow})`;
      ctx.beginPath();
      ctx.arc(0, -2, 4, 0, Math.PI * 2);
      ctx.fill();
      ctx.strokeStyle = '#a8a8a8';
      ctx.lineWidth = 3;
      ctx.beginPath();
      ctx.moveTo(0, -2);
      ctx.lineTo(0, -r - 22 + rising * 22);
      ctx.stroke();
      // Алебарда лежит рядом.
      ctx.strokeStyle = '#6d6d6d';
      ctx.lineWidth = 4;
      ctx.beginPath();
      ctx.moveTo(-r - 6, r * 0.6);
      ctx.lineTo(r + 30, r * 0.9);
      ctx.stroke();
      ctx.restore();
      return;
    }

    const open = boss.state === 'открыт';
    ctx.rotate(boss.facing + (open ? 0.5 : 0));

    // Древко и лезвие алебарды — по ним и читается, куда он смотрит.
    ctx.strokeStyle = second ? '#d8d8d8' : '#8b8b8b';
    ctx.lineWidth = 5;
    ctx.lineCap = 'round';
    ctx.beginPath();
    ctx.moveTo(-10, 0);
    ctx.lineTo(62, 0);
    ctx.stroke();
    ctx.fillStyle = second ? '#ffffff' : '#b5b5b5';
    ctx.beginPath();
    ctx.moveTo(62, -3);
    ctx.lineTo(84, -14);
    ctx.lineTo(80, 0);
    ctx.lineTo(84, 14);
    ctx.lineTo(62, 3);
    ctx.closePath();
    ctx.fill();

    // Во второй фазе из него лезет бездна.
    if (second) {
      ctx.strokeStyle = 'rgba(255, 255, 255, 0.7)';
      ctx.lineWidth = 3;
      for (let i = 0; i < 4; i++) {
        const wave = Math.sin(world.time * 6 + i * 2.1) * 9;
        ctx.beginPath();
        ctx.moveTo(-6, (i - 1.5) * 4);
        ctx.quadraticCurveTo(-26, wave, -42 - i * 5, wave * 1.6);
        ctx.stroke();
      }
    }

    // Туловище, наплечники и щель забрала.
    ctx.fillStyle = second ? '#2e2e2e' : '#242424';
    ctx.strokeStyle = second ? '#ffffff' : '#6d6d6d';
    ctx.lineWidth = 3;
    ctx.beginPath();
    ctx.arc(0, 0, r, 0, Math.PI * 2);
    ctx.fill();
    ctx.stroke();
    ctx.fillStyle = second ? '#3a3a3a' : '#2c2c2c';
    for (const side of [-1, 1]) {
      ctx.beginPath();
      ctx.ellipse(-2, side * r * 0.82, 9, 6, 0, 0, Math.PI * 2);
      ctx.fill();
      ctx.stroke();
    }
    ctx.strokeStyle = second ? '#ffffff' : '#9a9a9a';
    ctx.beginPath();
    ctx.moveTo(r * 0.35, -7);
    ctx.lineTo(r * 0.35, 7);
    ctx.stroke();
    ctx.restore();

    // Открыт — пульсирующее кольцо: подойди вплотную и бей, это добивание.
    if (open) {
      const pulse = 0.5 + 0.5 * Math.sin(world.time * 10);
      ctx.strokeStyle = `rgba(255, 255, 255, ${0.35 + 0.45 * pulse})`;
      ctx.lineWidth = 2;
      ctx.setLineDash([4, 6]);
      ctx.beginPath();
      ctx.arc(boss.at.x, boss.at.y, r + 18 + pulse * 4, 0, Math.PI * 2);
      ctx.stroke();
      ctx.setLineDash([]);
    }

    // Захват цели — точка на нём, как в первоисточнике.
    if (lockOn) {
      ctx.fillStyle = '#ffffff';
      ctx.beginPath();
      ctx.arc(boss.at.x, boss.at.y, 3.2, 0, Math.PI * 2);
      ctx.fill();
      ctx.strokeStyle = 'rgba(255, 255, 255, 0.6)';
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.arc(boss.at.x, boss.at.y, 7, 0, Math.PI * 2);
      ctx.stroke();
    }
  }

  function drawPlayer(ctx: CanvasRenderingContext2D): void {
    const player = world.player;
    const r = playerRadius();
    const { x, y } = player.at;
    const facing = player.facing;

    if (player.state === 'перекат') {
      ctx.fillStyle = 'rgba(255, 255, 255, 0.14)';
      ctx.beginPath();
      ctx.arc(x - player.roll.x * 26, y - player.roll.y * 26, r, 0, Math.PI * 2);
      ctx.fill();
    }
    shadow(ctx, x, y, r);

    // Тело. Глоток эстуса — тусклее и с тёплым сердцем, оглушение — шатает.
    const sway = player.state === 'оглушён' ? Math.sin(world.time * 18) * 2 : 0;
    ctx.fillStyle = player.state === 'лечится' ? '#bdbdbd' : '#ffffff';
    ctx.beginPath();
    ctx.arc(x + sway, y, r, 0, Math.PI * 2);
    ctx.fill();
    if (player.state === 'лечится') {
      ctx.fillStyle = 'rgba(255, 255, 255, 0.9)';
      ctx.beginPath();
      ctx.arc(x, y, r * 0.45 + Math.sin(world.time * 12) * 1.5, 0, Math.PI * 2);
      ctx.fill();
    }

    // Щит со стороны, противоположной клинку. В блоке — выставлен вперёд.
    ctx.strokeStyle = player.blocking ? '#ffffff' : 'rgba(130, 130, 130, 0.9)';
    ctx.lineWidth = player.blocking ? 6 : 4;
    ctx.beginPath();
    if (player.blocking) ctx.arc(x, y, r + 4, facing - 0.9, facing + 0.9);
    else ctx.arc(x, y, r + 3, facing + 1.15, facing + 2.35);
    ctx.stroke();

    // Парирование — короткая вспышка щитом вперёд.
    if (player.state === 'парирует') {
      ctx.strokeStyle = 'rgba(255, 255, 255, 0.95)';
      ctx.lineWidth = 3;
      ctx.beginPath();
      ctx.arc(x, y, r + 10, facing - 0.6, facing + 0.6);
      ctx.stroke();
    }

    if (player.iframes > 0) {
      ctx.strokeStyle = 'rgba(255, 255, 255, 0.85)';
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(x, y, r + 5, 0, Math.PI * 2);
      ctx.stroke();
    }

    // Клинок. На добивании — длинный выпад.
    const reach = player.state === 'риспост' ? 44 : player.state === 'тяжёлый' ? 28 : 22;
    ctx.strokeStyle = '#ffffff';
    ctx.lineWidth = 3;
    ctx.beginPath();
    ctx.moveTo(x, y);
    ctx.lineTo(x + Math.cos(facing) * reach, y + Math.sin(facing) * reach);
    ctx.stroke();

    if (player.state === 'оглушён') {
      ctx.fillStyle = 'rgba(255, 255, 255, 0.8)';
      ctx.font = '10px ui-monospace, monospace';
      ctx.textAlign = 'center';
      ctx.fillText('× ×', x, y - r - 8);
    }
  }

  function draw(ctx: CanvasRenderingContext2D, dt: number): void {
    ctx.clearRect(0, 0, WIDTH, HEIGHT);
    drawFloor(ctx);
    drawTells(ctx, dt);
    drawBoss(ctx);
    drawPlayer(ctx);
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

      // Догоняющие полосы — как в первоисточнике: сперва видно, сколько
      // откусили, и только потом полоса сползает до нового значения.
      const hp = Math.max(0, world.player.hp);
      const boss = Math.max(0, world.boss.hp) / bossMaxHp();
      const chase = Math.min(1, dt * 2.5);
      hud = {
        hp,
        hpGhost: hud.hpGhost > hp ? hud.hpGhost + (hp - hud.hpGhost) * chase : hp,
        stamina: Math.max(0, world.player.stamina),
        estus: world.player.estus,
        boss,
        bossGhost: hud.bossGhost > boss ? hud.bossGhost + (boss - hud.bossGhost) * chase : boss,
        awake: world.boss.state !== 'спит',
        bossPhase: world.boss.phase,
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
    return () => cancelAnimationFrame(frame);
  });
</script>

<svelte:window
  onkeydown={(event) => {
    if (event.key === 'Escape') return;
    if ([' ', 'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight'].includes(event.key)) {
      event.preventDefault();
    }
    const key = event.key.toLowerCase();
    // Захват цели переключается нажатием, а не удержанием: держать его незачем.
    if ((key === 'q' || key === 'й') && !event.repeat) lockOn = !lockOn;
    keys.add(key);
  }}
  onkeyup={(event) => keys.delete(event.key.toLowerCase())}
  onblur={() => {
    keys.clear();
    mouse.clear();
  }}
/>

{#snippet arena()}
  <div class="arena" class:bare>
    <div class="frame">
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <canvas
        bind:this={canvas}
        aria-label="Арена боя с Судией Гундиром"
        onmousedown={(event) => {
          event.preventDefault();
          mouse.add(event.button);
        }}
        onmouseup={(event) => mouse.delete(event.button)}
        onmouseleave={() => mouse.clear()}
        oncontextmenu={(event) => event.preventDefault()}
      >
        Бой с Судией Гундиром. Что происходит — сказано в строке состояния.
      </canvas>

      <!-- Полосы персонажа: жизнь длинная и светлая, выносливость короче и в
           штриховку — различаются светлотой и фактурой, цвета в системе нет. -->
      <div class="mine" aria-hidden="true">
        <div class="bar hp">
          <i class="ghost" style="transform: scaleX({hud.hpGhost / maxHp()})"></i>
          <i style="transform: scaleX({hud.hp / maxHp()})"></i>
        </div>
        <div class="bar stamina">
          <i style="transform: scaleX({hud.stamina / maxStamina()})"></i>
        </div>
      </div>

      <!-- Крестовина снаряжения: щит слева, меч справа, фляга внизу — с
           числом глотков, как в первоисточнике. -->
      <div class="kit" aria-label="Снаряжение: эстус {hud.estus} из {maxEstus()}">
        <span class="slot up" title="пусто"></span>
        <span class="slot left" title="щит · L / ПКМ">
          <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M10 2l6 2.4v5.2c0 4.1-2.7 6.8-6 8.4-3.3-1.6-6-4.3-6-8.4V4.4z" /></svg>
        </span>
        <span class="slot right" title="меч · J / ЛКМ">
          <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M15.5 2.5l2 2-9 9-2-2zM5.6 12.6l1.8 1.8-2.4 2.4-1.8-1.8zM4 15.2l.8.8-1.6 1.6-.8-.8z" /></svg>
        </span>
        <span class="slot down" class:dry={hud.estus === 0} title="эстус · R">
          <svg viewBox="0 0 12 16" aria-hidden="true">
            <path d="M4.4 1h3.2v3.6l3 5.9a2.3 2.3 0 0 1-2 3.4H3.4a2.3 2.3 0 0 1-2-3.4l3-5.9z" />
          </svg>
          <b>{hud.estus}</b>
        </span>
      </div>

      {#if hud.awake && hud.phase !== 'завеса'}
        <div class="boss" aria-hidden="true">
          <span class="name">Судия Гундир{hud.bossPhase === 2 ? ' · бездна' : ''}</span>
          <div class="bar boss-hp">
            <i class="ghost" style="transform: scaleX({hud.bossGhost})"></i>
            <i style="transform: scaleX({hud.boss})"></i>
          </div>
        </div>
      {/if}

      {#if hud.flash}<div class="flash">{hud.flash}</div>{/if}

      <div class="keys" aria-hidden="true">
        <span><b>WASD</b> ход</span>
        <span><b>пробел</b> перекат</span>
        <span><b>J</b> удар</span>
        <span><b>K</b> тяжёлый</span>
        <span><b>L</b> блок</span>
        <span><b>I</b> парировать</span>
        <span><b>R</b> глоток</span>
        <span><b>Shift</b> бег</span>
        <span class:off={!lockOn}><b>Q</b> захват {lockOn ? 'вкл' : 'выкл'}</span>
      </div>

      {#if hud.phase === 'завеса'}
        <div class="veil fog">
          <b>Туманная завеса</b>
          <span>За ней на коленях сидит судия. <kbd>W</kbd> — войти.</span>
        </div>
      {:else if hud.phase === 'смерть' || hud.phase === 'победа'}
        <div class="veil end" class:victory={hud.phase === 'победа'}>
          <b>{hud.phase === 'победа' ? 'ВРАГ ПОВЕРЖЕН' : 'ВЫ ПОГИБЛИ'}</b>
          <span>попытка {hud.attempt}</span>
          <div class="row">
            <button class="ghost-btn" onclick={retry}>
              {hud.phase === 'победа' ? 'ещё раз' : 'к костру'}
            </button>
            {#if !bare}<button class="ghost-btn" onclick={onback}>в зал</button>{/if}
          </div>
        </div>
      {/if}
    </div>
  </div>
{/snippet}

{#if bare}
  <!-- Отдельное окно: бой занимает его целиком, рамы зала вокруг нет -->
  {@render arena()}
{:else}
  <Screen
    title="Судия Гундир"
    hint="перекат уходит от всего · пунктирный замах парируется"
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
    width: min(100%, 58rem);
  }
  /* В отдельном окне бой занимает всё, а арена вписывается по меньшей стороне */
  .arena.bare {
    width: 100vw;
    height: 100vh;
    display: grid;
    place-items: center;
    background: #050505;
  }
  .frame {
    position: relative;
    width: 100%;
    aspect-ratio: 920 / 600;
    border: 1px solid var(--edge);
    border-radius: 16px;
    overflow: hidden;
    background: #0d0b10;
    box-shadow: 0 30px 60px -40px rgba(0, 0, 0, 0.95);
    user-select: none;
  }
  .arena.bare .frame {
    width: min(100vw, calc(100vh * 920 / 600));
    border: 0;
    border-radius: 0;
    box-shadow: none;
  }
  canvas {
    display: block;
    width: 100%;
    height: 100%;
  }

  /* Полосы двигаются transform'ом, а не шириной: шестьдесят раз в секунду
     раскладка на ровном месте — лишняя работа. */
  .bar {
    position: relative;
    border: 1px solid rgba(0, 0, 0, 0.8);
    background: rgba(0, 0, 0, 0.62);
    overflow: hidden;
  }
  .bar i {
    position: absolute;
    inset: 0;
    transform-origin: left center;
  }
  .bar i.ghost {
    background: rgba(255, 255, 255, 0.28);
  }

  .mine {
    position: absolute;
    top: 4%;
    left: 3%;
    display: grid;
    gap: 5px;
    width: 36%;
  }
  .hp {
    height: 0.62rem;
  }
  .hp i:not(.ghost) {
    background: linear-gradient(90deg, #9a9a9a, #ffffff);
  }
  .stamina {
    width: 72%;
    height: 0.42rem;
  }
  .stamina i {
    background: repeating-linear-gradient(-45deg, #8a8a8a 0 4px, #616161 4px 8px);
  }

  /* Крестовина: четыре ромба, как у первоисточника, — только без цвета. */
  .kit {
    position: absolute;
    left: 3.5%;
    bottom: 5%;
    display: grid;
    grid-template-columns: repeat(3, 2.3rem);
    grid-template-rows: repeat(3, 2.3rem);
    place-items: center;
  }
  .slot {
    position: relative;
    display: grid;
    place-items: center;
    width: 2.2rem;
    height: 2.2rem;
    border: 1px solid rgba(255, 255, 255, 0.35);
    background: rgba(0, 0, 0, 0.55);
    transform: rotate(45deg) scale(0.78);
  }
  .slot svg {
    width: 1.15rem;
    height: 1.15rem;
    fill: #e8e8e8;
    transform: rotate(-45deg);
  }
  .slot.up {
    grid-area: 1 / 2;
    opacity: 0.45;
  }
  .slot.left {
    grid-area: 2 / 1;
  }
  .slot.right {
    grid-area: 2 / 3;
  }
  .slot.down {
    grid-area: 3 / 2;
    border-color: rgba(255, 255, 255, 0.7);
  }
  .slot.down b {
    position: absolute;
    right: -0.75rem;
    bottom: 0.15rem;
    transform: rotate(-45deg);
    font-size: 0.78rem;
    font-weight: 700;
    color: #ffffff;
    text-shadow: 0 1px 3px #000;
  }
  .slot.dry svg {
    fill: none;
    stroke: #e8e8e8;
    stroke-width: 1.1;
    opacity: 0.4;
  }

  .boss {
    position: absolute;
    left: 50%;
    bottom: 6%;
    transform: translateX(-50%);
    width: 62%;
    display: grid;
    gap: 4px;
  }
  .boss .name {
    font-size: 0.74rem;
    letter-spacing: 0.08em;
    color: #e8e8e8;
    text-shadow: 0 2px 8px #000;
  }
  .boss-hp {
    height: 0.5rem;
  }
  .boss-hp i:not(.ghost) {
    background: linear-gradient(90deg, #bdbdbd, #ffffff);
  }

  .flash {
    position: absolute;
    left: 50%;
    top: 22%;
    transform: translateX(-50%);
    letter-spacing: 0.3em;
    text-transform: uppercase;
    font-size: 0.86rem;
    color: #ffffff;
    text-shadow: 0 2px 10px #000;
    animation: flash-in 160ms ease-out;
  }
  @keyframes flash-in {
    from {
      opacity: 0;
      letter-spacing: 0.5em;
    }
  }

  .keys {
    position: absolute;
    right: 2.5%;
    top: 4%;
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 3px 9px;
    max-width: 34%;
    font-size: 0.62rem;
    color: var(--muted);
  }
  .keys b {
    color: #e8e8e8;
    font-weight: 600;
  }
  .keys .off {
    opacity: 0.45;
  }

  .veil {
    position: absolute;
    inset: 0;
    display: grid;
    place-content: center;
    justify-items: center;
    gap: 0.9rem;
    text-align: center;
    background: rgba(5, 5, 5, 0.72);
  }
  .veil.fog {
    background: radial-gradient(ellipse at center, rgba(220, 220, 220, 0.16), rgba(5, 5, 5, 0.78) 70%);
    backdrop-filter: blur(3px);
  }
  .veil b {
    font-size: clamp(1.4rem, 1rem + 2.4vw, 2.6rem);
    letter-spacing: 0.24em;
    font-weight: 400;
    color: #b8b8b8;
  }
  /* «Вы погибли» — тусклым, «враг повержен» — в полный свет: разница читается
     без цвета, как и всё остальное в этой системе. */
  .veil.end {
    animation: fade 1.2s ease-out;
  }
  .veil.end b {
    color: #8b8b8b;
    text-shadow: 0 0 30px rgba(0, 0, 0, 0.9);
  }
  .veil.victory b {
    color: #ffffff;
    text-shadow: 0 0 30px rgba(255, 255, 255, 0.35);
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  .veil span {
    color: var(--muted);
    font-size: 0.86rem;
  }
  .veil kbd {
    padding: 0 0.3rem;
    border: 1px solid var(--edge);
    border-radius: 4px;
    font-family: inherit;
    color: #e8e8e8;
  }
  .row {
    display: flex;
    gap: 0.6rem;
  }
  .ghost-btn {
    padding: 0.4rem 0.9rem;
    border: 1px solid var(--edge);
    border-radius: 8px;
    font-size: 0.84rem;
    color: var(--paper);
    background: rgba(0, 0, 0, 0.35);
    transition: border-color 160ms ease, background 160ms ease;
  }
  .ghost-btn:hover {
    border-color: var(--paper);
    background: var(--lift-2);
  }

  @media (prefers-reduced-motion: reduce) {
    .flash,
    .veil.end {
      animation: none;
    }
  }
</style>
