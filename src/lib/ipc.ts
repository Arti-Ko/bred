// Единственное место, где фронтенд знает про Tauri.
// Всё общение с ядром идёт через типизированные обёртки — если сигнатура
// команды в Rust изменится, ломаться будет здесь, а не по всему UI.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export type Id = string;

export interface SpaceRow {
  id: Id;
  name: string;
  unread: number;
  /** Ключ собеседника, если это личная переписка. */
  direct: Id | null;
}

export interface EmojiRow {
  name: string;
  hash: Id;
  sticker: boolean;
}

export interface ChannelRow {
  id: Id;
  space: Id;
  name: string;
  category: string;
  voice: boolean;
  unread: number;
}

export interface ReactionRow {
  emoji: string;
  count: number;
  mine: boolean;
}

export interface ReplyPreview {
  author: Id;
  nick: string;
  body: string;
}

export interface AttachmentRow {
  hash: Id;
  name: string;
  size: number;
  mime: string;
  /** Лежат ли байты уже на этом устройстве. */
  local: boolean;
}

/** Описание вложения, как его отдаёт `attach_file` перед отправкой. */
export interface Attachment {
  hash: Id;
  name: string;
  size: number;
  mime: string;
}

export interface CallState {
  space: Id | null;
  channel: Id | null;
  participants: Id[];
}

export interface MessageRow {
  id: Id;
  channel: Id;
  author: Id;
  nick: string;
  body: string;
  ts: number;
  lamport: number;
  edited: boolean;
  deleted: boolean;
  reply_to: Id | null;
  reply_preview: ReplyPreview | null;
  thread: Id | null;
  thread_replies: number;
  reactions: ReactionRow[];
  attachments: AttachmentRow[];
}

export interface MemberRow {
  id: Id;
  nick: string;
  /** Хеш картинки профиля: качается как обычное вложение. */
  avatar: Id | null;
  /** Без ключа согласования личную переписку не завести. */
  dh: Id | null;
  online: boolean;
  last_seen: number;
  role: Role;
}

/** Кто в пространстве главный. Владелец доказуем, администраторов назначает он. */
export type Role = 'owner' | 'admin' | 'member';

/** Что мне можно в пространстве. */
export interface Governance {
  role: Role;
  /** `null` — владелец не установлен: пространство создано до 0.8 без записи о создании. */
  owner: Id | null;
  /** Владелец доказуем (пространство из 0.8 и новее): только тогда можно исключать и менять ключ. */
  founded: boolean;
  direct: boolean;
  /** Номер ключа: растёт с каждой сменой. */
  epoch: number;
  can_moderate: boolean;
}

/** Приглашение без ключа: срок, число входов, отзыв. */
export interface InviteView {
  id: Id;
  author: Id;
  author_nick: string;
  created: number;
  /** 0 — бессрочно. */
  expires: number;
  /** 0 — без ограничения. */
  uses: number;
  used: number;
  revoked: boolean;
  live: boolean;
  mine: boolean;
}

export interface InviteOptions {
  /** Сколько живёт, мс; 0 — бессрочно. */
  expiresIn: number;
  /** Сколько раз можно войти; 0 — без ограничения. */
  uses: number;
}

/** Приватность сети и устройства. */
export interface Privacy {
  /** Выбрано в настройках. */
  hide_ip: boolean;
  /** Действует сейчас — настройка применяется при запуске. */
  active: boolean;
  /** Закрыта ли база код-паролем. */
  passcode: boolean;
  /** Где ключ базы без кода: в хранилище системы или в файле рядом. */
  key_storage: 'system' | 'file';
}

export interface Bootstrap {
  me: Id;
  nick: string;
  endpoint: string;
  spaces: SpaceRow[];
}

export interface NetStatus {
  endpoint: string;
  online: boolean;
  spaces: number;
  /** Ретранслятор, через который нас видно снаружи. */
  relay: string | null;
  /** Внешний адрес, каким нас видит интернет. */
  external: string | null;
  /** Живые соседи во всех пространствах. */
  neighbors: number;
}

