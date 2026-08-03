//! Лог событий — единственный источник правды в БРЕД.
//!
//! Сервера нет, поэтому у истории не может быть «правильного» порядка,
//! назначенного кем-то одним. Вместо этого каждое событие несёт логические часы
//! Лампорта, а глобальный порядок задаётся кортежем `(lamport, author, seq)` —
//! он одинаков у всех участников независимо от того, в каком порядке события
//! доехали. Каждое событие подписано автором, так что подделать чужое нельзя
//! даже полностью контролируя транспорт.

use serde::{Deserialize, Serialize};

use super::ids::{AuthorId, ChannelId, EventId, Id, SpaceId};

/// Что именно произошло.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EventKind {
    /// Создание пространства («сервера»).
    SpaceCreate { name: String },
    /// Создание канала. Идентификатор канала — это идентификатор этого события.
    ChannelCreate {
        name: String,
        category: String,
        voice: bool,
    },
    /// Сообщение в канал.
    Message {
        channel: ChannelId,
        body: String,
        /// Ответ на сообщение (цитата).
        reply_to: Option<EventId>,
        /// Корень ветки, если сообщение написано внутри ветки.
        thread: Option<EventId>,
        /// Прикреплённые файлы. В событии едет только описание: сами байты
        /// забираются у пиров по требованию, иначе лог распух бы от медиа.
        #[serde(default)]
        attachments: Vec<Attachment>,
    },
    /// Правка ранее отправленного сообщения.
    Edit { target: EventId, body: String },
    /// Удаление сообщения (надгробие — само событие остаётся в логе).
    Delete { target: EventId },
    /// Реакция на сообщение. `remove = true` снимает ранее поставленную.
    Reaction {
        target: EventId,
        emoji: String,
        remove: bool,
    },
    /// Смена отображаемого имени и картинки. Аватар — хеш вложения, поэтому
    /// им может быть и анимированный GIF: показываем как обычную картинку.
    Profile {
        nick: String,
        #[serde(default)]
        avatar: Option<EventId>,
        /// Публичный ключ согласования: по нему собеседник выводит ключ личной
        /// переписки, ничего у нас не спрашивая.
        #[serde(default)]
        dh: Option<Id>,
    },
    /// Свой эмодзи или стикер пространства: `:имя:` указывает на вложение.
    EmojiAdd {
        name: String,
        hash: EventId,
        /// Стикер показывается крупно и отдельным сообщением.
        sticker: bool,
    },
    /// Убрать свой эмодзи из набора пространства.
    EmojiRemove { name: String },
    /// Удалить канал вместе со всем, что в нём написано.
    ChannelDelete { channel: ChannelId },
}

/// Описание прикреплённого файла.
///
/// Содержимое адресуется хешем: имя может совпадать у разных файлов, а
/// blake3-хеш — нет. Он же служит проверкой целостности при скачивании.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Attachment {
    pub hash: Id,
    pub name: String,
    pub size: u64,
    pub mime: String,
}

/// Событие до подписи.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Event {
    pub space: SpaceId,
    pub author: AuthorId,
    /// Порядковый номер в личном логе автора. Растёт строго на единицу —
    /// по нему получатель понимает, что событие пропущено.
    pub seq: u64,
    /// Логические часы Лампорта: `max(увиденные) + 1`.
    pub lamport: u64,
    /// Настенное время в миллисекундах. Только для показа, порядок им не задаётся:
    /// часы у участников разъезжаются, доверять им нельзя.
    pub ts: i64,
    pub kind: EventKind,
}

impl Event {
    /// Канонические байты для подписи и хеша.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("событие сериализуемо")
    }

    /// Идентификатор события — blake3 от канонических байтов.
    pub fn id(&self) -> EventId {
        Id(*blake3::hash(&self.canonical_bytes()).as_bytes())
    }

    /// Ключ глобального порядка. Одинаков у всех участников.
    pub fn order_key(&self) -> (u64, [u8; 32], u64) {
        (self.lamport, self.author.0, self.seq)
    }
}

/// Событие вместе с подписью автора.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignedEvent {
    pub event: Event,
    #[serde(with = "serde_sig")]
    pub sig: [u8; 64],
}

impl SignedEvent {
    pub fn id(&self) -> EventId {
        self.event.id()
    }
}

/// Логические часы Лампорта.
#[derive(Debug, Default)]
pub struct Clock {
    last: u64,
}

impl Clock {
    pub fn new(last_seen: u64) -> Self {
        Self { last: last_seen }
    }

    /// Тик при создании собственного события.
    pub fn tick(&mut self) -> u64 {
        self.last += 1;
        self.last
    }

    /// Наблюдение чужого события: часы не могут идти назад.
    pub fn observe(&mut self, remote: u64) {
        if remote > self.last {
            self.last = remote;
        }
    }

    pub fn current(&self) -> u64 {
        self.last
    }
}

/// Текущее настенное время в миллисекундах.
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Подпись — 64 байта, serde из коробки такие массивы не умеет.
mod serde_sig {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8; 64], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(v)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        // Симметрично `serialize_bytes` — см. комментарий в `domain::ids`.
        let raw: &[u8] = <&[u8]>::deserialize(d)?;
        raw.try_into()
            .map_err(|_| serde::de::Error::custom("подпись должна быть 64 байта"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(lamport: u64, author: u8, seq: u64) -> Event {
        Event {
            space: Id([1u8; 32]),
            author: Id([author; 32]),
            seq,
            lamport,
            ts: 0,
            kind: EventKind::Message {
                channel: Id([2u8; 32]),
                body: "привет".into(),
                reply_to: None,
                thread: None,
                attachments: Vec::new(),
            },
        }
    }

    #[test]
    fn identical_events_hash_identically() {
        assert_eq!(ev(1, 1, 1).id(), ev(1, 1, 1).id());
    }

    #[test]
    fn differing_body_changes_id() {
        let mut other = ev(1, 1, 1);
        other.ts = 5;
        assert_ne!(ev(1, 1, 1).id(), other.id());
    }

    #[test]
    fn order_is_stable_regardless_of_arrival() {
        let mut a = vec![ev(3, 2, 1), ev(1, 9, 1), ev(3, 1, 1)];
        let mut b = vec![ev(3, 1, 1), ev(3, 2, 1), ev(1, 9, 1)];
        a.sort_by_key(|e| e.order_key());
        b.sort_by_key(|e| e.order_key());
        assert_eq!(a, b, "порядок не зависит от очерёдности доставки");
    }

    #[test]
    fn clock_never_goes_backwards() {
        let mut c = Clock::new(5);
        c.observe(2);
        assert_eq!(c.current(), 5);
        c.observe(9);
        assert_eq!(c.tick(), 10);
    }
}
