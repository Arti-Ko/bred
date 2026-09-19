<script lang="ts">
  // Зал с играми — он же замок от адекватного режима.
  //
  // Сюда попадают из настроек, нажав кнопку «адекватный дизайн». Логика простая
  // и честная: другой вид интерфейса не выдаётся по просьбе, его выигрывают.

  import { untrack } from 'svelte';

  import { prefs, type Trophy } from '../stores/prefs.svelte';
  import Tictactoe from './tictactoe/Tictactoe.svelte';
  import Checkers from './checkers/Checkers.svelte';
  import Gundyr from './gundyr/Gundyr.svelte';

  type Screen = 'зал' | Trophy;

  interface Props {
    onclose: () => void;
    /** С чего открыться. Обычно с зала; игра — для снимков документации. */
    start?: Screen;
  }
  const { onclose, start = 'зал' }: Props = $props();

  // Только начальное значение: дальше экран живёт сам, а не следит за свойством.
  let screen = $state<Screen>(untrack(() => start));
  let won = $state<Trophy | null>(null);

  const games: { id: Trophy; title: string; line: string; about: string }[] = [
    {
      id: 'крестики',
      title: 'Крестики-нолики',
      line: 'поле 5×5, линия из четырёх',
      about:
        'Три на три разобраны до последнего хода — там не выигрывают, там в лучшем случае не проигрывают. Здесь поле больше, и выигрыш существует: нужна двойная угроза, на которую у противника один ответ.',
    },
    {
      id: 'шашки',
      title: 'Шашки',
      line: 'русские, с дамками и обязательным боем',
      about:
        'Полные правила: бить обязательно, цепочку доводят до конца, простая бьёт и назад, дамка ходит через всю доску. Противник считает на несколько ходов вперёд и разменов не боится.',
    },
    {
      id: 'гундир',
      title: 'Судия Гундир',
      line: 'первый босс, две фазы',
      about:
        'Перекат, выносливость, три глотка и окно после его связки. Он бьёт по площади, доворачивается медленно и на половине здоровья перестаёт быть человеком.',
    },
  ];

  /**
   * Победу записываем, но из игры не выгоняем.
   *
   * Раньше здесь же стоял возврат в зал — и получалось, что победного экрана
   * не существует: смена состояния игры и смена экрана попадают в один флаш
   * Svelte, и вместо собранной линии, добитого босса и вуали «ВРАГ ПОВЕРЖЕН»
   * человек мгновенно оказывался в меню. Уходит он теперь сам, посмотрев на
   * то, ради чего играл.
   */
  function finish(trophy: Trophy): void {
    prefs.win(trophy);
    won = trophy;
  }
</script>

<svelte:window
  onkeydown={(event) => {
    if (event.key !== 'Escape') return;
    event.stopPropagation();
    if (screen === 'зал') onclose();
    else screen = 'зал';
  }}
/>

