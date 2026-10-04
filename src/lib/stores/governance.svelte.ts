// Власть в открытом пространстве: моя роль, приглашения, исключение, ключ.
//
// Правила живут в ядре, здесь только то, что показать и куда нажать. Роль
// перечитывается на каждое применённое событие этого пространства: её могли
// выдать или снять, пока мы смотрели.

import {
  api,
  errorText,
  onNotice,
  type Governance,
  type Id,
  type InviteOptions,
  type InviteView,
  type MemberRow,
} from '../ipc';
import { session } from './session.svelte';

/** Готовые сроки приглашения — как в Дискорде, без календаря. */
export const INVITE_TTLS: Array<{ label: string; ms: number }> = [
  { label: '1 час', ms: 3_600_000 },
  { label: '1 день', ms: 86_400_000 },
  { label: '7 дней', ms: 7 * 86_400_000 },
  { label: 'бессрочно', ms: 0 },
];

export const INVITE_USES: Array<{ label: string; uses: number }> = [
  { label: '1 вход', uses: 1 },
  { label: '5', uses: 5 },
  { label: '25', uses: 25 },
  { label: 'без ограничений', uses: 0 },
];

export const ROLE_LABEL: Record<string, string> = {
  owner: 'владелец',
  admin: 'админ',
  member: '',
};

const RELOAD_WINDOW = 400;

class GovernanceStore {
  info = $state<Governance | null>(null);
  invites = $state<InviteView[]>([]);
  /** Ключ сменили без нас: читать новое в этом пространстве мы не можем. */
  lockedOut = $state<Id[]>([]);
  #space: Id | null = null;
  #timer: number | null = null;
  #listening = false;

  get canModerate(): boolean {
    return this.info?.can_moderate === true && this.info.direct === false;
  }

  get isOwner(): boolean {
    return this.info?.role === 'owner';
  }

  /** Можно ли здесь исключать и менять ключ: только при доказуемом владельце. */
  get canRekey(): boolean {
    return this.canModerate && this.info?.founded === true;
  }

  /** Пространство из версий до 0.8: роли есть, исключения и смены ключа нет. */
  get legacy(): boolean {
    return this.info !== null && !this.info.direct && !this.info.founded;
  }

  /** Можно ли мне исключить этого человека: правило то же, что в ядре. */
  canRemove(member: MemberRow): boolean {
    if (!this.canRekey || member.id === session.me || member.role === 'owner') return false;
    return this.isOwner || member.role !== 'admin';
  }

  async load(space: Id | null): Promise<void> {
    this.#space = space;
    this.#listen();
    if (!space) {
      this.info = null;
      this.invites = [];
      return;
    }
    try {
      const info = await api.governance(space);
      if (this.#space !== space) return;
      this.info = info;
      this.invites = await api.listInvites(space);
    } catch {
      // Пространство могли только что покинуть — показывать нечего.
      this.info = null;
      this.invites = [];
    }
  }

  async createInvite(options: InviteOptions): Promise<string> {
    if (!session.spaceId) throw new Error('пространство не выбрано');
    const link = await api.spaceInvite(session.spaceId, options);
    await this.load(session.spaceId);
    return link;
  }

  async revoke(invite: Id): Promise<void> {
    if (!session.spaceId) return;
    await api.revokeInvite(session.spaceId, invite);
    await this.load(session.spaceId);
  }

  async remove(member: MemberRow): Promise<void> {
    if (!session.spaceId) return;
    await api.removeMember(session.spaceId, member.id);
    await this.load(session.spaceId);
    await session.reloadMembers();
    session.say(`${member.nick} исключён(а), ключ пространства сменён — новое ему(ей) не прочитать`);
  }

  async setAdmin(member: MemberRow, admin: boolean): Promise<void> {
    if (!session.spaceId) return;
    await api.setAdmin(session.spaceId, member.id, admin);
    await session.reloadMembers();
    session.say(admin ? `${member.nick} теперь администратор` : `${member.nick} больше не администратор`);
  }

  async rotate(): Promise<void> {
    if (!session.spaceId) return;
    const count = await api.rotateKey(session.spaceId);
    await this.load(session.spaceId);
    session.say(`ключ пространства сменён — новый разложен ${count} участникам; старые ссылки с ключом больше не действуют`);
  }

  #listen(): void {
    if (this.#listening) return;
    this.#listening = true;
    void onNotice((notice) => {
      switch (notice.kind) {
        case 'applied':
        case 'rekeyed':
          if (notice.space === this.#space) this.#reloadSoon();
          if (notice.kind === 'rekeyed') {
            this.lockedOut = this.lockedOut.filter((s) => s !== notice.space);
          }
          break;
        case 'key-lost':
          if (!this.lockedOut.includes(notice.space)) {
            this.lockedOut = [...this.lockedOut, notice.space];
            const name = session.spaces.find((s) => s.id === notice.space)?.name ?? 'пространства';
            session.say(
              `ключ «${name}» сменили, а вашей копии в раздаче нет — новое там вам не видно. ` +
                'Попросите у участников новое приглашение',
            );
          }
          break;
        case 'removed':
          void session
            .dropSpace(notice.space, `вас исключили из «${notice.name}» — его история стёрта с этого устройства`)
            .catch((error) => session.say(errorText(error)));
          break;
      }
    });
  }

  #reloadSoon(): void {
    if (this.#timer !== null) return;
    this.#timer = window.setTimeout(() => {
      this.#timer = null;
      void this.load(this.#space);
    }, RELOAD_WINDOW);
  }
}

export const governance = new GovernanceStore();

/** «через 3 дня», «через 5 ч», «бессрочно» — для списка приглашений. */
export function expiresText(expires: number, now = Date.now()): string {
  if (expires === 0) return 'бессрочно';
  const left = expires - now;
  if (left <= 0) return 'срок вышел';
  const hours = Math.round(left / 3_600_000);
  if (hours < 1) return 'меньше часа';
  if (hours < 48) return `ещё ${hours} ч`;
  return `ещё ${Math.round(hours / 24)} дн`;
}

export function usesText(invite: InviteView): string {
  return invite.uses === 0 ? `вошли ${invite.used}` : `вошли ${invite.used} из ${invite.uses}`;
}
