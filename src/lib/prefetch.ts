// Фоновая подкачка файлов: аватары, эмодзи, картинки в ленте.
//
// Раньше интерфейс тянул их подряд, дожидаясь каждого: `await` в цикле по
// всем эмодзи, потом по всем аватарам, потом по всем вложениям страницы. Один
// человек не в сети — и каждая его картинка стоила таймаута, а весь цикл
// перезапускался на каждой пачке входящих событий. Отсюда и «сообщения доходят
// очень долго»: они приезжали вовремя, но нарисовать их было некому.
//
// Здесь три правила: одну и ту же картинку не просим дважды, просим несколько
// штук разом, и никогда не заставляем интерфейс ждать результата.

import { api, type Id } from './ipc';

/** Сколько файлов тянем одновременно. Больше — только толкаемся в одном канале. */
const PARALLEL = 4;

/** Через сколько разрешаем повторить неудачную попытку, миллисекунды. */
const RETRY_AFTER = 30_000;

/** Что уже получено или получается прямо сейчас. */
const done = new Set<Id>();
/** Когда в последний раз не получилось. Раньше срока не пробуем снова. */
const failed = new Map<Id, number>();

const queue: Array<{ space: Id; hash: Id }> = [];
let running = 0;

/**
 * Попросить файл в фоне. Ничего не возвращает и никого не ждёт: как только
 * байты лягут на диск, картинка появится сама — компоненты спрашивают адрес
 * отдельно.
 */
export function prefetch(space: Id, hash: Id): void {
  if (done.has(hash)) return;

  const stumbled = failed.get(hash);
  if (stumbled !== undefined && Date.now() - stumbled < RETRY_AFTER) return;

  done.add(hash);
  queue.push({ space, hash });
  pump();
}

/** Попросить сразу пачку — порядок сохраняется, ожидание не накапливается. */
export function prefetchAll(space: Id, hashes: Iterable<Id>): void {
  for (const hash of hashes) prefetch(space, hash);
}

function pump(): void {
  while (running < PARALLEL && queue.length > 0) {
    const next = queue.shift()!;
    running += 1;
    void api
      .downloadAttachment(next.space, next.hash)
      .catch(() => {
        // Не вышло — забываем, чтобы через полминуты попробовать снова.
        // Держателя может просто не быть в сети именно сейчас.
        done.delete(next.hash);
        failed.set(next.hash, Date.now());
      })
      .finally(() => {
        running -= 1;
        pump();
      });
  }
}

/** Смена личности или каталога данных — прошлые отметки больше не про нас. */
export function forgetPrefetched(): void {
  done.clear();
  failed.clear();
  queue.length = 0;
}
