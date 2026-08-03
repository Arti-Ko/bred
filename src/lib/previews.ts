// Показ картинок в ленте.
//
// Файл отдаёт само приложение по адресу `bredfile://…`, и браузер грузит его
// как обычную картинку. Раньше байты ехали ответом команды — на десятках
// мегабайт это рвало мост в ядро, и интерфейс писал «connection lost», после
// чего переставало работать вообще всё.

import { api, type AttachmentRow, type Id } from './ipc';

/** Что показываем встроенно. GIF в этот список входит и анимируется сам. */
const VIEWABLE = ['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/svg+xml'];

/** Картинки меньше этого размера тянем сами, не дожидаясь клика. */
const AUTO_FETCH_LIMIT = 2 * 1024 * 1024;

/** Адреса не меняются — содержимое адресуется хешем, поэтому кешируем навсегда. */
const cache = new Map<Id, string>();

export function isViewable(file: { mime: string }): boolean {
  return VIEWABLE.includes(file.mime);
}

export function shouldAutoFetch(file: AttachmentRow): boolean {
  return isViewable(file) && !file.local && file.size <= AUTO_FETCH_LIMIT;
}

/**
 * Идущие сейчас загрузки: иначе один и тот же файл тянулся бы по разу на
 * каждый компонент, который его показывает.
 */
const inflight = new Map<Id, Promise<string | null>>();

/**
 * Адрес картинки. Если файла ещё нет — сначала забираем его у того, у кого он
 * есть, и только потом отдаём адрес.
 *
 * Раньше адрес отдавался сразу: у получателя файл ещё не был скачан, браузер
 * получал 404 и рисовал вопросительный знак — навсегда, повторять было некому.
 */
export async function previewUrl(hash: Id, space: Id | null): Promise<string | null> {
  const ready = cache.get(hash);
  if (ready) return ready;
  if (!space) return null;

  const running = inflight.get(hash);
  if (running) return running;

  const task = (async () => {
    try {
      const url = await api.ensureAttachment(space, hash);
      cache.set(hash, url);
      return url;
    } catch {
      // Ещё не у кого забрать — попробуем снова, когда человек появится.
      return null;
    } finally {
      inflight.delete(hash);
    }
  })();

  inflight.set(hash, task);
  return task;
}

export function forgetPreviews(): void {
  cache.clear();
}
