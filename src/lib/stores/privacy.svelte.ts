// Приватность устройства: скрытый IP и код-пароль на базе.
//
// Одно хранилище на оба оформления — у них разная вёрстка, но одинаковые
// правила: коды должны совпасть, нынешний нужен, чтобы сменить или убрать.

import { api, errorText, type Privacy } from '../ipc';

export type CodeForm = '' | 'set' | 'change' | 'clear';

class PrivacyStore {
  info = $state<Privacy | null>(null);
  form = $state<CodeForm>('');
  current = $state('');
  fresh = $state('');
  repeat = $state('');
  error = $state('');
  busy = $state(false);

  /** Настройка IP применяется при запуске — до перезапуска она «в ожидании». */
  get pendingRestart(): boolean {
    return this.info !== null && this.info.hide_ip !== this.info.active;
  }

  async load(): Promise<void> {
    try {
      this.info = await api.privacy();
    } catch {
      // Ядро ещё не поднято — показывать нечего.
    }
  }

  async toggleHideIp(): Promise<void> {
    if (!this.info) return;
    try {
      this.info = await api.setHideIp(!this.info.hide_ip);
    } catch (issue) {
      this.error = errorText(issue);
    }
  }

  open(form: CodeForm): void {
    this.form = form;
    this.current = this.fresh = this.repeat = this.error = '';
  }

  async submit(): Promise<void> {
    this.error = '';
    if (this.form !== 'clear' && this.fresh !== this.repeat) {
      this.error = 'коды не совпали';
      return;
    }
    this.busy = true;
    try {
      this.info =
        this.form === 'clear'
          ? await api.clearPasscode(this.current)
          : await api.setPasscode(this.fresh, this.form === 'change' ? this.current : null);
      this.open('');
    } catch (issue) {
      this.error = errorText(issue);
    } finally {
      this.busy = false;
    }
  }

  /** Где сейчас ключ базы — словами. */
  get keyText(): string {
    if (!this.info) return '…';
    if (this.info.passcode) return 'закрыт кодом (Argon2id)';
    return this.info.key_storage === 'system' ? 'в хранилище Windows' : 'в файле рядом с базой';
  }
}

export const privacy = new PrivacyStore();
