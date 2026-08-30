// Настройки внешнего вида.
//
// Живут только на этом устройстве и в лог событий не попадают: это про то, как
// человеку удобно смотреть, а не про то, что произошло в переписке.

const STORAGE_KEY = 'bred.prefs';

interface Stored {
  colorVideo: boolean;
  /** Звук при входящем сообщении. */
  soundOnMessage: boolean;
  /** Громкость по каждому собеседнику, 1 — как есть. */
  volumes: Record<string, number>;
  /** Громкость общего плеера у себя, 1 — как есть. */
  musicVolume: number;
}

const DEFAULTS: Stored = {
  colorVideo: false,
  soundOnMessage: true,
  volumes: {},
  musicVolume: 0.7,
};

function load(): Stored {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) return { ...DEFAULTS, ...JSON.parse(raw) };
  } catch {
    // Повреждённые настройки — не повод не запускаться.
  }
  return DEFAULTS;
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

  /**
   * Громкость общего плеера.
   *
   * Своя у каждого слушателя и отдельно от голоса — иначе не выйдет главного:
   * приглушить музыку, чтобы за ней было слышно разговор. По умолчанию тише
   * единицы: музыка приходит с полной громкости приложения-источника и легко
   * перекрикивает людей.
   */
  musicVolume = $state(load().musicVolume);

  /**
   * Короткий звук при чужом сообщении.
   *
   * По умолчанию включён: уведомление операционной системы человек пропускает,
   * если окно просто ушло на второй план, а звук слышно всегда.
   */
  soundOnMessage = $state(load().soundOnMessage);

  toggleSoundOnMessage(): void {
    this.soundOnMessage = !this.soundOnMessage;
    this.#save();
  }

  toggleColorVideo(): void {
    this.colorVideo = !this.colorVideo;
    this.#save();
  }

  setVolume(author: string, value: number): void {
    this.volumes = { ...this.volumes, [author]: value };
    this.#save();
  }

  setMusicVolume(value: number): void {
    this.musicVolume = value;
    this.#save();
  }

  #save(): void {
    try {
      localStorage.setItem(
        STORAGE_KEY,
        JSON.stringify({
          colorVideo: this.colorVideo,
          soundOnMessage: this.soundOnMessage,
          volumes: this.volumes,
          musicVolume: this.musicVolume,
        }),
      );
    } catch {
      // Нет доступа к хранилищу — настройка просто не переживёт перезапуск.
    }
  }
}

export const prefs = new Prefs();
