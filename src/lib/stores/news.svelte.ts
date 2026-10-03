// Канал «Обновления БРЕД».
//
// Посты — это выпуски: заголовок и описание пишет разработчик в релизе на
// GitHub, ядро забирает их, когда выпуск достроился и его можно поставить.
// Писать сюда нельзя никому, а уведомление о новом выпуске не глушится:
// настройка звука сообщений его не касается.

import { isPermissionGranted, sendNotification } from '@tauri-apps/plugin-notification';

import { api, onNotice, type NewsPost } from '../ipc';

/** Сколько живёт аудиоконтекст фанфары — с запасом на последнюю ноту. */
const FANFARE_MS = 1500;

/** Сравнение версий по числам: «0.10» старше «0.9». */
export function newer(a: string, b: string): boolean {
  const parse = (v: string) => v.replace(/^v/, '').split('.').map((p) => Number(p) || 0);
  const x = parse(a);
  const y = parse(b);
  for (let i = 0; i < Math.max(x.length, y.length); i += 1) {
    if ((x[i] ?? 0) !== (y[i] ?? 0)) return (x[i] ?? 0) > (y[i] ?? 0);
  }
  return false;
}

/** Звук нового выпуска — свой, не как у сообщения: три восходящие ноты. */
function fanfare(): void {
  try {
    const context = new AudioContext();
    [523, 659, 784].forEach((frequency, index) => {
      const at = context.currentTime + index * 0.12;
      const osc = context.createOscillator();
      const gain = context.createGain();
      osc.type = 'triangle';
      osc.frequency.value = frequency;
      gain.gain.setValueAtTime(0.0001, at);
      gain.gain.exponentialRampToValueAtTime(0.1, at + 0.015);
      gain.gain.exponentialRampToValueAtTime(0.0001, at + 0.22);
      osc.connect(gain).connect(context.destination);
      osc.start(at);
      osc.stop(at + 0.24);
    });
    window.setTimeout(() => void context.close().catch(() => undefined), FANFARE_MS);
  } catch {
    // нет звукового устройства — уведомление и отметка всё равно будут
  }
}

class News {
  posts = $state<NewsPost[]>([]);
  read = $state<string | null>(null);
  current = $state('');
  /** Открыт ли канал вместо ленты. */
  open = $state(false);
  /** Свежий выпуск, о котором только что сказали, — для плашки поверх всего. */
  fresh = $state<{ version: string; title: string } | null>(null);
  #listening = false;

  /** Сколько выпусков человек ещё не видел. При первом запуске история —
   *  не непрочитанное: новое только то, что новее установленной версии. */
  get unread(): number {
    const mark = this.read ?? this.current;
    if (!mark) return 0;
    return this.posts.filter((post) => newer(post.version, mark)).length;
  }

  async init(): Promise<void> {
    await this.reload();
    if (this.#listening) return;
    this.#listening = true;
    await onNotice((notice) => {
      if (notice.kind === 'news') void this.#announce(notice.version, notice.title);
      if (notice.kind === 'news-feed') void this.reload();
    }).catch(() => undefined);
  }

  async reload(): Promise<void> {
    try {
      const feed = await api.newsFeed();
      this.posts = feed.posts;
      this.read = feed.read;
      this.current = feed.current;
    } catch {
      // Канал — не главное; без него приложение работает как работало.
    }
  }

  async show(): Promise<void> {
    this.open = true;
    this.fresh = null;
    await api.newsRead().catch(() => undefined);
    await this.reload();
  }

  hide(): void {
    this.open = false;
  }

  async refresh(): Promise<void> {
    try {
      const feed = await api.newsRefresh();
      this.posts = feed.posts;
      this.read = feed.read;
      this.current = feed.current;
    } catch {
      // GitHub недоступен — покажем то, что уже скачано
    }
  }

  async #announce(version: string, title: string): Promise<void> {
    await this.reload();
    this.fresh = { version, title };
    fanfare();
    try {
      if (await isPermissionGranted()) {
        sendNotification({ title, body: `Вышло обновление БРЕД ${version} — что нового, в канале «Обновления»` });
      }
    } catch {
      // уведомления недоступны — плашка и отметка в канале остаются
    }
  }
}

export const news = new News();
