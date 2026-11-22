// Микрофон, камера и колонки для звонка.
//
// Список устройств даёт вебвью, а не ядро: захват и воспроизведение живут там
// же, и идентификаторы годятся только для него. Выбор хранится в prefs, а
// здесь — то, что подключено прямо сейчас, и сопоставление одного с другим.

import { prefs, type ChosenDevice, type DeviceKind } from './prefs.svelte';

export interface DeviceOption {
  id: string;
  label: string;
}

const KIND: Record<MediaDeviceKind, DeviceKind> = {
  audioinput: 'mic',
  videoinput: 'camera',
  audiooutput: 'speaker',
};

/** Системные «устройства по умолчанию» — это не устройства, а ссылки на них. */
function isAlias(device: MediaDeviceInfo): boolean {
  return device.deviceId === '' || device.deviceId === 'default' || device.deviceId === 'communications';
}

/**
 * Можно ли вообще выбрать колонки.
 *
 * WebView2 на Windows умеет это у аудиоконтекста, WKWebView на маке — только у
 * элемента `<audio>`: тогда звонок играет через него.
 */
export function speakerSelectable(): boolean {
  return (
    'setSinkId' in AudioContext.prototype ||
    (typeof HTMLMediaElement !== 'undefined' && 'setSinkId' in HTMLMediaElement.prototype)
  );
}

class DevicesStore {
  mics = $state<DeviceOption[]>([]);
  cameras = $state<DeviceOption[]>([]);
  speakers = $state<DeviceOption[]>([]);
  /**
   * Названия скрыты, пока не дано разрешение на микрофон или камеру: до этого
   * вебвью показывает только безымянные устройства.
   */
  unnamed = $state(false);
  error = $state('');
  /** Виды устройств, список которых вебвью пока прячет. */
  #hidden = $state(new Set<DeviceKind>());
  /** Идущий запрос доступа к звуку — один на всех, кто ждёт. */
  #unlocking: Promise<void> | null = null;
  #watching = false;

  /**
   * Есть ли из чего выбирать колонки. Платформа может уметь переключать
   * вывод, но не перечислять его — тогда пустой список только путает.
   */
  get canPickSpeaker(): boolean {
    return speakerSelectable() && (this.speakers.length > 0 || this.unnamed);
  }

  /** Прячет ли вебвью этот вид устройств — до разрешения в этом запуске. */
  hidden(kind: DeviceKind): boolean {
    return this.#hidden.has(kind);
  }

