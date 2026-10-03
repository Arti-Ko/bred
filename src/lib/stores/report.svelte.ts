// Форма «Сообщить о проблеме».
//
// Открывается из настроек (и командой /баг), отправляет отчёт в Telegram
// разработчика. Здесь — только состояние формы; отправку, проверку вложений и
// ограничение частоты держит ядро.

import { open as openFileDialog } from '@tauri-apps/plugin-dialog';

import { api, errorText, type ReportFile, type ReportInfo } from '../ipc';

/** Расширения, которые предлагаем в выборе файла. */
const MEDIA = ['png', 'jpg', 'jpeg', 'webp', 'gif', 'heic', 'mp4', 'mov', 'm4v', 'webm'];
/** Столько вложений принимает ядро. */
const MAX_FILES = 10;

class Report {
  open = $state(false);
  info = $state<ReportInfo | null>(null);
  kind = $state('');
  title = $state('');
  body = $state('');
  files = $state<ReportFile[]>([]);
  withLog = $state(true);
  sending = $state(false);
  error = $state('');
  /** Номер отправленного отчёта — показываем его вместо формы. */
  sent = $state('');

  async show(): Promise<void> {
    this.open = true;
    this.error = '';
    this.sent = '';
    try {
      this.info ??= await api.reportInfo();
    } catch (error) {
      this.error = errorText(error);
    }
  }

  close(): void {
    if (this.sending) return;
    this.open = false;
    // Отправленное — забываем; недописанное — помним до следующего открытия.
    if (this.sent) this.#reset();
  }

  async pick(): Promise<void> {
    try {
      const picked = await openFileDialog({
        multiple: true,
        title: 'Фото или видео к отчёту',
        filters: [{ name: 'Фото и видео', extensions: MEDIA }],
      });
      if (!picked) return;
      const paths = (Array.isArray(picked) ? picked : [picked]).filter(
        (path) => !this.files.some((file) => file.path === path),
      );
      const room = MAX_FILES - this.files.length;
      if (paths.length > room) this.error = `не больше ${MAX_FILES} вложений`;
      const infos = await api.reportFiles(paths.slice(0, Math.max(0, room)));
      this.files = [...this.files, ...infos];
    } catch (error) {
      this.error = errorText(error);
    }
  }

  drop(path: string): void {
    this.files = this.files.filter((file) => file.path !== path);
  }

  get ready(): boolean {
    return (
      !!this.kind &&
      this.title.trim().length > 0 &&
      this.body.trim().length > 0 &&
      this.files.every((file) => !file.problem)
    );
  }

  async send(): Promise<void> {
    if (!this.ready || this.sending) return;
    this.sending = true;
    this.error = '';
    try {
      this.sent = await api.sendReport(
        {
          kind: this.kind,
          title: this.title,
          body: this.body,
          files: this.files.map((file) => file.path),
          with_log: this.withLog,
        },
        // Время — по часам человека и в его поясе: ядру пояс неизвестен.
        new Date().toLocaleString('ru-RU', {
          day: '2-digit',
          month: '2-digit',
          year: 'numeric',
          hour: '2-digit',
          minute: '2-digit',
          timeZoneName: 'short',
        }),
      );
    } catch (error) {
      this.error = errorText(error);
    } finally {
      this.sending = false;
    }
  }

  #reset(): void {
    this.kind = '';
    this.title = '';
    this.body = '';
    this.files = [];
    this.withLog = true;
    this.sent = '';
  }
}

export const report = new Report();
