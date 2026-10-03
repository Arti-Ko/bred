// Входящие зовы в комнату.
//
// Зов эфемерный, как присутствие: он не лежит в истории и живёт полминуты.
// Здесь — что сейчас звонит, кто зовёт и куда, и что делать по нажатию.

import { isPermissionGranted, sendNotification } from '@tauri-apps/plugin-notification';

import { api, errorText, onNotice, type Id } from '../ipc';
import { RING_LIMIT, Ringtone } from '../ringtone';
import { call } from './call.svelte';
import { session } from './session.svelte';

export interface IncomingRing {
  /** Кто и какой зов: повторы одного зова ядро уже отсеяло. */
  key: string;
  space: Id;
  spaceName: string;
  channel: Id;
  room: string;
  from: Id;
  nick: string;
}

class Ringing {
  incoming = $state<IncomingRing[]>([]);
  #tone = new Ringtone();
  #expiry = new Map<string, number>();
  /**
   * Отбои, пришедшие раньше, чем зов успел показаться.
   *
   * Между приходом зова и показом карточки есть ожидание — имя комнаты
   * спрашивается у ядра. Отбой, попавший в эту щель, раньше терялся, и звонок
   * звенел полминуты в комнату, из которой уже ушли.
   */
  #cancelled = new Set<string>();
  #arriving = new Set<string>();

  /** Слушать зовы от ядра. Отдельно от сессии: зову не нужна ни лента, ни каналы. */
  async listen(): Promise<() => void> {
    return onNotice((notice) => {
      if (notice.kind === 'ring') {
        void this.receive(notice.space, notice.channel, notice.from, notice.nick, notice.id);
      } else if (notice.kind === 'ring-cancel') {
        this.cancel(notice.from, notice.id);
      }
    });
  }

  async receive(space: Id, channel: Id, from: Id, nick: string, id: string): Promise<void> {
    const key = `${from}:${id}`;
    if (this.#arriving.has(key) || this.#cancelled.has(key)) return;
    if (this.incoming.some((ring) => ring.key === key)) return;
    // Уже в этой комнате — звонить незачем: ядро это проверяет, но человек
    // мог зайти, пока зов ехал.
    if (call.active && call.space === space && call.channel === channel) return;

    const spaceName = session.spaces.find((s) => s.id === space)?.name ?? 'пространство';
    let room = 'голосовая комната';
    this.#arriving.add(key);
    try {
      room = (await api.listChannels(space)).find((c) => c.id === channel)?.name ?? room;
    } catch {
      // Имя комнаты — украшение; без него зов всё равно понятен.
    } finally {
      this.#arriving.delete(key);
    }
    // Пока узнавали имя, могли дать отбой — или мы сами зашли в эту комнату.
    if (this.#cancelled.has(key)) return;
    if (call.active && call.space === space && call.channel === channel) return;

    this.incoming = [...this.incoming, { key, space, spaceName, channel, room, from, nick }];
    this.#tone.start();
    this.#expiry.set(
      key,
      window.setTimeout(() => this.#drop(key), RING_LIMIT),
    );

    // Окно свёрнуто — человек карточку не увидит. Системное уведомление
    // приведёт его в приложение, где кнопка «подключиться» уже ждёт.
    if (!document.hasFocus()) {
      try {
        if (await isPermissionGranted()) {
          sendNotification({ title: `${nick} зовёт в «${room}»`, body: `${spaceName} · откройте БРЕД, чтобы подключиться` });
        }
      } catch {
        // уведомления недоступны — звонок и карточка остаются
      }
    }
  }

  /** Звонящий дал отбой. Помним минуту: зов, доехавший позже отбоя, не звонит. */
  cancel(from: Id, id: string): void {
    const key = `${from}:${id}`;
    this.#cancelled.add(key);
    window.setTimeout(() => this.#cancelled.delete(key), 60_000);
    this.#drop(key);
  }

  async accept(ring: IncomingRing): Promise<void> {
    // Все зовы в ту же комнату гаснут разом: зайти можно только один раз.
    for (const other of this.incoming.filter((r) => r.space === ring.space && r.channel === ring.channel)) {
      this.#drop(other.key);
    }
    try {
      if (session.spaceId !== ring.space) await session.selectSpace(ring.space);
      await session.joinVoice(ring.channel);
    } catch (error) {
      session.status = errorText(error);
    }
  }

  decline(ring: IncomingRing): void {
    this.#drop(ring.key);
  }

  /** Зашли в комнату сами — зовы туда больше не нужны. */
  joined(space: Id, channel: Id): void {
    for (const ring of this.incoming.filter((r) => r.space === space && r.channel === channel)) {
      this.#drop(ring.key);
    }
  }

  #drop(key: string): void {
    const timer = this.#expiry.get(key);
    if (timer !== undefined) window.clearTimeout(timer);
    this.#expiry.delete(key);
    this.incoming = this.incoming.filter((ring) => ring.key !== key);
    if (this.incoming.length === 0) this.#tone.stop();
  }
}

export const ringing = new Ringing();
