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

/** Адрес картинки, которая уже лежит на диске. */
export async function previewUrl(hash: Id): Promise<string | null> {
  const ready = cache.get(hash);
  if (ready) return ready;

  try {
    const url = await api.attachmentUrl(hash);
    cache.set(hash, url);
    return url;
  } catch {
    return null;
  }
}

export function forgetPreviews(): void {
  cache.clear();
}
