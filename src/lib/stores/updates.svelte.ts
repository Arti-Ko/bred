// Проверка и установка обновлений.
//
// Источник — релизы на GitHub. Файлы подписаны ключом, публичная половина
// которого зашита в приложение: подменить сборку по дороге нельзя, даже
// подменив сам сервер раздачи.

import { check, type Update } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { getVersion } from '@tauri-apps/api/app';

import { errorText } from '../ipc';

export type UpdateStage =
  | 'idle'
  | 'checking'
  | 'available'
  | 'downloading'
  | 'ready'
  | 'current'
  | 'failed';

export class Updates {
  version = $state('');
  stage = $state<UpdateStage>('idle');
  /** Версия, которая доступна к установке. */
  next = $state('');
  notes = $state('');
  /** Прогресс загрузки в процентах, если размер известен. */
  progress = $state(0);
  error = $state('');

  #pending: Update | null = null;

  async init(): Promise<void> {
    this.version = await getVersion().catch(() => '0.0.0');
  }

  /** Ручная проверка. Молчаливой автопроверки нет: обновление — решение человека. */
  async check(): Promise<void> {
    if (this.stage === 'checking' || this.stage === 'downloading') return;
    this.stage = 'checking';
    this.error = '';

    try {
      const update = await check();
      if (!update) {
        this.stage = 'current';
        return;
      }
      this.#pending = update;
      this.next = update.version;
      this.notes = update.body ?? '';
      this.stage = 'available';
    } catch (error) {
      // Нет сети или ещё нет ни одного релиза — это не поломка приложения.
      this.error = errorText(error);
      this.stage = 'failed';
    }
  }

  async install(): Promise<void> {
    if (!this.#pending) return;
    this.stage = 'downloading';
    this.progress = 0;

    let total = 0;
    let received = 0;

    try {
      await this.#pending.downloadAndInstall((event) => {
        if (event.event === 'Started') {
          total = event.data.contentLength ?? 0;
        } else if (event.event === 'Progress') {
          received += event.data.chunkLength;
          this.progress = total > 0 ? Math.round((received / total) * 100) : 0;
        } else if (event.event === 'Finished') {
          this.progress = 100;
        }
      });
      this.stage = 'ready';
    } catch (error) {
      this.error = errorText(error);
      this.stage = 'failed';
    }
  }

  /** Перезапуск после установки. Отдельным действием: вдруг человек в звонке. */
  async restart(): Promise<void> {
    await relaunch();
  }
}

export const updates = new Updates();
