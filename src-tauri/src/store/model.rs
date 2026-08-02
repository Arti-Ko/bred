//! Структуры, которые уезжают в UI. Отдельно от доменных, чтобы формат
//! экрана можно было менять, не трогая формат лога и сети.

use serde::{Deserialize, Serialize};

use crate::domain::Id;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceRow {
    pub id: Id,
    pub name: String,
    pub unread: i64,
    /// Ключ собеседника, если это личная переписка.
    pub direct: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmojiRow {
    pub name: String,
    pub hash: Id,
    pub sticker: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRow {
    pub id: Id,
    pub space: Id,
    pub name: String,
    pub category: String,
    pub voice: bool,
    pub unread: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactionRow {
    pub emoji: String,
    pub count: i64,
    /// Поставил ли реакцию текущий пользователь — от этого зависит инверсия в UI.
    pub mine: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentRow {
    pub hash: Id,
    pub name: String,
    pub size: u64,
    pub mime: String,
    /// Лежат ли байты уже на этом устройстве.
    pub local: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRow {
    pub id: Id,
    pub channel: Id,
    pub author: Id,
    pub nick: String,
    pub body: String,
    pub ts: i64,
    pub lamport: u64,
    pub edited: bool,
    pub deleted: bool,
    pub reply_to: Option<Id>,
    /// Развёрнутая цитата: показать в ленте, не догружая отдельно.
    pub reply_preview: Option<ReplyPreview>,
    pub thread: Option<Id>,
    pub thread_replies: i64,
    pub reactions: Vec<ReactionRow>,
    pub attachments: Vec<AttachmentRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyPreview {
    pub author: Id,
    pub nick: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberRow {
    pub id: Id,
    pub nick: String,
    /// Есть ли у участника ключ согласования — без него личную переписку
    /// с ним не завести, и кнопку показывать нечестно.
    pub dh: Option<Id>,
    /// Хеш картинки профиля. Качается так же, как обычное вложение.
    pub avatar: Option<Id>,
    /// `online` живёт только в памяти: присутствие не пишется в лог,
    /// иначе история распухнет на порядок от «зашёл/вышел».
    pub online: bool,
    pub last_seen: i64,
}
