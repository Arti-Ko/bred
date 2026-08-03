<script lang="ts">
  // Полоса команд над полем ввода.
  //
  // Раньше команды были только в подсказках текстом: их нужно было запомнить и
  // набрать вручную. Теперь по клику команда подставляется в поле — остаётся
  // дописать аргумент.

  interface Props {
    onpick: (command: string) => void;
  }

  const { onpick }: Props = $props();

  interface Item {
    command: string;
    hint: string;
    /** Команда без аргумента срабатывает сразу — подставлять пробел незачем. */
    instant?: boolean;
  }

  const ITEMS: Item[] = [
    { command: '/простор', hint: 'создать пространство' },
    { command: '/канал', hint: 'создать канал' },
    { command: '/голос', hint: 'голосовой канал' },
    { command: '/позвать', hint: 'ссылка-приглашение', instant: true },
    { command: '/визитка', hint: 'ссылка для связи один на один', instant: true },
    { command: '/войти', hint: 'подключиться по ссылке' },
    { command: '/лс', hint: 'написать лично' },
    { command: '/имя', hint: 'сменить имя' },
    { command: '/аватар', hint: 'картинка профиля', instant: true },
    { command: '/файл', hint: 'прикрепить файл', instant: true },
    { command: '/эмодзи', hint: 'свой эмодзи' },
    { command: '/стикер', hint: 'стикер' },
    { command: '/экран', hint: 'показать экран', instant: true },
    { command: '/обновление', hint: 'проверить обновления', instant: true },
    { command: '/покинуть', hint: 'выйти из пространства', instant: true },
  ];
</script>

<nav class="bar" aria-label="Команды">
  {#each ITEMS as item (item.command)}
    <button title={item.hint} onclick={() => onpick(item.instant ? item.command : `${item.command} `)}>
      {item.command}
    </button>
  {/each}
</nav>

<style>
  .bar {
    display: flex;
    gap: var(--gap-2);
    padding: 4px var(--gap-4);
    border-top: 1px solid var(--line);
    overflow-x: auto;
    scrollbar-width: none;
  }
  .bar::-webkit-scrollbar {
    display: none;
  }

  button {
    flex: none;
    border: 1px solid var(--line);
    color: var(--fg-dimmer);
    padding: 1px 8px;
    font-size: var(--text-xs);
    white-space: nowrap;
    transition: color var(--fast) var(--ease), border-color var(--fast) var(--ease);
  }
  button:hover {
    color: var(--fg-hi);
    border-color: var(--fg-dim);
  }
</style>
