// Настройки внешнего вида.
//
// Живут только на этом устройстве и в лог событий не попадают: это про то, как
// человеку удобно смотреть, а не про то, что произошло в переписке.

const STORAGE_KEY = 'bred.prefs';

interface Stored {
  colorVideo: boolean;
  /** Громкость по каждому собеседнику, 1 — как есть. */
  volumes: Record<string, number>;
}

function load(): Stored {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) return { colorVideo: false, volumes: {}, ...JSON.parse(raw) };
  } catch {
    // Повреждённые настройки — не повод не запускаться.
  }
  return { colorVideo: false, volumes: {} };
}

export class Prefs {
  /**
   * Показывать видео в цвете.
   *
   * По умолчанию выключено: интерфейс монохромный, и цветное окно посреди него
   * выбивается. Но это вкус, а не закон — поэтому переключатель.
   */
  colorVideo = $state(load().colorVideo);

  /**
   * Настроенная громкость собеседников. Переживает перезапуск: подкручивать
   * одного и того же тихого человека каждый созвон — издевательство.
   */
  volumes = $state<Record<string, number>>(load().volumes);

  toggleColorVideo(): void {
    this.colorVideo = !this.colorVideo;
    this.#save();
  }

  setVolume(author: string, value: number): void {
    this.volumes = { ...this.volumes, [author]: value };
    this.#save();
  }

  #save(): void {
    try {
      localStorage.setItem(
        STORAGE_KEY,
        JSON.stringify({ colorVideo: this.colorVideo, volumes: this.volumes }),
      );
    } catch {
      // Нет доступа к хранилищу — настройка просто не переживёт перезапуск.
    }
  }
}

export const prefs = new Prefs();
