// Показ картинок прямо в ленте.
//
// Байты берём у ядра и заворачиваем в object URL. Кеш по хешу обязателен:
// без него каждая перерисовка ленты тянула бы файл заново, а перерисовок при
// живой переписке десятки в минуту.

import { api, type AttachmentRow, type Id } from './ipc';

/** Что показываем встроенно. GIF в этот список входит и анимируется сам. */
const VIEWABLE = ['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/svg+xml'];

/** Картинки меньше этого размера тянем сами, не дожидаясь клика. */
const AUTO_FETCH_LIMIT = 2 * 1024 * 1024;

const cache = new Map<Id, string>();
const inflight = new Map<Id, Promise<string | null>>();

export function isViewable(file: { mime: string }): boolean {
  return VIEWABLE.includes(file.mime);
}

export function shouldAutoFetch(file: AttachmentRow): boolean {
  return isViewable(file) && !file.local && file.size <= AUTO_FETCH_LIMIT;
}

/** Ссылка на уже скачанную картинку. `null`, если показывать нечего. */
export async function previewUrl(hash: Id): Promise<string | null> {
  const ready = cache.get(hash);
  if (ready) return ready;

  const running = inflight.get(hash);
  if (running) return running;

  const task = (async () => {
    try {
      const bytes = await api.attachmentBytes(hash);
      const url = URL.createObjectURL(new Blob([bytes]));
      cache.set(hash, url);
      return url;
    } catch {
      // Файла ещё нет на диске или он слишком большой — молча без превью.
      return null;
    } finally {
      inflight.delete(hash);
    }
  })();

  inflight.set(hash, task);
  return task;
}

/** Сбросить кеш: вызывается при выходе из пространства, иначе URL текут. */
export function forgetPreviews(): void {
  for (const url of cache.values()) URL.revokeObjectURL(url);
  cache.clear();
}
