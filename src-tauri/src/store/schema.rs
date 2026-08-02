//! Схема БД и запросы чтения ленты.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;

use super::model::{AttachmentRow, MessageRow, ReactionRow, ReplyPreview};
use crate::domain::Id;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS settings (
    k TEXT PRIMARY KEY,
    v BLOB NOT NULL
);

CREATE TABLE IF NOT EXISTS spaces (
    id     BLOB PRIMARY KEY,
    name   TEXT NOT NULL,
    key    BLOB NOT NULL,
    -- Непусто — это личная переписка, и здесь лежит ключ собеседника.
    direct BLOB
);

-- Источник правды. Всё остальное ниже — производные таблицы.
CREATE TABLE IF NOT EXISTS events (
    id      BLOB PRIMARY KEY,
    space   BLOB NOT NULL,
    author  BLOB NOT NULL,
    seq     INTEGER NOT NULL,
    lamport INTEGER NOT NULL,
    ts      INTEGER NOT NULL,
    raw     BLOB NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS events_author_seq ON events(space, author, seq);
CREATE INDEX IF NOT EXISTS events_order ON events(space, lamport, author, seq);

CREATE TABLE IF NOT EXISTS channels (
    id         BLOB PRIMARY KEY,
    space      BLOB NOT NULL,
    name       TEXT NOT NULL,
    category   TEXT NOT NULL DEFAULT '',
    voice      INTEGER NOT NULL DEFAULT 0,
    created_ts INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS channels_space ON channels(space);

CREATE TABLE IF NOT EXISTS messages (
    id       BLOB PRIMARY KEY,
    space    BLOB NOT NULL,
    channel  BLOB NOT NULL,
    author   BLOB NOT NULL,
    body     TEXT NOT NULL,
    ts       INTEGER NOT NULL,
    lamport  INTEGER NOT NULL,
    edited   INTEGER NOT NULL DEFAULT 0,
    deleted  INTEGER NOT NULL DEFAULT 0,
    reply_to BLOB,
    thread   BLOB
);
-- Основной индекс ленты: канал + глобальный порядок.
CREATE INDEX IF NOT EXISTS messages_feed ON messages(channel, lamport, id);
CREATE INDEX IF NOT EXISTS messages_thread ON messages(thread);

CREATE TABLE IF NOT EXISTS reactions (
    target BLOB NOT NULL,
    author BLOB NOT NULL,
    emoji  TEXT NOT NULL,
    PRIMARY KEY (target, author, emoji)
);

CREATE TABLE IF NOT EXISTS peers (
    id        BLOB NOT NULL,
    space     BLOB NOT NULL,
    nick      TEXT,
    avatar    BLOB,
    -- Публичный ключ согласования: по нему заводится личная переписка.
    dh        BLOB,
    last_seen INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (id, space)
);

-- Описания вложений едут в событии, сами байты — по требованию.
CREATE TABLE IF NOT EXISTS attachments (
    message BLOB NOT NULL,
    hash    BLOB NOT NULL,
    name    TEXT NOT NULL,
    size    INTEGER NOT NULL,
    mime    TEXT NOT NULL,
    PRIMARY KEY (message, hash)
);
CREATE INDEX IF NOT EXISTS attachments_hash ON attachments(hash);

-- Что из вложений реально лежит на диске у нас.
CREATE TABLE IF NOT EXISTS blobs (
    hash BLOB PRIMARY KEY,
    path TEXT NOT NULL,
    size INTEGER NOT NULL
);

-- Раздвоение автора: два разных события с одним и тем же номером в его логе.
-- Честного разрешения у этого нет — узел либо сломан, либо жульничает,
-- поэтому мы фиксируем факт и показываем его людям, а не молчим.
CREATE TABLE IF NOT EXISTS forks (
    space   BLOB NOT NULL,
    author  BLOB NOT NULL,
    seq     INTEGER NOT NULL,
    kept    BLOB NOT NULL,
    dropped BLOB NOT NULL,
    ts      INTEGER NOT NULL,
    PRIMARY KEY (space, author, seq)
);

-- Свои эмодзи и стикеры пространства.
CREATE TABLE IF NOT EXISTS emojis (
    space   BLOB NOT NULL,
    name    TEXT NOT NULL,
    hash    BLOB NOT NULL,
    sticker INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (space, name)
);

CREATE TABLE IF NOT EXISTS reads (
    channel       BLOB PRIMARY KEY,
    read_lamport  INTEGER NOT NULL DEFAULT 0
);
"#;

const MESSAGE_COLUMNS: &str = "
    m.id, m.channel, m.author, COALESCE(p.nick, ''), m.body, m.ts, m.lamport,
    m.edited, m.deleted, m.reply_to,
    r.author, COALESCE(rp.nick, ''), r.body,
    m.thread,
    (SELECT COUNT(*) FROM messages t WHERE t.thread = m.id)
";

const MESSAGE_JOINS: &str = "
    FROM messages m
    LEFT JOIN peers    p  ON p.id  = m.author   AND p.space = m.space
    LEFT JOIN messages r  ON r.id  = m.reply_to
    LEFT JOIN peers    rp ON rp.id = r.author   AND rp.space = m.space
";

/// Страница сообщений канала в глобальном порядке.
///
/// `before` — курсор: отдаём то, что строго старше указанных логических часов.
/// Именно курсор, а не OFFSET: в ленту постоянно приезжают новые события, и
/// смещение съезжало бы при каждой подгрузке.
pub fn read_messages(
    conn: &Connection,
    channel: Id,
    limit: usize,
    me: Id,
    before: Option<u64>,
) -> Result<Vec<MessageRow>> {
    let cursor = before.map(|c| c as i64).unwrap_or(i64::MAX);
    let sql = format!(
        "SELECT {MESSAGE_COLUMNS} {MESSAGE_JOINS}
         WHERE m.channel = ?1 AND m.thread IS NULL AND m.lamport < ?3
         ORDER BY m.lamport DESC, m.id DESC
         LIMIT ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt
        .query_map(params![&channel.0[..], limit as i64, cursor], map_message)?
        .collect::<Result<Vec<_>, _>>()?;
    // Запрашивали с конца, показываем с начала.
    rows.reverse();
    attach_reactions(conn, &mut rows, me)?;
    attach_files(conn, &mut rows)?;
    Ok(rows)
}

/// Сообщения внутри ветки, от корня к последнему ответу.
pub fn read_thread(conn: &Connection, root: Id, me: Id) -> Result<Vec<MessageRow>> {
    let sql = format!(
        "SELECT {MESSAGE_COLUMNS} {MESSAGE_JOINS}
         WHERE m.id = ?1 OR m.thread = ?1
         ORDER BY m.lamport, m.id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt
        .query_map(params![&root.0[..]], map_message)?
        .collect::<Result<Vec<_>, _>>()?;
    attach_reactions(conn, &mut rows, me)?;
    attach_files(conn, &mut rows)?;
    Ok(rows)
}

/// Одно сообщение по идентификатору — нужно, чтобы дослать в UI пришедшее из сети.
pub fn read_message(conn: &Connection, id: Id, me: Id) -> Result<Option<MessageRow>> {
    let sql = format!("SELECT {MESSAGE_COLUMNS} {MESSAGE_JOINS} WHERE m.id = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let row = stmt.query_row(params![&id.0[..]], map_message).optional()?;
    let Some(row) = row else { return Ok(None) };
    let mut rows = vec![row];
    attach_reactions(conn, &mut rows, me)?;
    attach_files(conn, &mut rows)?;
    Ok(rows.pop())
}

fn map_message(r: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRow> {
    let reply_to: Option<Vec<u8>> = r.get(9)?;
    let reply_author: Option<Vec<u8>> = r.get(10)?;
    let reply_body: Option<String> = r.get(12)?;

    let reply_preview = match (reply_author, reply_body) {
        (Some(author), Some(body)) => Some(ReplyPreview {
            author: opt_id(Some(author)).unwrap_or(Id::ZERO),
            nick: r.get::<_, String>(11)?,
            body,
        }),
        _ => None,
    };

    Ok(MessageRow {
        id: opt_id(Some(r.get::<_, Vec<u8>>(0)?)).unwrap_or(Id::ZERO),
        channel: opt_id(Some(r.get::<_, Vec<u8>>(1)?)).unwrap_or(Id::ZERO),
        author: opt_id(Some(r.get::<_, Vec<u8>>(2)?)).unwrap_or(Id::ZERO),
        nick: r.get(3)?,
        body: r.get(4)?,
        ts: r.get(5)?,
        lamport: r.get::<_, i64>(6)? as u64,
        edited: r.get::<_, i64>(7)? != 0,
        deleted: r.get::<_, i64>(8)? != 0,
        reply_to: opt_id(reply_to),
        reply_preview,
        thread: opt_id(r.get::<_, Option<Vec<u8>>>(13)?),
        thread_replies: r.get(14)?,
        reactions: Vec::new(),
        attachments: Vec::new(),
    })
}

/// Реакции подтягиваются одним запросом на всю страницу ленты, а не по одному
/// запросу на сообщение — иначе на 200 сообщениях получаем 200 обращений к БД.
fn attach_reactions(conn: &Connection, rows: &mut [MessageRow], me: Id) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let ids: Vec<Id> = rows.iter().map(|m| m.id).collect();
    // Нумеруем все места явно. Если смешать анонимные `?` с нумерованным `?N`,
    // SQLite присваивает анонимным номера «следующие за наибольшим уже занятым»,
    // и нумерация разъезжается с порядком аргументов.
    let placeholders = (1..=ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let me_slot = ids.len() + 1;
    let sql = format!(
        "SELECT target, emoji, COUNT(*), MAX(author = ?{me_slot})
           FROM reactions WHERE target IN ({placeholders})
          GROUP BY target, emoji ORDER BY COUNT(*) DESC"
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut args: Vec<Vec<u8>> = ids.iter().map(|i| i.0.to_vec()).collect();
    args.push(me.0.to_vec());
    let param_refs: Vec<&dyn rusqlite::ToSql> =
        args.iter().map(|a| a as &dyn rusqlite::ToSql).collect();

    let mut grouped: HashMap<Id, Vec<ReactionRow>> = HashMap::new();
    let mapped = stmt.query_map(param_refs.as_slice(), |r| {
        Ok((
            opt_id(Some(r.get::<_, Vec<u8>>(0)?)).unwrap_or(Id::ZERO),
            ReactionRow {
                emoji: r.get(1)?,
                count: r.get(2)?,
                mine: r.get::<_, i64>(3)? != 0,
            },
        ))
    })?;
    for item in mapped {
        let (target, reaction) = item?;
        grouped.entry(target).or_default().push(reaction);
    }

    for row in rows.iter_mut() {
        if let Some(list) = grouped.remove(&row.id) {
            row.reactions = list;
        }
    }
    Ok(())
}

/// Вложения — тоже одним запросом на страницу. Флаг `local` показывает,
/// лежат ли байты уже у нас: от него зависит, скачивать или открывать.
fn attach_files(conn: &Connection, rows: &mut [MessageRow]) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let ids: Vec<Id> = rows.iter().map(|m| m.id).collect();
    let placeholders = (1..=ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT a.message, a.hash, a.name, a.size, a.mime,
                (SELECT COUNT(*) FROM blobs b WHERE b.hash = a.hash)
           FROM attachments a WHERE a.message IN ({placeholders})"
    );

    let mut stmt = conn.prepare(&sql)?;
    let args: Vec<Vec<u8>> = ids.iter().map(|i| i.0.to_vec()).collect();
    let param_refs: Vec<&dyn rusqlite::ToSql> =
        args.iter().map(|a| a as &dyn rusqlite::ToSql).collect();

    let mut grouped: HashMap<Id, Vec<AttachmentRow>> = HashMap::new();
    let mapped = stmt.query_map(param_refs.as_slice(), |r| {
        Ok((
            opt_id(Some(r.get::<_, Vec<u8>>(0)?)).unwrap_or(Id::ZERO),
            AttachmentRow {
                hash: opt_id(Some(r.get::<_, Vec<u8>>(1)?)).unwrap_or(Id::ZERO),
                name: r.get(2)?,
                size: r.get::<_, i64>(3)? as u64,
                mime: r.get(4)?,
                local: r.get::<_, i64>(5)? > 0,
            },
        ))
    })?;
    for item in mapped {
        let (message, attachment) = item?;
        grouped.entry(message).or_default().push(attachment);
    }

    for row in rows.iter_mut() {
        if let Some(list) = grouped.remove(&row.id) {
            row.attachments = list;
        }
    }
    Ok(())
}

fn opt_id(raw: Option<Vec<u8>>) -> Option<Id> {
    raw.and_then(|b| Id::from_slice(&b))
}