<div class="arcade">
  {#if screen === 'зал'}
    <div class="hall">
      <header>
        <div class="crumbs">
          <span>настройки</span><span class="sep">/</span><span>оформление</span>
        </div>
        <h1>Адекватный дизайн</h1>
        <p class="lead">
          {#if prefs.unlocked}
            Замок открыт. Можно переключаться сколько угодно — или пройти оставшиеся две,
            раз уж всё равно начали.
          {:else}
            Заперт. Ключ — победа в любой из трёх: нолики не поддаются, шашки не прощают,
            судия не ждёт.
          {/if}
        </p>
        <button class="close" onclick={onclose} aria-label="Закрыть">закрыть ✕</button>
      </header>

      {#if won}
        <div class="banner">
          <b>Замок открыт.</b>
          <span>Адекватный режим включён — закройте зал, и увидите.</span>
          <button class="ghost" onclick={onclose}>посмотреть</button>
        </div>
      {/if}

      <div class="cards">
        {#each games as game, index (game.id)}
          <button class="card" class:beaten={prefs.trophies.includes(game.id)} onclick={() => (screen = game.id)}>
            <span class="ordinal">{index + 1}</span>
            <span class="art" aria-hidden="true">
              {#if game.id === 'крестики'}
                <svg viewBox="0 0 64 64">
                  <rect x="6" y="6" width="52" height="52" rx="5" />
                  <path d="M23 8 V56 M41 8 V56 M8 23 H56 M8 41 H56" />
                  <circle cx="32" cy="14" r="5" />
                  <circle cx="14" cy="32" r="5" />
                  <path class="ink" d="M10 10 l8 8 M18 10 l-8 8" />
                  <path class="ink" d="M28 28 l8 8 M36 28 l-8 8" />
                  <path class="ink" d="M46 46 l8 8 M54 46 l-8 8" />
                </svg>
              {:else if game.id === 'шашки'}
                <svg viewBox="0 0 64 64">
                  <ellipse cx="32" cy="48" rx="21" ry="7" />
                  <ellipse cx="32" cy="39" rx="21" ry="7" />
                  <ellipse class="ink" cx="32" cy="30" rx="21" ry="7" />
                  <path class="ink" d="M25 21 l7 -7 l7 7" />
                </svg>
              {:else}
                <svg viewBox="0 0 64 64">
                  <path d="M19 56 V30 a13 13 0 0 1 26 0 v26" />
                  <path d="M25 33 h14" />
                  <path class="ink" d="M8 58 L44 20" />
                  <path class="ink" d="M42 10 l12 12 l-8 8 l-12 -12 z" />
                </svg>
              {/if}
            </span>
            <span class="title">{game.title}</span>
            <span class="line">{game.line}</span>
            <span class="about">{game.about}</span>
            <span class="go">
              {#if prefs.trophies.includes(game.id)}пройдено · ещё раз{:else}играть{/if}
            </span>
          </button>
        {/each}
      </div>

      <footer>
        {#if prefs.unlocked}
          <span class="state">
            сейчас: <b>{prefs.adequate ? 'адекватный режим' : 'терминал'}</b>
          </span>
          <button class="ghost" onclick={() => prefs.toggleAdequate()}>
            {prefs.adequate ? 'вернуть терминал' : 'включить адекватный'}
          </button>
        {:else}
          <span class="state">трофеев нет — переключать нечего</span>
        {/if}
        <span class="hint">Esc — назад</span>
      </footer>
    </div>
  {:else if screen === 'крестики'}
    <Tictactoe onwin={() => finish('крестики')} onback={() => (screen = 'зал')} />
  {:else if screen === 'шашки'}
    <Checkers onwin={() => finish('шашки')} onback={() => (screen = 'зал')} />
  {:else}
    <Gundyr onwin={() => finish('гундир')} onback={() => (screen = 'зал')} />
  {/if}
</div>

<style>
  /* Палитра зала — та же монохромная, что и во всём БРЕД: ни одного цветового
     акцента, активное помечается светом. Переход из мессенджера в игру и обратно
     не выглядит переездом в другое приложение. */
  .arcade {
    --ink: #0a0a0a;
    --surface: #111111;
    --raised: #171717;
    --edge: #242424;
    --paper: #e9e9e9;
    --muted: #8b8b8b;
    --accent: #ffffff;
    --lift: rgba(255, 255, 255, 0.07);
    --lift-2: rgba(255, 255, 255, 0.13);

    position: fixed;
    inset: 0;
    z-index: 120;
    overflow: auto;
    color: var(--paper);
    font-family: ui-sans-serif, -apple-system, 'SF Pro Text', 'Segoe UI', Inter, system-ui,
      sans-serif;
    background:
      radial-gradient(1100px 620px at 78% -8%, rgba(255, 255, 255, 0.06), transparent 60%),
      var(--ink);
  }

  .hall {
    display: flex;
    flex-direction: column;
    min-height: 100vh;
    max-width: 68rem;
    margin: 0 auto;
    padding: clamp(1.5rem, 1rem + 3vw, 3.5rem) clamp(1rem, 0.6rem + 2.5vw, 2.5rem) 2rem;
  }

  header {
    position: relative;
    padding-bottom: 1.6rem;
  }

  .crumbs {
    display: flex;
    gap: 0.5rem;
    font-size: 0.72rem;
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--muted);
  }
  .sep {
    opacity: 0.45;
  }

  h1 {
    margin: 0.55rem 0 0;
    font-size: clamp(2.1rem, 1.3rem + 3.4vw, 3.6rem);
    line-height: 0.95;
    letter-spacing: -0.035em;
    font-weight: 680;
  }

  .lead {
    margin: 0.9rem 0 0;
    max-width: 34rem;
    color: var(--muted);
    font-size: 0.95rem;
    line-height: 1.55;
  }

  .close {
    position: absolute;
    top: 0;
    right: 0;
    color: var(--muted);
    font-size: 0.8rem;
    letter-spacing: 0.06em;
    padding: 0.35rem 0.6rem;
    border-radius: 6px;
    transition: color 140ms ease, background 140ms ease;
  }
  .close:hover {
    color: var(--paper);
    background: rgba(255, 255, 255, 0.06);
  }

  .banner {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 1.6rem;
    padding: 0.85rem 1.1rem;
    border: 1px solid rgba(255, 255, 255, 0.28);
    border-left: 3px solid var(--accent);
    border-radius: 10px;
    background: linear-gradient(90deg, var(--lift-2), transparent 70%);
    font-size: 0.92rem;
  }
  .banner span {
    color: var(--muted);
  }

  .cards {
    /* Заголовок сверху, подвал снизу, карточки — по центру того, что осталось */
    margin: auto 0;
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(17rem, 1fr));
    gap: 1.1rem;
  }

  .card {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 1.4rem 1.3rem 1.2rem;
    text-align: left;
    border: 1px solid var(--edge);
    border-radius: 14px;
    background: linear-gradient(170deg, var(--raised), var(--surface));
    transition:
      transform 220ms cubic-bezier(0.16, 1, 0.3, 1),
      border-color 220ms ease,
      box-shadow 220ms ease;
  }
  .card:hover,
  .card:focus-visible {
    transform: translateY(-4px);
    border-color: rgba(255, 255, 255, 0.35);
    box-shadow: 0 18px 40px -24px rgba(0, 0, 0, 0.9), 0 0 0 1px rgba(255, 255, 255, 0.08);
  }

  .ordinal {
    position: absolute;
    top: 0.7rem;
    right: 1rem;
    font-size: 3.4rem;
    line-height: 1;
    font-weight: 700;
    color: rgba(255, 255, 255, 0.05);
  }

  .art {
    width: 3.1rem;
    height: 3.1rem;
    margin-bottom: 0.4rem;
  }
  .art :global(svg) {
    width: 100%;
    height: 100%;
    fill: none;
    stroke: #3b3848;
    stroke-width: 2.4;
    stroke-linecap: round;
    stroke-linejoin: round;
    transition: stroke 200ms ease;
  }
  .art :global(.ink) {
    stroke: var(--paper);
  }
  .card:hover .art :global(svg) {
    stroke: var(--muted);
  }
  .card:hover .art :global(.ink) {
    stroke: var(--accent);
  }

  .title {
    font-size: 1.22rem;
    font-weight: 620;
    letter-spacing: -0.015em;
  }
  .line {
    font-size: 0.8rem;
    letter-spacing: 0.04em;
    color: var(--paper);
  }
  .about {
    color: var(--muted);
    font-size: 0.86rem;
    line-height: 1.5;
  }
  .go {
    margin-top: auto;
    padding-top: 0.9rem;
    font-size: 0.82rem;
    letter-spacing: 0.06em;
    color: var(--paper);
    border-top: 1px solid var(--edge);
  }
  .card.beaten .go {
    color: var(--accent);
  }
  .card.beaten::after {
    content: '★';
    position: absolute;
    top: 1.35rem;
    right: 1.1rem;
    font-size: 1rem;
    color: var(--accent);
  }

  footer {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.9rem;
    margin-top: auto;
    padding-top: 1.1rem;
    border-top: 1px solid var(--edge);
    font-size: 0.85rem;
    color: var(--muted);
  }
  .state b {
    color: var(--paper);
    font-weight: 600;
  }
  .hint {
    margin-left: auto;
    font-size: 0.78rem;
    opacity: 0.7;
  }

  .ghost {
    padding: 0.4rem 0.9rem;
    border: 1px solid var(--edge);
    border-radius: 8px;
    font-size: 0.84rem;
    color: var(--paper);
    transition: border-color 160ms ease, background 160ms ease;
  }
  .ghost:hover {
    border-color: var(--paper);
    background: var(--lift);
  }
</style>
