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
}

export type Notice =
  | { kind: 'applied'; space: Id; event: Id }
  | { kind: 'presence'; space: Id }
  | { kind: 'fork'; space: Id; author: Id }
  | { kind: 'typing'; space: Id; channel: Id; author: Id; nick: string }
  | { kind: 'net' };

export const api = {
  bootstrap: () => invoke<Bootstrap>('bootstrap'),
  netStatus: () => invoke<NetStatus>('net_status'),

  listSpaces: () => invoke<SpaceRow[]>('list_spaces'),
  listChannels: (space: Id) => invoke<ChannelRow[]>('list_channels', { space }),
  listMessages: (channel: Id, before: number | null = null) =>
    invoke<MessageRow[]>('list_messages', { channel, before }),
  listThread: (root: Id) => invoke<MessageRow[]>('list_thread', { root }),
  attachmentUrl: (hash: Id) => invoke<string>('attachment_url', { hash }),
  collectGarbage: () => invoke<number>('collect_garbage'),
  saveAttachment: (hash: Id, target: string) =>
    invoke<void>('save_attachment', { hash, target }),
  getMessage: (id: Id) => invoke<MessageRow | null>('get_message', { id }),
  listMembers: (space: Id) => invoke<MemberRow[]>('list_members', { space }),

  createSpace: (name: string) => invoke<Id>('create_space', { name }),
  joinSpace: (ticket: string) => invoke<Id>('join_space', { ticket }),
  spaceInvite: (space: Id) => invoke<string>('space_invite', { space }),
  leaveSpace: (space: Id) => invoke<void>('leave_space', { space }),
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

  react: (space: Id, target: Id, emoji: string, remove: boolean) =>
    invoke<void>('react', { space, target, emoji, remove }),
  editMessage: (space: Id, target: Id, body: string) =>
    invoke<void>('edit_message', { space, target, body }),
  deleteMessage: (space: Id, target: Id) => invoke<void>('delete_message', { space, target }),

  setNick: (nick: string) => invoke<void>('set_nick', { nick }),
  markRead: (channel: Id) => invoke<void>('mark_read', { channel }),
  typing: (space: Id, channel: Id) => invoke<void>('typing', { space, channel }),
};

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