  /**
   * Показать микрофоны и колонки, если они ещё скрыты. Звук спрашиваем без
   * кнопки: микрофон открывается на миг, а без этого в списке пусто, хотя
   * он подключён. Камеру — только по кнопке: её огонёк пугает.
   */
  async showAudio(): Promise<void> {
    await this.refresh();
    if (this.#hidden.has('mic') || this.#hidden.has('speaker')) await this.#unlockAudio();
  }

  list(kind: DeviceKind): DeviceOption[] {
    return kind === 'mic' ? this.mics : kind === 'camera' ? this.cameras : this.speakers;
  }

  async refresh(): Promise<void> {
    if (!navigator.mediaDevices?.enumerateDevices) return;
    this.#watch();
    try {
      const all = await navigator.mediaDevices.enumerateDevices();
      const lists: Record<DeviceKind, DeviceOption[]> = { mic: [], camera: [], speaker: [] };
      const hidden = new Set<DeviceKind>();
      all.forEach((device, index) => {
        if (isAlias(device)) {
          if (!device.label && device.kind !== 'audiooutput') hidden.add(KIND[device.kind]);
          return;
        }
        if (!device.label) hidden.add(KIND[device.kind]);
        lists[KIND[device.kind]].push({
          id: device.deviceId,
          label: device.label || `устройство ${index + 1}`,
        });
      });
      this.mics = lists.mic;
      this.cameras = lists.camera;
      this.speakers = lists.speaker;
      this.#hidden = hidden;
      this.unnamed = hidden.size > 0;
      this.error = '';
    } catch (issue) {
      this.error = issue instanceof Error ? issue.message : String(issue);
    }
  }

  /**
   * Спросить разрешение, чтобы увидеть названия.
   *
   * Микрофон и камеру спрашиваем порознь: без камеры совместный запрос упал бы
   * целиком, и микрофоны так и остались бы безымянными.
   */
  async reveal(): Promise<void> {
    for (const constraints of [{ audio: true }, { video: true }] as MediaStreamConstraints[]) {
      try {
        const stream = await navigator.mediaDevices.getUserMedia(constraints);
        stream.getTracks().forEach((track) => track.stop());
      } catch {
        // Нет такого устройства или отказали — у остальных названия всё равно появятся.
      }
    }
    await this.refresh();
  }

  /**
   * Какое устройство взять на самом деле.
   *
   * Пусто — системное по умолчанию: и когда ничего не выбрано, и когда
   * выбранное сейчас не подключено. Звонок без наушников, которые остались
   * дома, должен идти через встроенный микрофон, а не не идти вовсе.
   *
   * Но «не подключено» надо знать наверняка. Пока в этом запуске не дано
   * разрешение, WKWebView прячет список: ни id, ни названий. Раньше выбранный
   * микрофон в таком списке «не находился», и звонок молча шёл через
   * встроенный — а у ноутбука с закрытой крышкой тот заглушён, и человека не
   * было слышно вовсе. Поэтому сначала открываем звук — он всё равно сейчас
   * понадобится, — и только потом сверяемся.
   */
  async resolve(kind: DeviceKind): Promise<string> {
    const chosen = prefs.devices[kind];
    if (!chosen) return '';
    await this.refresh();
    if (kind !== 'camera' && this.#hidden.has(kind)) await this.#unlockAudio();
    const list = this.list(kind);
    const exact = list.find((device) => device.id === chosen.id);
    if (exact) return exact.id;
    const byName = list.find((device) => device.label === chosen.label);
    if (byName) {
      prefs.setDevice(kind, { id: byName.id, label: byName.label });
      return byName.id;
    }
    // Камеру ради списка не включаем — мигнул бы огонёк. Берём сохранённый
    // id как есть: если устройства нет, openDevice сам возьмёт системное.
    if (this.#hidden.has(kind)) return chosen.id;
    return '';
  }

  /** Разрешение на звук, чтобы вебвью показал микрофоны и колонки. */
  async #unlockAudio(): Promise<void> {
    this.#unlocking ??= (async () => {
      try {
        const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
        stream.getTracks().forEach((track) => track.stop());
      } catch {
        // Отказали или микрофона нет — останется системное.
      }
      await this.refresh();
    })().finally(() => (this.#unlocking = null));
    await this.#unlocking;
  }

  /** Подключено ли выбранное устройство прямо сейчас. */
  present(kind: DeviceKind, chosen: ChosenDevice | null): boolean {
    if (!chosen) return true;
    return this.list(kind).some((device) => device.id === chosen.id || device.label === chosen.label);
  }

  /** Что показать в списке: подключённые плюс выбранное, если его сейчас нет. */
  choices(kind: DeviceKind): DeviceOption[] {
    const list = this.list(kind);
    const chosen = prefs.devices[kind];
    if (!chosen || this.present(kind, chosen)) return list;
    return [...list, { id: chosen.id, label: `${chosen.label} — не подключено` }];
  }

  /** Что выбрано в списке. Пусто — системное. */
  selected(kind: DeviceKind): string {
    const chosen = prefs.devices[kind];
    if (!chosen) return '';
    const live = this.list(kind).find((device) => device.id === chosen.id || device.label === chosen.label);
    return live?.id ?? chosen.id;
  }

  #watch(): void {
    if (this.#watching) return;
    this.#watching = true;
    navigator.mediaDevices.addEventListener('devicechange', () => void this.refresh());
  }
}

export const devices = new DevicesStore();
