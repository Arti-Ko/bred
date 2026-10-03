<script lang="ts">
  // «Сообщить о проблеме»: вид, заголовок, описание, фото или видео — и в
  // Telegram разработчика. Одна форма на оба оформления: она поверх всего и
  // нужна редко, а выглядеть должна одинаково понятно.
  import { prefs } from '../stores/prefs.svelte';
  import { report } from '../stores/report.svelte';

  function size(bytes: number): string {
    if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} КБ`;
    return `${(bytes / 1024 / 1024).toFixed(1)} МБ`;
  }

  const asWhat: Record<string, string> = {
    photo: 'фото',
    video: 'видео',
    document: 'файлом',
  };
</script>

<svelte:window onkeydown={(event) => event.key === 'Escape' && report.open && report.close()} />

{#if report.open}
  <div class="scrim" role="presentation" onclick={() => report.close()}></div>
  <div class="sheet" class:soft={prefs.adequate} role="dialog" aria-label="Сообщить о проблеме">
    <header>
      <b>{prefs.adequate ? 'Сообщить о проблеме' : 'сообщить о проблеме'}</b>
      <button class="x" onclick={() => report.close()} aria-label="Закрыть">✕</button>
    </header>

    {#if report.sent}
      <div class="done" role="status">
        <b>Отправлено</b>
        <p>
          Номер отчёта <code class="selectable">{report.sent}</code>. Если понадобятся подробности,
          назовите его — по нему отчёт находится сразу.
        </p>
        <button class="primary" onclick={() => report.close()}>Готово</button>
      </div>
    {:else if report.info && !report.info.configured}
      <p class="note">
        В этой сборке отправка отчётов не настроена — ключ бота подставляется только в
        официальные выпуски. Напишите разработчику напрямую, указав ваш номер
        <code class="selectable">{report.info.number}</code>.
      </p>
    {:else}
      <form
        onsubmit={(event) => {
          event.preventDefault();
          void report.send();
        }}
      >
        <fieldset class="kinds">
          <legend>Что не так</legend>
          {#each report.info?.kinds ?? [] as kind (kind.code)}
            <label class="kind" class:on={report.kind === kind.code}>
              <input type="radio" name="kind" value={kind.code} bind:group={report.kind} />
              {kind.label}
            </label>
          {/each}
        </fieldset>

        <label class="field">
          <span>Заголовок</span>
          <input
            bind:value={report.title}
            maxlength="120"
            placeholder="Коротко: «пропадает звук через 10 минут»"
          />
        </label>

        <label class="field">
          <span>Что случилось</span>
          <textarea
            bind:value={report.body}
            rows="6"
            maxlength="4000"
            placeholder="Что делали, что ожидали, что получилось. Чем подробнее — тем быстрее починится."
          ></textarea>
        </label>

        <div class="files">
          <button type="button" class="ghost" onclick={() => report.pick()}>
            Прикрепить фото или видео
          </button>
          {#each report.files as file (file.path)}
            <div class="file" class:bad={!!file.problem}>
              <span class="name">{file.name}</span>
              <span class="meta">
                {file.problem ?? `${size(file.size)} · уйдёт ${asWhat[file.kind]}`}
              </span>
              <button type="button" class="x" onclick={() => report.drop(file.path)} aria-label="Убрать">
                ✕
              </button>
            </div>
          {/each}
        </div>

        <label class="check">
          <input type="checkbox" bind:checked={report.withLog} />
          Приложить журнал работы за последние минуты — без него сетевые проблемы почти не
          разобрать
        </label>

        <!-- Что именно уйдёт — честно и заранее: отчёт попадает в чат к человеку. -->
        <p class="note">
          Вместе с отчётом уйдут ваше имя, номер <code class="selectable">{report.info?.number ?? '…'}</code>,
          версия БРЕДа, система и состояние сети. Переписка и ключи не уходят никогда.
        </p>

        {#if report.error}<p class="error" role="alert">{report.error}</p>{/if}

        <div class="actions">
          <button type="button" class="ghost" onclick={() => report.close()}>Отмена</button>
          <button type="submit" class="primary" disabled={!report.ready || report.sending}>
            {report.sending ? 'Отправляем…' : 'Отправить'}
          </button>
        </div>
      </form>
    {/if}
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 130;
    background: rgba(0, 0, 0, 0.6);
  }

  .sheet {
    position: fixed;
    top: 50%;
    left: 50%;
    z-index: 131;
    transform: translate(-50%, -50%);
    width: min(34rem, calc(100vw - 2rem));
    max-height: calc(100vh - 2rem);
    overflow-y: auto;
    padding: 16px 18px 18px;
    background: var(--bg-raised);
    border: 1px solid var(--fg-faint);
    box-shadow: 0 30px 60px -30px rgba(0, 0, 0, 0.95);
  }
  .sheet.soft {
    border-color: var(--edge, var(--fg-faint));
    border-radius: 16px;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 12px;
  }
  header b {
    color: var(--fg-hi);
    font-size: var(--text-md, 15px);
  }
  .x {
    color: var(--fg-dimmer);
    padding: 2px 6px;
  }
  .x:hover {
    color: var(--fg-hi);
  }

  form {
    display: grid;
    gap: 12px;
  }

  .kinds {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin: 0;
    padding: 0;
    border: 0;
  }
  .kinds legend {
    margin-bottom: 6px;
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }
  .kind {
    padding: 4px 10px;
    border: 1px solid var(--fg-faint);
    color: var(--fg-dim);
    font-size: var(--text-sm);
    cursor: pointer;
    transition: border-color var(--fast) var(--ease), color var(--fast) var(--ease);
  }
  .soft .kind {
    border-radius: 999px;
  }
  .kind:hover {
    color: var(--fg-hi);
    border-color: var(--fg-dim);
  }
  .kind.on {
    background: var(--inv-bg);
    border-color: var(--inv-bg);
    color: var(--inv-fg);
    font-weight: 700;
  }
  .kind input {
    position: absolute;
    opacity: 0;
    pointer-events: none;
  }
  .kind:has(input:focus-visible) {
    outline: 2px solid var(--fg);
    outline-offset: 2px;
  }

  .field {
    display: grid;
    gap: 4px;
  }
  .field span {
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }
  .field input,
  .field textarea {
    width: 100%;
    padding: 7px 9px;
    border: 1px solid var(--fg-faint);
    background: var(--bg);
    color: var(--fg);
    font: inherit;
    font-size: var(--text-sm);
    resize: vertical;
  }
  .soft .field input,
  .soft .field textarea {
    border-radius: 8px;
  }
  .field input:focus,
  .field textarea:focus {
    outline: none;
    border-color: var(--fg-dim);
  }

  .files {
    display: grid;
    gap: 4px;
  }
  .file {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    border: 1px solid var(--line);
    font-size: var(--text-sm);
  }
  .file .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg);
  }
  .file .meta {
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
  }
  .file.bad {
    border-color: var(--fg-dim);
  }
  .file.bad .meta {
    color: var(--fg-hi);
    font-weight: 700;
  }

  .check {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    color: var(--fg-dim);
    font-size: var(--text-sm);
  }
  .check input {
    margin-top: 3px;
    accent-color: var(--fg);
  }

  .note {
    margin: 0;
    color: var(--fg-dimmer);
    font-size: var(--text-xs);
    line-height: 1.5;
  }
  code {
    color: var(--fg-hi);
  }
  .error {
    margin: 0;
    color: var(--fg-hi);
    font-weight: 700;
    font-size: var(--text-sm);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  button.primary,
  button.ghost {
    padding: 6px 14px;
    font-size: var(--text-sm);
    border: 1px solid var(--fg-faint);
  }
  .soft button.primary,
  .soft button.ghost {
    border-radius: 8px;
  }
  button.primary {
    background: var(--inv-bg);
    border-color: var(--inv-bg);
    color: var(--inv-fg);
    font-weight: 700;
  }
  button.primary:disabled {
    opacity: 0.4;
  }
  button.ghost {
    color: var(--fg-dim);
  }
  button.ghost:hover {
    color: var(--fg-hi);
    border-color: var(--fg-dim);
  }

  .done {
    display: grid;
    gap: 10px;
    justify-items: start;
  }
  .done b {
    color: var(--fg-hi);
    font-size: 18px;
  }
  .done p {
    margin: 0;
    color: var(--fg-dim);
    font-size: var(--text-sm);
  }
</style>
