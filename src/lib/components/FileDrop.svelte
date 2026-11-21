<script lang="ts">
  // Файлы в чат без окна выбора: вставкой из буфера и перетаскиванием в окно.
  //
  // Одно на оба оформления — подключается в корне приложения. Перетаскивание
  // ловит сам Tauri: он отдаёт настоящие пути, а HTML-событие drop в его окне
  // не приходит вовсе. У вставки пути нет, только содержимое, — его ядро
  // принимает байтами.

  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { session } from '../stores/session.svelte';

  let hovering = $state(false);

  $effect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    let webview: ReturnType<typeof getCurrentWebview>;
    try {
      webview = getCurrentWebview();
    } catch {
      return; // вне Tauri, например в демо, перетаскивания просто нет
    }
    void webview
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === 'enter' || payload.type === 'over') {
          hovering = true;
        } else if (payload.type === 'drop') {
          hovering = false;
          if (payload.paths.length > 0) void session.attachPaths(payload.paths);
        } else {
          hovering = false;
        }
      })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  });

  /**
   * Вставка с файлами. Текст не трогаем — его вставит поле ввода, как всегда.
   * Но если в буфере файл, Finder кладёт рядом и его имя текстом: тогда
   * прикрепляем файл, а имя в поле не пускаем.
   */
  function onPaste(event: ClipboardEvent): void {
    const data = event.clipboardData;
    if (!data) return;
    const files = [...data.files];
    if (files.length === 0) {
      for (const item of data.items) {
        if (item.kind !== 'file') continue;
        const file = item.getAsFile();
        if (file) files.push(file);
      }
    }
    if (files.length === 0) return;
    event.preventDefault();
    void session.attachFiles(files);
  }
</script>

<svelte:window onpaste={onPaste} />

{#if hovering}
  <div class="drop" aria-hidden="true">
    <span>{session.spaceId ? 'отпустите — прикрепим к сообщению' : 'сначала выберите пространство'}</span>
  </div>
{/if}

<style>
  .drop {
    position: fixed;
    inset: 8px;
    z-index: 200;
    display: grid;
    place-items: center;
    border: 1px dashed var(--fg-dim);
    background: rgba(0, 0, 0, 0.6);
    pointer-events: none;
  }
  .drop span {
    color: var(--fg-hi);
    font-size: var(--text-sm);
    letter-spacing: 0.08em;
  }
</style>