export type Notice =
  | { kind: 'applied'; space: Id; event: Id }
  | { kind: 'presence'; space: Id }
  | { kind: 'fork'; space: Id; author: Id }
  | { kind: 'version'; space: Id; theirs: number; ours: number }
  | { kind: 'typing'; space: Id; channel: Id; author: Id; nick: string }
  | { kind: 'net' }
  /** В звонке появился новый собеседник — кодировщику пора выдать ключевой кадр. */
  | { kind: 'keyframe' }
  /** Ядро подобрало битрейт дорожки под реальный исходящий канал. */
  | { kind: 'bitrate'; track: string; bps: number }
  /** Изменилось состояние общего плеера: сменился ведущий, источник или пауза. */
  | { kind: 'player'; space: Id }
  /** Кто сейчас говорит в звонке — по громкости его звука у нас. */
  | { kind: 'speaking'; authors: Id[] }
  /** Нас зовут в голосовую комнату. Номер зова — строкой: u64 в JS не влезает. */
  | { kind: 'ring'; space: Id; channel: Id; from: Id; nick: string; id: string }
  /** Звонящий дал отбой. */
  | { kind: 'ring-cancel'; space: Id; from: Id; id: string }
  /** В канале обновлений новый пост: вышла версия, которую можно поставить. */
  | { kind: 'news'; version: string; title: string }
  /** Лента канала обновлений изменилась. */
  | { kind: 'news-feed' }
  /** Ключ пространства сменили, а нашей копии в раздаче нет. */
  | { kind: 'key-lost'; space: Id }
  /** Нас исключили — пространство с устройства стёрто. */
  | { kind: 'removed'; space: Id; name: string }
  /** Пространство перешло на новый ключ. */
  | { kind: 'rekeyed'; space: Id };

/** Приложение, чей звук можно включить на комнату. */
export interface MusicSource {
  /** Идентификатор бандла, либо `*` — весь звук системы. */
  id: string;
  name: string;
}

/** Что играет в комнате и у кого. */
export interface PlayerState {
  host: Id;
  channel: Id;
  source: string;
  playing: boolean;
  ts: number;
}

export type PlayerCommand = 'toggle' | 'next' | 'previous';

export const api = {
  bootstrap: () => invoke<Bootstrap>('bootstrap'),
  lockState: () => invoke<boolean>('lock_state'),
  unlock: (passcode: string) => invoke<void>('unlock', { passcode }),
  wipeLocked: () => invoke<void>('wipe_locked'),
  setPasscode: (passcode: string, current: string | null = null) =>
    invoke<Privacy>('set_passcode', { passcode, current }),
  clearPasscode: (current: string) => invoke<Privacy>('clear_passcode', { current }),
  privacy: () => invoke<Privacy>('privacy_info'),
  setHideIp: (hide: boolean) => invoke<Privacy>('set_hide_ip', { hide }),
  netStatus: () => invoke<NetStatus>('net_status'),

  listSpaces: () => invoke<SpaceRow[]>('list_spaces'),
  listChannels: (space: Id) => invoke<ChannelRow[]>('list_channels', { space }),
  listMessages: (channel: Id, before: number | null = null) =>
    invoke<MessageRow[]>('list_messages', { channel, before }),
  listThread: (root: Id) => invoke<MessageRow[]>('list_thread', { root }),
  attachmentUrl: (hash: Id) => invoke<string>('attachment_url', { hash }),
  ensureAttachment: (space: Id, hash: Id) =>
    invoke<string>('ensure_attachment', { space, hash }),
  collectGarbage: () => invoke<number>('collect_garbage'),
  saveAttachment: (hash: Id, target: string) =>
    invoke<void>('save_attachment', { hash, target }),
  getMessage: (id: Id) => invoke<MessageRow | null>('get_message', { id }),
  listMembers: (space: Id) => invoke<MemberRow[]>('list_members', { space }),

  createSpace: (name: string) => invoke<Id>('create_space', { name }),
  joinSpace: (ticket: string) => invoke<Id>('join_space', { ticket }),
  spaceInvite: (space: Id, options: InviteOptions | null = null) =>
    invoke<string>('space_invite', { space, options }),
  governance: (space: Id) => invoke<Governance>('space_governance', { space }),
  listInvites: (space: Id) => invoke<InviteView[]>('list_invites', { space }),
  revokeInvite: (space: Id, invite: Id) => invoke<void>('revoke_invite', { space, invite }),
  removeMember: (space: Id, member: Id) => invoke<void>('remove_member', { space, member }),
  rotateKey: (space: Id) => invoke<number>('rotate_space_key', { space }),
  setAdmin: (space: Id, member: Id, admin: boolean) =>
    invoke<void>('set_admin', { space, member, admin }),
  leaveSpace: (space: Id) => invoke<void>('leave_space', { space }),
  deleteChannel: (space: Id, channel: Id) => invoke<void>('delete_channel', { space, channel }),
  openDirect: (peer: Id) => invoke<Id>('open_direct', { peer }),
  personalLink: () => invoke<string>('personal_link'),
  openDirectLink: (link: string) => invoke<Id>('open_direct_link', { link }),

  listEmojis: (space: Id) => invoke<EmojiRow[]>('list_emojis', { space }),
  addEmoji: (space: Id, name: string, path: string, sticker: boolean) =>
    invoke<void>('add_emoji', { space, name, path, sticker }),
  removeEmoji: (space: Id, name: string) => invoke<void>('remove_emoji', { space, name }),

  createChannel: (space: Id, name: string, category: string, voice: boolean) =>
    invoke<Id>('create_channel', { space, name, category, voice }),

  sendMessage: (
    space: Id,
    channel: Id,
    body: string,
    replyTo: Id | null = null,
    thread: Id | null = null,
    attachments: Attachment[] = [],
  ) => invoke<Id>('send_message', { space, channel, body, replyTo, thread, attachments }),

  attachFile: (path: string) => invoke<Attachment>('attach_file', { path }),
  setAvatar: (path: string) => invoke<Attachment>('set_avatar', { path }),
  downloadAttachment: (space: Id, hash: Id) =>
    invoke<string>('download_attachment', { space, hash }),

  joinCall: (space: Id, channel: Id) => invoke<void>('join_call', { space, channel }),
  leaveCall: () => invoke<void>('leave_call'),
  callState: () => invoke<CallState>('call_state'),
  voiceMap: (space: Id) => invoke<Array<[Id, Id]>>('voice_map', { space }),

  musicSources: () => invoke<MusicSource[]>('music_sources'),
  musicStart: (source: string) => invoke<void>('music_start', { source }),
  musicStop: () => invoke<void>('music_stop'),
  accountInfo: () => invoke<AccountInfo>('account_info'),
  reportInfo: () => invoke<ReportInfo>('report_info'),
  reportFiles: (paths: string[]) => invoke<ReportFile[]>('report_files', { paths }),
  /** Отправить отчёт. Ответ — его номер, по нему он находится в чате. */
  sendReport: (draft: ReportDraft, sentAt: string) =>
    invoke<string>('send_report', { draft, sentAt }),
  newsFeed: () => invoke<NewsFeed>('news_feed'),
  newsRead: () => invoke<void>('news_read'),
  newsRefresh: () => invoke<NewsFeed>('news_refresh'),
  /** Позвать в свою комнату: пустой список — всех. Ответ — скольких видно в сети. */
  ring: (to: Id[]) => invoke<number>('ring', { to }),
  /** Громкость собеседника у себя: сводит звук ядро, оно и крутит регулятор. */
  setVoiceVolume: (author: Id, gain: number) => invoke<void>('set_voice_volume', { author, gain }),
  setMusicVolume: (gain: number) => invoke<void>('set_music_volume', { gain }),
  musicControl: (command: PlayerCommand) => invoke<void>('music_control', { command }),
  playerState: (space: Id) => invoke<PlayerState | null>('player_state', { space }),

  react: (space: Id, target: Id, emoji: string, remove: boolean) =>
    invoke<void>('react', { space, target, emoji, remove }),
  editMessage: (space: Id, target: Id, body: string) =>
    invoke<void>('edit_message', { space, target, body }),
  deleteMessage: (space: Id, target: Id) => invoke<void>('delete_message', { space, target }),

  setNick: (nick: string) => invoke<void>('set_nick', { nick }),
  markRead: (channel: Id) => invoke<void>('mark_read', { channel }),
  typing: (space: Id, channel: Id) => invoke<void>('typing', { space, channel }),
};

