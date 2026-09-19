<script lang="ts">
  // Первый запуск: имя и два пути, которые вообще существуют.
  //
  // Раньше на это требовалось знать три команды подряд — /имя, /простор и
  // /войти. Теперь экран спрашивает сам.

  import Icon from './Icon.svelte';
  import { errorText } from '../ipc';
  import { session } from '../stores/session.svelte';

  interface Props {
    onnewspace: () => void;
    onjoin: () => void;
  }
  const { onnewspace, onjoin }: Props = $props();

  let nick = $state(session.nick);
  let saved = $state(false);
  let error = $state('');

  async function save(): Promise<void> {
    error = '';
    try {
      await session.rename(nick);
      saved = true;
      setTimeout(() => (saved = false), 1800);
    } catch (issue) {
      error = errorText(issue);
    }
  }
</script>

<div class="welcome">
  <div class="card">
    <h2>Здравствуйте. Как вас зовут?</h2>
    <p>
      Регистрации нет: имя увидят только те, кого вы позовёте сами. Поменять его можно в любой
      момент — в профиле слева внизу.
    </p>

    <div class="name">
      <span class="field-label">Ваше имя</span>
      <div class="row">
        <input
          class="input"
          bind:value={nick}
          placeholder="как к вам обращаться"
          onkeydown={(event) => event.key === 'Enter' && save()}
        />
        <button class="btn" disabled={!nick.trim() || nick === session.nick} onclick={save}>
          {saved ? 'Сохранено' : 'Сохранить'}
        </button>
      </div>
      {#if error}<p class="error">{error}</p>{/if}
    </div>

    <div class="choices">
      <button class="choice lit" onclick={onnewspace}>
        <Icon name="home" size={20} />
        <b>Создать пространство</b>
        <span>Своё место для компании или команды: каналы, файлы и звонки внутри.</span>
        <span class="go">Создать →</span>
      </button>
      <button class="choice" onclick={onjoin}>
        <Icon name="link" size={20} />
        <b>Войти по ссылке</b>
        <span>Вам прислали ссылку вида bred://… — вставьте её, и вы внутри.</span>
        <span class="go">Вставить ссылку →</span>
      </button>
    </div>
  </div>
</div>

<style>
  .welcome {
    display: grid;
    place-items: center;
    min-height: 0;
    padding: clamp(1rem, 0.5rem + 3vw, 3rem);
    overflow-y: auto;
  }
  .card {
    width: min(46rem, 100%);
    display: flex;
    flex-direction: column;
    gap: 1.25rem;
    text-align: center;
    align-items: center;
  }
  h2 {
    margin: 0;
    font-size: clamp(1.4rem, 1rem + 1.6vw, 2rem);
    letter-spacing: -0.02em;
    color: var(--fg-hi);
    font-weight: 650;
  }
  p {
    margin: 0;
    max-width: 34rem;
    color: var(--fg-dim);
  }

  .name {
    width: min(26rem, 100%);
    text-align: left;
  }
  .row {
    display: flex;
    gap: 8px;
  }
  .error {
    margin-top: 6px;
    color: var(--fg-hi);
    font-size: var(--text-sm);
  }

  .choices {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(15rem, 1fr));
    gap: 12px;
    width: 100%;
  }
  .choice {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 18px;
    border: 1px solid var(--line);
    border-radius: var(--radius);
    background: var(--bg-raised);
    text-align: left;
    color: var(--fg-dim);
    transition: border-color var(--fast) var(--ease), transform var(--fast) var(--ease);
  }
  .choice:hover {
    border-color: var(--fg-dim);
    transform: translateY(-2px);
  }
  .choice.lit {
    border-color: var(--edge);
    background: linear-gradient(160deg, var(--lift), var(--bg-raised) 70%);
  }
  .choice b {
    color: var(--fg-hi);
    font-size: 15px;
    font-weight: 620;
  }
  .choice span {
    font-size: var(--text-sm);
  }
  .go {
    margin-top: 8px;
    padding-top: 10px;
    border-top: 1px solid var(--line);
    color: var(--fg-hi);
  }
</style>
