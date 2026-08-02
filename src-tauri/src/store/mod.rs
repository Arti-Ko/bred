//! Локальное хранилище на SQLite.
//!
//! Лог событий (`events`) — источник правды; всё остальное — производные
//! таблицы, которые пересчитываются при применении события. Так лента канала
//! читается одним индексным запросом, а не сборкой из лога на каждый рендер.

mod model;
mod schema;

pub use model::{
    AttachmentRow, ChannelRow, EmojiRow, MemberRow, MessageRow, ReactionRow, ReplyPreview, SpaceRow,
};

use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use std::{collections::HashMap, path::Path};

use crate::domain::{Event, EventKind, Id, SignedEvent, Space, SpaceId};

/// Что случилось с событием при записи.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// Новое, записано.
    Fresh,
    /// Уже было. Дубликаты приходят постоянно: одно событие приезжает и
    /// по gossip, и досинхронизацией.
    Duplicate,
    /// Автор выдал другое событие с тем же номером — зафиксировано в `forks`.
    Fork,
}

pub struct Store {
    conn: Mutex<Connection>,
    /// Идентификатор текущего пользователя — нужен, чтобы помечать свои реакции.
    me: Mutex<Id>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    /// Хранилище без файла на диске: для тестов и одноразовых сессий.
    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        // WAL — чтобы чтение ленты не блокировалось записью входящих событий.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        schema::migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            me: Mutex::new(Id::ZERO),
        })
    }

    /// Вызывается один раз после загрузки личного ключа.
    pub fn set_me(&self, me: Id) {
        *self.me.lock() = me;
    }

    // ── настройки ───────────────────────────────────────────────────────────

    pub fn get_setting(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row("SELECT v FROM settings WHERE k = ?1", params![key], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &[u8]) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO settings(k, v) VALUES(?1, ?2)
             ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            params![key, value],
        )?;
        Ok(())
    }

    // ── пространства ────────────────────────────────────────────────────────

    pub fn save_space(&self, space: &Space) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO spaces(id, name, key, direct) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name",
            params![
                &space.id.0[..],
                space.name,
                &space.key[..],
                space.direct.map(|d| d.0.to_vec())
            ],
        )?;
        Ok(())
    }

    pub fn spaces(&self) -> Result<Vec<Space>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT id, name, key, direct FROM spaces ORDER BY rowid")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Space {
                    id: id_from_row(r.get::<_, Vec<u8>>(0)?),
                    name: r.get(1)?,
                    key: key_from_row(r.get::<_, Vec<u8>>(2)?),
                    direct: r
                        .get::<_, Option<Vec<u8>>>(3)?
                        .and_then(|raw| Id::from_slice(&raw)),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn space_rows(&self) -> Result<Vec<SpaceRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT s.id, s.name, s.direct,
                    (SELECT COUNT(*) FROM messages m
                      JOIN channels c ON c.id = m.channel
                     WHERE c.space = s.id AND m.lamport > COALESCE(
                           (SELECT read_lamport FROM reads WHERE channel = c.id), 0))
             FROM spaces s ORDER BY s.rowid",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(SpaceRow {
                    id: id_from_row(r.get::<_, Vec<u8>>(0)?),
                    name: r.get(1)?,
                    direct: r
                        .get::<_, Option<Vec<u8>>>(2)?
                        .and_then(|raw| Id::from_slice(&raw)),
                    unread: r.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Полностью забыть пространство: и членство, и всю его историю.
    ///
    /// Именно удаление, а не «скрыть»: если человек вышел, держать у него на
    /// диске чужую переписку неправильно.
    pub fn forget_space(&self, space: SpaceId) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let id = &space.0[..];

        tx.execute(
            "DELETE FROM reactions WHERE target IN (SELECT id FROM messages WHERE space = ?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM attachments WHERE message IN (SELECT id FROM messages WHERE space = ?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM reads WHERE channel IN (SELECT id FROM channels WHERE space = ?1)",
            params![id],
        )?;
        for table in ["messages", "channels", "peers", "events", "spaces"] {
            tx.execute(
                &format!(
                    "DELETE FROM {table} WHERE {} = ?1",
                    if table == "spaces" { "id" } else { "space" }
                ),
                params![id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    // ── применение событий ──────────────────────────────────────────────────

    /// Записывает событие и обновляет производные таблицы.
    pub fn apply(&self, signed: &SignedEvent) -> Result<Applied> {
        let id = signed.id();
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;

        // Тот же номер в логе автора, но другое содержимое — это раздвоение.
        // Первое пришедшее оставляем, второе фиксируем и отвергаем.
        let existing: Option<Vec<u8>> = tx
            .query_row(
                "SELECT id FROM events WHERE space = ?1 AND author = ?2 AND seq = ?3",
                params![
                    &signed.event.space.0[..],
                    &signed.event.author.0[..],
                    signed.event.seq as i64
                ],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(kept) = existing {
            if kept.as_slice() != &id.0[..] {
                tx.execute(
                    "INSERT OR IGNORE INTO forks(space, author, seq, kept, dropped, ts)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        &signed.event.space.0[..],
                        &signed.event.author.0[..],
                        signed.event.seq as i64,
                        kept,
                        &id.0[..],
                        signed.event.ts
                    ],
                )?;
                tx.commit()?;
                return Ok(Applied::Fork);
            }
            return Ok(Applied::Duplicate);
        }

        let raw = postcard::to_stdvec(signed)?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO events(id, space, author, seq, lamport, ts, raw)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &id.0[..],
                &signed.event.space.0[..],
                &signed.event.author.0[..],
                signed.event.seq as i64,
                signed.event.lamport as i64,
                signed.event.ts,
                raw
            ],
        )?;
        if inserted == 0 {
            return Ok(Applied::Duplicate);
        }

        materialize(&tx, &id, &signed.event)?;
        tx.commit()?;
        Ok(Applied::Fresh)
    }

    /// Зафиксированные раздвоения: кому не стоит доверять.
    pub fn forks(&self, space: SpaceId) -> Result<Vec<(Id, u64)>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT author, seq FROM forks WHERE space = ?1 ORDER BY ts DESC")?;
        let rows = stmt
            .query_map(params![&space.0[..]], |r| {
                Ok((
                    id_from_row(r.get::<_, Vec<u8>>(0)?),
                    r.get::<_, i64>(1)? as u64,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Вектор версий: для каждого автора — длина **непрерывного** префикса его
    /// лога, то есть такое N, что события 1..=N у нас точно есть.
    ///
    /// Именно непрерывного, а не максимального номера. Если взять `MAX(seq)`,
    /// то лог с дырой (1, 2, 4) отрапортует «у меня всё до 4», пир не пришлёт
    /// третье событие, и пропуск останется навсегда. Событие теряется тихо —
    /// это худший вид потери данных.
    pub fn version_vector(&self, space: SpaceId) -> Result<HashMap<Id, u64>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT author, MIN(seq), MAX(seq), COUNT(*) FROM events
              WHERE space = ?1 GROUP BY author",
        )?;
        let summary = stmt
            .query_map(params![&space.0[..]], |r| {
                Ok((
                    id_from_row(r.get::<_, Vec<u8>>(0)?),
                    r.get::<_, i64>(1)? as u64,
                    r.get::<_, i64>(2)? as u64,
                    r.get::<_, i64>(3)? as u64,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut out = HashMap::new();
        for (author, min, max, count) in summary {
            // Обычный случай: нумерация с единицы и без пропусков — тогда
            // считать нечего, префикс равен максимуму.
            let prefix = if min == 1 && count == max {
                max
            } else {
                contiguous_prefix(&conn, space, author)?
            };
            if prefix > 0 {
                out.insert(author, prefix);
            }
        }
        Ok(out)
    }

    /// События, которых нет у пира (по его вектору версий).
    pub fn events_missing_for(
        &self,
        space: SpaceId,
        peer_has: &HashMap<Id, u64>,
        limit: usize,
    ) -> Result<Vec<SignedEvent>> {
        // Сначала дешёвая проверка по векторам: если пир не отстал ни по одному
        // автору, читать историю незачем. Раньше здесь был полный скан на каждую
        // встречу — на сотне тысяч событий это заметно.
        let mine = self.version_vector(space)?;
        let behind = mine
            .iter()
            .any(|(author, prefix)| peer_has.get(author).copied().unwrap_or(0) < *prefix);
        if !behind {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT author, seq, raw FROM events WHERE space = ?1 ORDER BY lamport, author, seq",
        )?;
        let rows = stmt.query_map(params![&space.0[..]], |r| {
            Ok((
                id_from_row(r.get::<_, Vec<u8>>(0)?),
                r.get::<_, i64>(1)? as u64,
                r.get::<_, Vec<u8>>(2)?,
            ))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (author, seq, raw) = row?;
            if peer_has.get(&author).copied().unwrap_or(0) >= seq {
                continue;
            }
            out.push(postcard::from_bytes(&raw)?);
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    /// Следующий порядковый номер в личном логе автора.
    pub fn next_seq(&self, space: SpaceId, author: Id) -> Result<u64> {
        let conn = self.conn.lock();
        let max: Option<i64> = conn.query_row(
            "SELECT MAX(seq) FROM events WHERE space = ?1 AND author = ?2",
            params![&space.0[..], &author.0[..]],
            |r| r.get(0),
        )?;
        Ok(max.unwrap_or(0) as u64 + 1)
    }

    /// Максимальные логические часы по всем пространствам — стартовое значение
    /// для `Clock` при запуске приложения.
    pub fn max_lamport(&self) -> Result<u64> {
        let conn = self.conn.lock();
        let max: Option<i64> =
            conn.query_row("SELECT MAX(lamport) FROM events", [], |r| r.get(0))?;
        Ok(max.unwrap_or(0) as u64)
    }

    // ── чтение для UI ───────────────────────────────────────────────────────

    pub fn channels(&self, space: SpaceId) -> Result<Vec<ChannelRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT c.id, c.space, c.name, c.category, c.voice,
                    (SELECT COUNT(*) FROM messages m
                      WHERE m.channel = c.id AND m.lamport > COALESCE(
                            (SELECT read_lamport FROM reads WHERE channel = c.id), 0))
             FROM channels c WHERE c.space = ?1 ORDER BY c.voice, c.created_ts",
        )?;
        let rows = stmt
            .query_map(params![&space.0[..]], |r| {
                Ok(ChannelRow {
                    id: id_from_row(r.get::<_, Vec<u8>>(0)?),
                    space: id_from_row(r.get::<_, Vec<u8>>(1)?),
                    name: r.get(2)?,
                    category: r.get(3)?,
                    voice: r.get::<_, i64>(4)? != 0,
                    unread: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn messages(
        &self,
        channel: Id,
        limit: usize,
        before: Option<u64>,
    ) -> Result<Vec<MessageRow>> {
        let conn = self.conn.lock();
        schema::read_messages(&conn, channel, limit, *self.me.lock(), before)
    }

    /// Ветка целиком: корневое сообщение и все ответы в нём.
    pub fn thread(&self, root: Id) -> Result<Vec<MessageRow>> {
        let conn = self.conn.lock();
        schema::read_thread(&conn, root, *self.me.lock())
    }

    pub fn message(&self, id: Id) -> Result<Option<MessageRow>> {
        let conn = self.conn.lock();
        schema::read_message(&conn, id, *self.me.lock())
    }

    pub fn members(&self, space: SpaceId) -> Result<Vec<MemberRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT p.id, COALESCE(p.nick, ''), p.last_seen, p.avatar, p.dh FROM peers p
             WHERE p.space = ?1 ORDER BY p.last_seen DESC",
        )?;
        let rows = stmt
            .query_map(params![&space.0[..]], |r| {
                Ok(MemberRow {
                    id: id_from_row(r.get::<_, Vec<u8>>(0)?),
                    nick: r.get(1)?,
                    avatar: r
                        .get::<_, Option<Vec<u8>>>(3)?
                        .and_then(|raw| Id::from_slice(&raw)),
                    dh: r
                        .get::<_, Option<Vec<u8>>>(4)?
                        .and_then(|raw| Id::from_slice(&raw)),
                    online: false,
                    last_seen: r.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ── вложения ────────────────────────────────────────────────────────────

    /// Отметить, что байты вложения лежат у нас по указанному пути.
    pub fn record_blob(&self, hash: Id, path: &Path, size: u64) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO blobs(hash, path, size) VALUES(?1, ?2, ?3)
             ON CONFLICT(hash) DO UPDATE SET path = excluded.path, size = excluded.size",
            params![&hash.0[..], path.to_string_lossy(), size as i64],
        )?;
        Ok(())
    }

    pub fn blob_path(&self, hash: Id) -> Result<Option<std::path::PathBuf>> {
        let conn = self.conn.lock();
        let path: Option<String> = conn
            .query_row(
                "SELECT path FROM blobs WHERE hash = ?1",
                params![&hash.0[..]],
                |r| r.get(0),
            )
            .optional()?;
        Ok(path.map(std::path::PathBuf::from))
    }

    /// Описание вложения по хешу — нужно, чтобы знать ожидаемый размер и имя
    /// до того, как файл скачан.
    pub fn attachment(&self, hash: Id) -> Result<Option<AttachmentRow>> {
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT hash, name, size, mime,
                        (SELECT COUNT(*) FROM blobs b WHERE b.hash = a.hash)
                   FROM attachments a WHERE a.hash = ?1 LIMIT 1",
                params![&hash.0[..]],
                |r| {
                    Ok(AttachmentRow {
                        hash: id_from_row(r.get::<_, Vec<u8>>(0)?),
                        name: r.get(1)?,
                        size: r.get::<_, i64>(2)? as u64,
                        mime: r.get(3)?,
                        local: r.get::<_, i64>(4)? > 0,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Кто из участников пространства упоминал это вложение — у них и просим.
    pub fn attachment_holders(&self, hash: Id) -> Result<Vec<Id>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT e.author FROM attachments a
               JOIN events e ON e.id = a.message
              WHERE a.hash = ?1",
        )?;
        let rows = stmt
            .query_map(params![&hash.0[..]], |r| {
                Ok(id_from_row(r.get::<_, Vec<u8>>(0)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Файлы, на которые больше никто не ссылается.
    ///
    /// Вложение живёт, пока на него есть хоть одна ссылка из сообщений:
    /// один и тот же файл могли переслать в трёх каналах, и удаление одного
    /// сообщения не должно уносить остальные.
    pub fn orphan_blobs(&self) -> Result<Vec<(Id, std::path::PathBuf)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT b.hash, b.path FROM blobs b
              WHERE NOT EXISTS (SELECT 1 FROM attachments a WHERE a.hash = b.hash)",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    id_from_row(r.get::<_, Vec<u8>>(0)?),
                    std::path::PathBuf::from(r.get::<_, String>(1)?),
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn forget_blob(&self, hash: Id) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM blobs WHERE hash = ?1", params![&hash.0[..]])?;
        Ok(())
    }

    /// Ключ согласования участника — из любого общего пространства.
    pub fn peer_dh(&self, peer: Id) -> Result<Option<Id>> {
        let conn = self.conn.lock();
        let raw: Option<Vec<u8>> = conn
            .query_row(
                "SELECT dh FROM peers WHERE id = ?1 AND dh IS NOT NULL LIMIT 1",
                params![&peer.0[..]],
                |r| r.get(0),
            )
            .optional()?;
        Ok(raw.and_then(|b| Id::from_slice(&b)))
    }

    pub fn peer_nick(&self, peer: Id) -> Result<Option<String>> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT nick FROM peers WHERE id = ?1 AND nick IS NOT NULL LIMIT 1",
                params![&peer.0[..]],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Набор своих эмодзи и стикеров пространства.
    pub fn emojis(&self, space: SpaceId) -> Result<Vec<EmojiRow>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT name, hash, sticker FROM emojis WHERE space = ?1 ORDER BY name")?;
        let rows = stmt
            .query_map(params![&space.0[..]], |r| {
                Ok(EmojiRow {
                    name: r.get(0)?,
                    hash: id_from_row(r.get::<_, Vec<u8>>(1)?),
                    sticker: r.get::<_, i64>(2)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn mark_read(&self, channel: Id) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO reads(channel, read_lamport)
             VALUES(?1, COALESCE((SELECT MAX(lamport) FROM messages WHERE channel = ?1), 0))
             ON CONFLICT(channel) DO UPDATE SET read_lamport = excluded.read_lamport",
            params![&channel.0[..]],
        )?;
        Ok(())
    }
}

/// Обновление производных таблиц под конкретное событие.
fn materialize(tx: &rusqlite::Transaction<'_>, id: &Id, ev: &Event) -> Result<()> {
    match &ev.kind {
        EventKind::SpaceCreate { name } => {
            tx.execute(
                "UPDATE spaces SET name = ?2 WHERE id = ?1",
                params![&ev.space.0[..], name],
            )?;
        }
        EventKind::ChannelCreate {
            name,
            category,
            voice,
        } => {
            tx.execute(
                "INSERT OR IGNORE INTO channels(id, space, name, category, voice, created_ts)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    &id.0[..],
                    &ev.space.0[..],
                    name,
                    category,
                    *voice as i64,
                    ev.ts
                ],
            )?;
        }
        EventKind::Message {
            channel,
            body,
            reply_to,
            thread,
            attachments,
        } => {
            tx.execute(
                "INSERT OR IGNORE INTO messages
                   (id, space, channel, author, body, ts, lamport, reply_to, thread)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    &id.0[..],
                    &ev.space.0[..],
                    &channel.0[..],
                    &ev.author.0[..],
                    body,
                    ev.ts,
                    ev.lamport as i64,
                    reply_to.map(|r| r.0.to_vec()),
                    thread.map(|t| t.0.to_vec()),
                ],
            )?;
            for file in attachments {
                tx.execute(
                    "INSERT OR IGNORE INTO attachments(message, hash, name, size, mime)
                     VALUES(?1, ?2, ?3, ?4, ?5)",
                    params![
                        &id.0[..],
                        &file.hash.0[..],
                        file.name,
                        file.size as i64,
                        file.mime
                    ],
                )?;
            }
            touch_peer(tx, ev)?;
        }
        EventKind::Edit { target, body } => {
            // Править может только автор — чужие правки просто игнорируем.
            tx.execute(
                "UPDATE messages SET body = ?2, edited = 1
                 WHERE id = ?1 AND author = ?3",
                params![&target.0[..], body, &ev.author.0[..]],
            )?;
        }
        EventKind::Delete { target } => {
            let removed = tx.execute(
                "UPDATE messages SET deleted = 1, body = '' WHERE id = ?1 AND author = ?2",
                params![&target.0[..], &ev.author.0[..]],
            )?;
            // Ссылки на вложения снимаем вместе с сообщением: сами байты уберёт
            // сборщик мусора, когда на них не останется ни одной ссылки.
            if removed > 0 {
                tx.execute(
                    "DELETE FROM attachments WHERE message = ?1",
                    params![&target.0[..]],
                )?;
            }
        }
        EventKind::EmojiAdd {
            name,
            hash,
            sticker,
        } => {
            tx.execute(
                "INSERT INTO emojis(space, name, hash, sticker) VALUES(?1, ?2, ?3, ?4)
                 ON CONFLICT(space, name) DO UPDATE SET hash = excluded.hash,
                     sticker = excluded.sticker",
                params![&ev.space.0[..], name, &hash.0[..], *sticker as i64],
            )?;
            // Держим ссылку на файл, чтобы уборщик не унёс картинку эмодзи.
            tx.execute(
                "INSERT OR IGNORE INTO attachments(message, hash, name, size, mime)
                 VALUES(?1, ?2, ?3, 0, 'image/*')",
                params![&id.0[..], &hash.0[..], name],
            )?;
        }
        EventKind::EmojiRemove { name } => {
            tx.execute(
                "DELETE FROM emojis WHERE space = ?1 AND name = ?2",
                params![&ev.space.0[..], name],
            )?;
        }
        EventKind::Reaction {
            target,
            emoji,
            remove,
        } => {
            if *remove {
                tx.execute(
                    "DELETE FROM reactions WHERE target = ?1 AND author = ?2 AND emoji = ?3",
                    params![&target.0[..], &ev.author.0[..], emoji],
                )?;
            } else {
                tx.execute(
                    "INSERT OR IGNORE INTO reactions(target, author, emoji) VALUES(?1, ?2, ?3)",
                    params![&target.0[..], &ev.author.0[..], emoji],
                )?;
            }
        }
        EventKind::Profile { nick, avatar, dh } => {
            tx.execute(
                "INSERT INTO peers(id, space, nick, avatar, dh, last_seen)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id, space) DO UPDATE SET nick = excluded.nick,
                     avatar = excluded.avatar,
                     dh = COALESCE(excluded.dh, peers.dh),
                     last_seen = MAX(peers.last_seen, excluded.last_seen)",
                params![
                    &ev.author.0[..],
                    &ev.space.0[..],
                    nick,
                    avatar.map(|a| a.0.to_vec()),
                    dh.map(|d| d.0.to_vec()),
                    ev.ts
                ],
            )?;
            // Картинку профиля регистрируем как обычное вложение этого события:
            // тогда её и сборщик мусора не тронет, и скачать можно тем же путём.
            if let Some(hash) = avatar {
                tx.execute(
                    "INSERT OR IGNORE INTO attachments(message, hash, name, size, mime)
                     VALUES(?1, ?2, 'avatar', 0, 'image/*')",
                    params![&id.0[..], &hash.0[..]],
                )?;
            }
        }
    }
    Ok(())
}

/// Отмечаем автора как известного участника пространства.
fn touch_peer(tx: &rusqlite::Transaction<'_>, ev: &Event) -> Result<()> {
    tx.execute(
        "INSERT INTO peers(id, space, nick, last_seen) VALUES(?1, ?2, NULL, ?3)
         ON CONFLICT(id, space) DO UPDATE SET last_seen = MAX(peers.last_seen, excluded.last_seen)",
        params![&ev.author.0[..], &ev.space.0[..], ev.ts],
    )?;
    Ok(())
}

/// Длина непрерывного префикса лога автора: наибольшее N, при котором
/// все события с 1 по N присутствуют.
fn contiguous_prefix(conn: &Connection, space: SpaceId, author: Id) -> Result<u64> {
    let mut stmt =
        conn.prepare("SELECT seq FROM events WHERE space = ?1 AND author = ?2 ORDER BY seq")?;
    let mut rows = stmt.query(params![&space.0[..], &author.0[..]])?;

    let mut expected = 1u64;
    while let Some(row) = rows.next()? {
        let seq = row.get::<_, i64>(0)? as u64;
        if seq != expected {
            break;
        }
        expected += 1;
    }
    Ok(expected - 1)
}

fn id_from_row(raw: Vec<u8>) -> Id {
    Id::from_slice(&raw).unwrap_or(Id::ZERO)
}

fn key_from_row(raw: Vec<u8>) -> [u8; 32] {
    raw.try_into().unwrap_or([0u8; 32])
}