/** Вид проблемы в отчёте: код и подпись. */
export interface ReportKind {
  code: string;
  label: string;
}

/** Что ядро знает о форме отчёта. */
export interface ReportInfo {
  /** Зашит ли в эту сборку ключ бота — без него отправлять некуда. */
  configured: boolean;
  /** Номер человека вида 4829-1305: по нему ищутся его отчёты. */
  number: string;
  kinds: ReportKind[];
}

/** Вложение отчёта: как уйдёт или почему не уйдёт. */
export interface ReportFile {
  path: string;
  name: string;
  size: number;
  kind: 'photo' | 'video' | 'document';
  problem: string | null;
}

export interface ReportDraft {
  kind: string;
  title: string;
  body: string;
  files: string[];
  with_log: boolean;
}

/** Пост канала «Обновления БРЕД» — один выпуск. */
export interface NewsPost {
  version: string;
  title: string;
  body: string;
  published: number;
}

export interface NewsFeed {
  /** Новые сверху. */
  posts: NewsPost[];
  /** До какой версии прочитано. */
  read: string | null;
  /** Установленная версия. */
  current: string;
}

/** Аккаунт: несколько устройств одного человека. См. domain/account.rs в ядре. */
export interface AccountInfo {
  account: Id;
  /** Это устройство. */
  device: Id;
  devices: Array<{ device: Id; name: string; issued: number }>;
}

/** Подписка на уведомления ядра. */
export function onNotice(handler: (notice: Notice) => void): Promise<UnlistenFn> {
  return listen<Notice>('bred://notice', (event) => handler(event.payload));
}

/** Ошибки из Rust приезжают строкой — приводим к ней всё остальное. */
export function errorText(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error instanceof Error) return error.message;
  return String(error);
}
