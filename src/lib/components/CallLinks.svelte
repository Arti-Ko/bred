<script lang="ts">
  // Окно «Связь»: схема звонка — кто до кого как дотягивается.
  //
  // Сверху вывод словами — на чьей стороне беда. Ниже своя связь с каждым:
  // вы → путь (напрямую или через ретранслятор) → собеседник, с задержкой и
  // потерями. Ещё ниже — как связаны остальные между собой: это они сами
  // присылают в пингах, и только по этому видно, у кого проблема.

  import Avatar from '../adequate/Avatar.svelte';
  import Dialog from '../adequate/Dialog.svelte';
  import Icon from '../adequate/Icon.svelte';
  import { api, errorText, type Id, type LinkReport } from '../ipc';
  import { GRADE_TEXT, lines, verdict, type Grade, type Line } from '../linkquality';
  import { call } from '../stores/call.svelte';
  import { session } from '../stores/session.svelte';

  interface Props {
    onclose: () => void;
  }
  const { onclose }: Props = $props();

  /** Как часто спрашивать ядро: замеры там тоже раз в секунду. */
  const REFRESH_MS = 1000;

  let report = $state<LinkReport | null>(null);
  let error = $state('');

  $effect(() => {
    let alive = true;
    const pull = async (): Promise<void> => {
      try {
        const fresh = await api.callLinks();
        if (alive) {
          report = fresh;
          error = '';
        }
      } catch (issue) {
        if (alive) error = errorText(issue);
      }
    };
    void pull();
    const timer = setInterval(() => void pull(), REFRESH_MS);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  });

  const present = $derived(call.participants.map((p) => p.id));
  const all = $derived(report ? lines(report, session.me, present) : []);
  const mine = $derived(
    present
      .filter((id) => id !== session.me)
      .map((id) => all.find((line) => (line.a === session.me && line.b === id) || (line.b === session.me && line.a === id)))
      .filter((line): line is Line => Boolean(line)),
  );
  const between = $derived(all.filter((line) => line.a !== session.me && line.b !== session.me));
  /** Кто не присылает отчётов — у него версия без них, его сторону не видно. */
  const silent = $derived(
    report
      ? present.filter((id) => id !== session.me && !report!.others.some((other) => other.from === id))
      : [],
  );
  const summary = $derived(verdict(all, session.me, present, nick));

  function nick(id: Id): string {
    if (id === session.me) return 'вы';
    return session.members.find((m) => m.id === id)?.nick ?? id.slice(0, 8);
  }

  /** Тот конец линии, что не я. */
  function far(line: Line): Id {
    return line.a === session.me ? line.b : line.a;
  }
</script>

{#snippet route(from: Id, to: Id, line: Line)}
  <div class="route" class:culprit={summary.culprit === to || summary.culprit === from}>
    <div class="node">
      {#if from === session.me}
        <span class="box"><Icon name="laptop" size={16} /></span>
      {:else}
        <Avatar id={from} size="sm" />
      {/if}
      <span class="who">{nick(from)}</span>
    </div>
    <span class="wire {line.grade}"></span>
    <div class="hop" title={line.relay ? 'Пакеты идут через сервер-ретранслятор' : 'Пакеты идут напрямую'}>
      <span class="box"><Icon name={line.grade === 'none' ? 'close' : line.relay ? 'server' : 'direct'} size={14} /></span>
      <span class="who">{line.grade === 'none' ? 'нет пути' : line.relay ? 'через сервер' : 'напрямую'}</span>
    </div>
    <span class="wire {line.grade}"></span>
    <div class="node">
      <Avatar id={to} size="sm" />
      <span class="who">{nick(to)}</span>
    </div>
    <div class="metric {line.grade}">
      {#if line.grade === 'none'}
        <b>—</b>
        <span>не соединились</span>
      {:else}
        <b>{line.rtt} мс</b>
        <span>{line.loss > 0 ? `${line.loss}% потерь · ` : ''}{GRADE_TEXT[line.grade]}</span>
      {/if}
    </div>
  </div>
{/snippet}

{#snippet dot(grade: Grade)}
  <span class="dot {grade}" aria-hidden="true"></span>
{/snippet}

<Dialog title="Связь в звонке" {onclose} wide>
  <div class="links">
    <div class="verdict {summary.grade}" role="status">
      {@render dot(summary.grade)}
      <b>{summary.text}</b>
    </div>

    <section>
      <h4>Ваша связь</h4>
      {#each mine as line (far(line))}
        {@render route(session.me, far(line), line)}
      {:else}
        <p class="hint-text">{report ? 'В звонке пока никого.' : 'Меряем…'}</p>
      {/each}
    </section>

    {#if between.length > 0}
      <section>
        <h4>Между остальными</h4>
        {#each between as line (`${line.a}|${line.b}`)}
          {@render route(line.a, line.b, line)}
        {/each}
      </section>
    {/if}

    {#if silent.length > 0}
      <p class="hint-text">
        {silent.map(nick).join(', ')} — на старой версии БРЕДа: их связь с остальными не видна,
        только с вами.
      </p>
    {/if}
    {#if error}<p class="hint-text">Не удалось спросить ядро: {error}</p>{/if}

    <footer class="legend">
      <span>{@render dot('good')} до 150 мс</span>
      <span>{@render dot('fair')} до 300 мс</span>
      <span>{@render dot('bad')} дольше или потери</span>
      <span class="sp"></span>
      <span>обновляется каждую секунду</span>
    </footer>
  </div>
</Dialog>

<style>
  /* Цвет здесь — содержание: по нему ищут плохую линию, не читая цифр.
     Форма дублирует его — у плохой линии пунктир. */
  .links {
    --q-good: #7bd393;
    --q-fair: #e6bd5c;
    --q-bad: #f07467;
    --q-none: var(--fg-dimmer);
    height: 100%;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 18px;
    padding: 14px 18px 16px;
    border-top: 1px solid var(--line);
  }

  .verdict {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 14px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--bg);
  }
  .verdict b {
    color: var(--fg-hi);
    font-weight: 600;
  }
  .verdict.bad,
  .verdict.none {
    border-color: color-mix(in srgb, var(--q-bad) 45%, var(--line));
  }

  section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  h4 {
    margin: 0;
    font-size: 11px;
    letter-spacing: 0.16em;
    text-transform: uppercase;
    color: var(--fg-dimmer);
    font-weight: 600;
  }

  .route {
    display: grid;
    grid-template-columns: 5.5rem 1fr 6.5rem 1fr 5.5rem 7.5rem;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    border: 1px solid var(--line);
    border-radius: var(--radius-sm);
    background: var(--bg);
  }
  .route.culprit {
    border-color: color-mix(in srgb, var(--q-bad) 45%, var(--line));
  }
  .node,
  .hop {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    min-width: 0;
  }
  .box {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--bg-raised);
    color: var(--fg-hi);
  }
  .who {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--text-xs);
    color: var(--fg-dim);
  }

  /* Провод: бегущий штрих показывает, что пакеты идут, а цвет — как. */
  .wire {
    height: 2px;
    border-radius: 2px;
    background: repeating-linear-gradient(90deg, currentColor 0 10px, transparent 10px 16px);
    background-size: 16px 2px;
    animation: flow 0.9s linear infinite;
  }
  .wire.good {
    color: var(--q-good);
    background: currentColor;
    animation: none;
  }
  .wire.fair {
    color: var(--q-fair);
  }
  .wire.bad {
    color: var(--q-bad);
    animation-duration: 2.4s;
  }
  .wire.none {
    color: var(--q-none);
    animation: none;
    opacity: 0.5;
  }
  @keyframes flow {
    to {
      background-position: 16px 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .wire {
      animation: none;
    }
  }

  .metric {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 2px;
    text-align: right;
  }
  .metric b {
    font-variant-numeric: tabular-nums;
    font-size: 15px;
    font-weight: 650;
  }
  .metric span {
    font-size: var(--text-xs);
    color: var(--fg-dimmer);
  }
  .metric.good b {
    color: var(--q-good);
  }
  .metric.fair b {
    color: var(--q-fair);
  }
  .metric.bad b {
    color: var(--q-bad);
  }
  .metric.none b {
    color: var(--q-none);
  }

  .dot {
    display: inline-block;
    flex: none;
    width: 9px;
    height: 9px;
    border-radius: 50%;
    vertical-align: middle;
  }
  .dot.good {
    background: var(--q-good);
  }
  .dot.fair {
    background: var(--q-fair);
  }
  .dot.bad {
    background: var(--q-bad);
  }
  .dot.none {
    background: var(--q-none);
  }

  .legend {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 14px;
    margin-top: auto;
    font-size: var(--text-xs);
    color: var(--fg-dimmer);
  }
  .legend .sp {
    flex: 1;
  }

  @media (max-width: 640px) {
    .route {
      grid-template-columns: 4rem 1fr 5rem 1fr 4rem;
    }
    .metric {
      grid-column: 1 / -1;
      flex-direction: row;
      justify-content: space-between;
      align-items: baseline;
    }
  }
</style>
