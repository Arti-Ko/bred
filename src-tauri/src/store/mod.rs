//! Локальное хранилище на SQLite.
//!
//! Лог событий (`events`) — источник правды; всё остальное — производные
//! таблицы, которые пересчитываются при применении события. Так лента канала
//! читается одним индексным запросом, а не сборкой из лога на каждый рендер.

mod governance;
mod model;
mod schema;

pub use governance::{InviteRow, Role, Rotation};
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
    /// Открыть зашифрованную базу. Базу прежних версий — открытым текстом —
    /// сначала переписываем в зашифрованную.
    pub fn open(path: &Path, key: &[u8; 32]) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if is_plaintext(path)? {
            encrypt_in_place(path, key)?;
        }
        let conn = Connection::open(path)?;
        apply_key(&conn, key)?;
        Self::from_connection(conn)
    }

    /// Хранилище без файла на диске: для тестов и одноразовых сессий.
    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(mut conn: Connection) -> Result<Self> {
        // WAL — чтобы чтение ленты не блокировалось записью входящих событий.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        schema::migrate(&conn)?;
        governance::upgrade(&mut conn)?;
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

    // ── устройства аккаунтов ────────────────────────────────────────────────

    /// Запомнить удостоверение устройства. Проверять подпись — забота
    /// вызывающего: хранилище не знает про криптографию.
    ///
    /// Погашено оно или нет, решает самый поздний отзыв: удостоверение,
    /// выданное после него, живое — устройство привязали заново.
    pub fn remember_device(&self, cert: &crate::domain::account::DeviceCert) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO devices(account, device, name, issued, cert) VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(account, device) DO UPDATE SET
                 name = excluded.name, issued = excluded.issued, cert = excluded.cert
             WHERE excluded.issued > devices.issued",
            params![
                &cert.account.0[..],
                &cert.device.0[..],
                cert.name,
                cert.issued,
                postcard::to_stdvec(cert)?
            ],
        )?;
        Self::settle_revocation(&conn, cert.account, cert.device)
    }

    /// Запомнить отзыв. Удостоверение, выданное позже отзыва, им не гасится.
    pub fn revoke_device(&self, revoke: &crate::domain::account::DeviceRevoke) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO device_revocations(account, device, at) VALUES(?1, ?2, ?3)
             ON CONFLICT(account, device) DO UPDATE SET at = MAX(at, excluded.at)",
            params![&revoke.account.0[..], &revoke.device.0[..], revoke.at],
        )?;
        Self::settle_revocation(&conn, revoke.account, revoke.device)
    }

    /// Свести удостоверение с отзывом: гашение — только если отзыв не раньше выдачи.
    fn settle_revocation(conn: &Connection, account: Id, device: Id) -> Result<()> {
        conn.execute(
            "UPDATE devices SET revoked = (
                 SELECT r.at FROM device_revocations r
                  WHERE r.account = devices.account AND r.device = devices.device
                    AND r.at >= devices.issued)
              WHERE account = ?1 AND device = ?2",
            params![&account.0[..], &device.0[..]],
        )?;
        Ok(())
    }

    /// Чьё это устройство. Отозванное — уже ничьё.
    pub fn account_of(&self, device: Id) -> Result<Option<Id>> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT account FROM devices WHERE device = ?1 AND revoked IS NULL
                  ORDER BY issued DESC LIMIT 1",
                params![&device.0[..]],
                |r| Ok(id_from_row(r.get::<_, Vec<u8>>(0)?)),
            )
            .optional()?)
    }

    /// Живые устройства аккаунта — для списка «мои устройства».
    pub fn devices_of(&self, account: Id) -> Result<Vec<crate::domain::account::DeviceCert>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT cert FROM devices WHERE account = ?1 AND revoked IS NULL ORDER BY issued",
        )?;
        let rows = stmt
            .query_map(params![&account.0[..]], |r| r.get::<_, Vec<u8>>(0))?
            .filter_map(Result::ok)
            .filter_map(|raw| postcard::from_bytes(&raw).ok())
            .collect();
        Ok(rows)
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.conn
            .lock()
            .execute("DELETE FROM settings WHERE k = ?1", params![key])?;
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
                     WHERE c.space = s.id AND m.author != ?1 AND m.deleted = 0
                       AND m.lamport > COALESCE(
                           (SELECT read_lamport FROM reads WHERE channel = c.id), 0))
             FROM spaces s ORDER BY s.rowid",
        )?;
        let me = *self.me.lock();
        let rows = stmt
            .query_map(params![&me.0[..]], |r| {
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
        for table in ["invite_revokes", "invite_entries"] {
            tx.execute(
                &format!(
                    "DELETE FROM {table} WHERE invite IN (SELECT id FROM invites WHERE space = ?1)"
                ),
                params![id],
            )?;
        }
        for table in [
            "messages",
            "channels",
            "emojis",
            "peers",
            "events",
            "space_claims",
            "role_grants",
            "removals",
            "key_rotations",
            "past_keys",
            "invites",
            "spaces",
        ] {
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

        let reshaped = materialize(&tx, &id, &signed.event)?;
        // Решение о правах, приехавшее позже того, на что оно влияет, меняет
        // уже записанное: без пересборки у разных участников остались бы разные
        // каналы и сообщения — смотря кто в каком порядке что получил.
        if reshaped && governance::affects_stored(&tx, &signed.event)? {
            governance::rebuild(&tx, signed.event.space)?;
        }
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
            // Своё и удалённое непрочитанным не считается: значок над каналом,
            // куда ты сам только что написал, — это просто вранье.
            "SELECT c.id, c.space, c.name, c.category, c.voice,
                    (SELECT COUNT(*) FROM messages m
                      WHERE m.channel = c.id AND m.author != ?2 AND m.deleted = 0
                        AND m.lamport > COALESCE(
                            (SELECT read_lamport FROM reads WHERE channel = c.id), 0))
             FROM channels c WHERE c.space = ?1 ORDER BY c.voice, c.created_ts",
        )?;
        let me = *self.me.lock();
        let rows = stmt
            .query_map(params![&space.0[..], &me.0[..]], |r| {
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

        // Личную переписку открывают обе стороны, и каждая заводит свой канал
        // «личное» — сойтись они не могут, идентификатор канала это хеш события.
        // Схлопываем одноимённые, оставляя наименьший: правило одинаково у всех.
        let direct = conn
            .query_row(
                "SELECT direct IS NOT NULL FROM spaces WHERE id = ?1",
                params![&space.0[..]],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0)
            != 0;
        if direct {
            let mut seen: std::collections::HashMap<String, ChannelRow> = HashMap::new();
            for row in rows {
                seen.entry(row.name.clone())
                    .and_modify(|kept| {
                        if row.id.0 < kept.id.0 {
                            *kept = row.clone();
                        }
                    })
                    .or_insert(row);
            }
            let mut merged: Vec<ChannelRow> = seen.into_values().collect();
            merged.sort_by_key(|c| (c.voice, c.id.0));
            return Ok(merged);
        }
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

    /// Запомнить, что сосед был на связи: имя, адрес и время.
    ///
    /// Адрес пригодится при следующем запуске: справочник в памяти к тому
    /// моменту пуст, а публичный поиск по идентификатору доступен не в каждой
    /// сети.
    ///
    /// Заводит строку, а не только обновляет её. Строка участника появлялась
    /// раньше единственным путём — применением его события, — а человек,
    /// пришедший по ссылке, мог ещё ничего не написать. Обновлять было нечего:
    /// в списке участников его не было, адрес записывать было некуда, и после
    /// перезапуска звать его было некому. Выглядело это так, что сидишь в
    /// пространстве один, хотя приложение у собеседника открыто.
    pub fn remember_peer_seen(
        &self,
        space: SpaceId,
        peer: Id,
        nick: &str,
        addr: Option<&[u8]>,
    ) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO peers(id, space, nick, addr, last_seen) VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id, space) DO UPDATE SET
                 nick      = COALESCE(excluded.nick, peers.nick),
                 addr      = COALESCE(excluded.addr, peers.addr),
                 last_seen = MAX(peers.last_seen, excluded.last_seen)",
            params![
                &peer.0[..],
                &space.0[..],
                (!nick.is_empty()).then_some(nick),
                addr,
                crate::domain::now_ms()
            ],
        )?;
        Ok(())
    }

    /// Кого видели в сети не раньше `since` (миллисекунды по нашим часам).
    ///
    /// Почтальону нужны именно они, а не все знакомые: дозвон до того, кто
    /// неделю не открывал приложение, — это поиск его адреса по справочникам и
    /// распределённой таблице, пробивка NAT вслепую и таймаут. Каждые полминуты,
    /// на каждого такого — и всё это на том же узле, по которому идёт звонок.
    pub fn recent_peers(&self, space: SpaceId, since: i64) -> Result<Vec<Id>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id FROM peers p WHERE space = ?1 AND last_seen >= ?2
               AND NOT EXISTS (SELECT 1 FROM removals x WHERE x.space = p.space AND x.member = p.id)
              ORDER BY last_seen DESC LIMIT 32",
        )?;
        let rows = stmt
            .query_map(params![&space.0[..], since], |r| {
                Ok(id_from_row(r.get::<_, Vec<u8>>(0)?))
            })?
            .filter_map(Result::ok)
            .collect();
        Ok(rows)
    }

    /// Соседи по пространству и их последние адреса — точки входа в рой.
    pub fn known_peers(&self, space: SpaceId) -> Result<Vec<(Id, Option<Vec<u8>>)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, addr FROM peers p WHERE space = ?1
               AND NOT EXISTS (SELECT 1 FROM removals x WHERE x.space = p.space AND x.member = p.id)
             ORDER BY last_seen DESC LIMIT 32",
        )?;
        let rows = stmt
            .query_map(params![&space.0[..]], |r| {
                Ok((id_from_row(r.get::<_, Vec<u8>>(0)?), r.get(1)?))
            })?
            .filter_map(Result::ok)
            .collect();
        Ok(rows)
    }

    pub fn members(&self, space: SpaceId) -> Result<Vec<MemberRow>> {
        let roles: HashMap<Id, Role> = self.roles(space)?.into_iter().collect();
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT p.id, COALESCE(p.nick, ''), p.last_seen, p.avatar, p.dh FROM peers p
             WHERE p.space = ?1
               AND NOT EXISTS (SELECT 1 FROM removals x WHERE x.space = p.space AND x.member = p.id)
             ORDER BY p.last_seen DESC",
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
                    role: Role::Member,
                })
            })?
            .map(|row| {
                row.map(|mut member| {
                    member.role = roles.get(&member.id).copied().unwrap_or(Role::Member);
                    member
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ── адреса имён ─────────────────────────────────────────────────────────

    /// Запомнить, во что разрешилось имя. `family` — 4 или 6.
    pub fn remember_host(&self, host: &str, family: u8, addrs: &[String]) -> Result<()> {
        if addrs.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO hosts(host, family, addrs, at) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(host, family) DO UPDATE SET addrs = excluded.addrs, at = excluded.at",
            params![
                host,
                family as i64,
                addrs.join(","),
                crate::domain::now_ms()
            ],
        )?;
        Ok(())
    }

    /// Последние известные адреса имени. Пусто — значит спрашивать было не у кого.
    pub fn known_host(&self, host: &str, family: u8) -> Result<Vec<String>> {
        let conn = self.conn.lock();
        let found: Option<String> = conn
            .query_row(
                "SELECT addrs FROM hosts WHERE host = ?1 AND family = ?2",
                params![host, family as i64],
                |row| row.get(0),
            )
            .ok();
        Ok(found
            .unwrap_or_default()
            .split(',')
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect())
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

    /// Запомнить собеседника: имя и ключ согласования из визитки.
    /// Без этого личная переписка забудется при перезапуске.
    pub fn remember_peer(&self, space: SpaceId, peer: Id, nick: &str, dh: Id) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO peers(id, space, nick, dh, last_seen) VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id, space) DO UPDATE SET nick = excluded.nick, dh = excluded.dh",
            params![
                &peer.0[..],
                &space.0[..],
                nick,
                &dh.0[..],
                crate::domain::now_ms()
            ],
        )?;
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
/// Записать последствия события в производные таблицы. `true` — изменились
/// чьи-то права, и уже записанное, возможно, придётся пересобрать.
fn materialize(tx: &rusqlite::Transaction<'_>, id: &Id, ev: &Event) -> Result<bool> {
    // Исключённый мог успеть отправить что-то после исключения — пока ключ
    // ещё не сменился. В логе это остаётся, но силы не имеет.
    if governance::silenced(tx, ev)? {
        return Ok(false);
    }
    match &ev.kind {
        EventKind::SpaceCreate { name } => return governance::claim(tx, ev, name, false),
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
            // Своё удаляет каждый, чужое — тот, кто наводит порядок. Сообщения
            // владельца не трогает никто, кроме него самого.
            let target_author: Option<Id> = tx
                .query_row(
                    "SELECT author FROM messages WHERE id = ?1",
                    params![&target.0[..]],
                    |r| r.get::<_, Vec<u8>>(0),
                )
                .optional()?
                .map(id_from_row);
            let allowed = match target_author {
                None => false,
                Some(author) if author == ev.author => true,
                Some(author) => {
                    governance::moderator_at(tx, ev)?
                        && governance::role(tx, ev.space, author)? != Role::Owner
                }
            };
            let removed = if allowed {
                tx.execute(
                    "UPDATE messages SET deleted = 1, body = '' WHERE id = ?1",
                    params![&target.0[..]],
                )?
            } else {
                0
            };
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
            // Занятое имя перезаписывает только его автор или модератор.
            if !may_touch_emoji(tx, ev, name)? {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO emojis(space, name, hash, sticker, author, added)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(space, name) DO UPDATE SET hash = excluded.hash,
                     sticker = excluded.sticker, author = excluded.author, added = excluded.added",
                params![
                    &ev.space.0[..],
                    name,
                    &hash.0[..],
                    *sticker as i64,
                    &ev.author.0[..],
                    ev.ts
                ],
            )?;
            // Держим ссылку на файл, чтобы уборщик не унёс картинку эмодзи.
            tx.execute(
                "INSERT OR IGNORE INTO attachments(message, hash, name, size, mime)
                 VALUES(?1, ?2, ?3, 0, 'image/*')",
                params![&id.0[..], &hash.0[..], name],
            )?;
        }
        EventKind::ChannelDelete { channel } => {
            // Канал со всей историей убирает только администратор. Событие
            // остаётся в логе, поэтому решение видно и воспроизводимо.
            let created: Option<i64> = tx
                .query_row(
                    "SELECT created_ts FROM channels WHERE id = ?1",
                    params![&channel.0[..]],
                    |r| r.get(0),
                )
                .optional()?;
            let old_ways = governance::legacy(ev.ts) && created.is_some_and(governance::legacy);
            if !old_ways && !governance::moderator_at(tx, ev)? {
                return Ok(false);
            }
            tx.execute(
                "DELETE FROM attachments WHERE message IN
                   (SELECT id FROM messages WHERE channel = ?1)",
                params![&channel.0[..]],
            )?;
            tx.execute(
                "DELETE FROM reactions WHERE target IN
                   (SELECT id FROM messages WHERE channel = ?1)",
                params![&channel.0[..]],
            )?;
            tx.execute(
                "DELETE FROM messages WHERE channel = ?1",
                params![&channel.0[..]],
            )?;
            tx.execute(
                "DELETE FROM reads WHERE channel = ?1",
                params![&channel.0[..]],
            )?;
            tx.execute(
                "DELETE FROM channels WHERE id = ?1",
                params![&channel.0[..]],
            )?;
        }
        EventKind::EmojiRemove { name } => {
            if !may_touch_emoji(tx, ev, name)? {
                return Ok(false);
            }
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
        EventKind::Founded { .. }
        | EventKind::RoleSet { .. }
        | EventKind::MemberRemove { .. }
        | EventKind::KeyRotate { .. }
        | EventKind::InviteCreate { .. }
        | EventKind::InviteRevoke { .. }
        | EventKind::InviteUse { .. } => return governance::materialize(tx, id, ev),
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
    Ok(false)
}

/// Может ли автор события занять или убрать эмодзи с этим именем.
fn may_touch_emoji(tx: &rusqlite::Transaction<'_>, ev: &Event, name: &str) -> Result<bool> {
    let holder: Option<(Option<Vec<u8>>, i64)> = tx
        .query_row(
            "SELECT author, added FROM emojis WHERE space = ?1 AND name = ?2",
            params![&ev.space.0[..], name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match holder {
        None => true,
        Some((Some(author), _)) if Id::from_slice(&author) == Some(ev.author) => true,
        // До выпуска 0.8 набор эмодзи был общим — см. `governance::legacy`.
        Some((_, added)) if governance::legacy(ev.ts) && governance::legacy(added) => true,
        Some(_) => governance::moderator_at(tx, ev)?,
    })
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

impl Store {
    /// Перешифровать базу новым ключом. Сам SQLCipher делает это одной
    /// транзакцией: база либо целиком на новом ключе, либо на старом.
    pub fn rekey(&self, key: &[u8; 32]) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute_batch(&format!("PRAGMA rekey = \"x'{}'\";", hex(key)))?;
        Ok(())
    }
}

/// Ключ SQLCipher — сырые 32 байта, без собственного растягивания пароля:
/// он и так случайный, а код-пароль уже прошёл через Argon2 в сейфе.
fn apply_key(conn: &Connection, key: &[u8; 32]) -> Result<()> {
    conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", hex(key)))?;
    // Ключ проверяется только при первом чтении. Неверный — здесь и ошибка,
    // а не где-то посреди миграции.
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| {
        r.get::<_, i64>(0)
    })
    .map_err(|_| anyhow::anyhow!("базу не открыть этим ключом"))?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Открытая ли база: у неё в начале стоит заголовок SQLite, у зашифрованной —
/// шум.
fn is_plaintext(path: &Path) -> Result<bool> {
    use std::io::Read;
    let mut head = [0u8; 16];
    match std::fs::File::open(path) {
        Ok(mut file) => Ok(file.read_exact(&mut head).is_ok() && &head == b"SQLite format 3\0"),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err.into()),
    }
}

/// Переписать открытую базу в зашифрованную.
///
/// Порядок важен: сначала полностью готовая и проверенная зашифрованная
/// копия, потом подмена. Оборвись процесс на любом шаге — на диске остаётся
/// либо старая база, либо новая, но не половина.
fn encrypt_in_place(path: &Path, key: &[u8; 32]) -> Result<()> {
    let sidecar = |suffix: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        std::path::PathBuf::from(name)
    };
    let fresh = sidecar(".encrypting");
    let _ = std::fs::remove_file(&fresh);
    {
        let plain = Connection::open(path)?;
        // Всё из журнала WAL — в основной файл, иначе свежие записи потеряются.
        plain.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        plain.execute(
            &format!("ATTACH DATABASE ?1 AS sealed KEY \"x'{}'\"", hex(key)),
            params![fresh.to_string_lossy()],
        )?;
        plain.query_row("SELECT sqlcipher_export('sealed')", [], |_| Ok(()))?;
        plain.execute_batch("DETACH DATABASE sealed;")?;
    }
    // Проверяем строго и без поблажек: после подмены открытой базы уже не
    // будет, и усечённая копия (кончилось место, оборвался процесс) стоила бы
    // всей истории.
    {
        let check = Connection::open(&fresh)?;
        apply_key(&check, key)?;
        let verdict: String = check.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if verdict != "ok" {
            anyhow::bail!("зашифрованная копия базы повреждена: {verdict}");
        }
        let plain = Connection::open(path)?;
        let tables: Vec<String> = {
            let mut stmt = plain.prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )?;
            let names = stmt
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            names
        };
        for table in tables {
            let count = format!("SELECT count(*) FROM \"{table}\"");
            let had: i64 = plain.query_row(&count, [], |r| r.get(0))?;
            let has: i64 = check.query_row(&count, [], |r| r.get(0))?;
            if had != has {
                anyhow::bail!("в зашифрованной копии таблица {table}: {has} строк вместо {had}");
            }
        }
        // На диск — с правом записи: Windows отказывает в сбросе файла,
        // открытого только для чтения («отказано в доступе», os error 5).
        std::fs::OpenOptions::new()
            .write(true)
            .open(&fresh)?
            .sync_all()?;
    }
    for suffix in ["-wal", "-shm"] {
        let _ = std::fs::remove_file(sidecar(suffix));
    }
    std::fs::rename(&fresh, path)?;
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        std::fs::File::open(dir)?.sync_all()?;
    }
    tracing::info!("база переписана в зашифрованную");
    Ok(())
}

fn id_from_row(raw: Vec<u8>) -> Id {
    Id::from_slice(&raw).unwrap_or(Id::ZERO)
}

fn key_from_row(raw: Vec<u8>) -> [u8; 32] {
    raw.try_into().unwrap_or([0u8; 32])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bred-db-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("bred.sqlite")
    }

    #[test]
    fn old_plaintext_base_is_encrypted_without_losing_anything() {
        let path = temp_db();
        {
            // База прежней версии — без ключа.
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "journal_mode", "WAL").unwrap();
            schema::migrate(&conn).unwrap();
            conn.execute("INSERT INTO settings(k, v) VALUES('проверка', x'2a')", [])
                .unwrap();
        }
        assert!(is_plaintext(&path).unwrap());

        let key = [7u8; 32];
        let store = Store::open(&path, &key).unwrap();
        assert_eq!(store.get_setting("проверка").unwrap(), Some(vec![0x2a]));
        drop(store);

        assert!(
            !is_plaintext(&path).unwrap(),
            "на диске больше нет открытого текста"
        );
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw
            .windows("проверка".len())
            .any(|w| w == "проверка".as_bytes()));
        assert!(
            Store::open(&path, &[8u8; 32]).is_err(),
            "чужим ключом не открыть"
        );
        assert!(Store::open(&path, &key).is_ok());
    }

    #[test]
    fn device_belongs_to_account_until_revoked() {
        use crate::domain::account::{DeviceCert, DeviceRevoke};
        let store = Store::in_memory().unwrap();
        let account = Id([1u8; 32]);
        let phone = Id([2u8; 32]);
        let cert = DeviceCert {
            account,
            device: phone,
            name: "телефон".into(),
            issued: 10,
            sig: [0u8; 64],
        };
        store.remember_device(&cert).unwrap();
        assert_eq!(store.account_of(phone).unwrap(), Some(account));
        assert_eq!(store.devices_of(account).unwrap(), vec![cert.clone()]);

        store
            .revoke_device(&DeviceRevoke {
                account,
                device: phone,
                at: 20,
                sig: [0u8; 64],
            })
            .unwrap();
        assert_eq!(
            store.account_of(phone).unwrap(),
            None,
            "потерянный телефон больше ничей"
        );
        assert!(store.devices_of(account).unwrap().is_empty());

        // Нашёлся и привязан заново — новое удостоверение живое.
        store
            .remember_device(&DeviceCert {
                issued: 30,
                ..cert.clone()
            })
            .unwrap();
        assert_eq!(
            store.account_of(phone).unwrap(),
            Some(account),
            "привязан заново"
        );
    }

    #[test]
    fn revocation_that_arrives_first_still_counts() {
        use crate::domain::account::{DeviceCert, DeviceRevoke};
        let store = Store::in_memory().unwrap();
        let account = Id([1u8; 32]);
        let phone = Id([2u8; 32]);
        // Отзыв доехал раньше удостоверения, которое он гасит.
        store
            .revoke_device(&DeviceRevoke {
                account,
                device: phone,
                at: 20,
                sig: [0u8; 64],
            })
            .unwrap();
        store
            .remember_device(&DeviceCert {
                account,
                device: phone,
                name: "телефон".into(),
                issued: 10,
                sig: [0u8; 64],
            })
            .unwrap();
        assert_eq!(
            store.account_of(phone).unwrap(),
            None,
            "порядок доставки не важен"
        );
    }

    const SPACE: SpaceId = Id([5u8; 32]);
    const PEER: Id = Id([7u8; 32]);

    #[test]
    fn peer_seen_only_in_presence_becomes_a_member() {
        let store = Store::in_memory().unwrap();
        // Человек пришёл по ссылке и не написал ни слова: в логе его нет, и
        // единственное, что о нём известно, — удар сердца.
        store
            .remember_peer_seen(SPACE, PEER, "Кирилл", Some(&[1, 2, 3]))
            .unwrap();

        let members = store.members(SPACE).unwrap();
        assert_eq!(members.len(), 1, "он обязан быть в списке участников");
        assert_eq!(members[0].nick, "Кирилл");

        let known = store.known_peers(SPACE).unwrap();
        assert_eq!(
            known,
            vec![(PEER, Some(vec![1, 2, 3]))],
            "и его адрес обязан пережить перезапуск: звать его больше неоткуда"
        );
    }

    #[test]
    fn empty_nick_does_not_erase_a_known_one() {
        let store = Store::in_memory().unwrap();
        store
            .remember_peer_seen(SPACE, PEER, "Кирилл", Some(&[1, 2, 3]))
            .unwrap();
        // Ссылка-приглашение имени не несёт — затирать им живое нельзя.
        store.remember_peer_seen(SPACE, PEER, "", None).unwrap();

        let members = store.members(SPACE).unwrap();
        assert_eq!(members[0].nick, "Кирилл");
        assert_eq!(
            store.known_peers(SPACE).unwrap()[0].1,
            Some(vec![1, 2, 3]),
            "пустой адрес тоже не должен стирать известный"
        );
    }

    #[test]
    fn newer_address_replaces_the_old_one() {
        let store = Store::in_memory().unwrap();
        store
            .remember_peer_seen(SPACE, PEER, "", Some(&[1]))
            .unwrap();
        store
            .remember_peer_seen(SPACE, PEER, "", Some(&[2]))
            .unwrap();
        assert_eq!(store.known_peers(SPACE).unwrap()[0].1, Some(vec![2]));
    }
}

#[cfg(test)]
mod host_tests {
    use super::*;

    #[test]
    fn last_known_address_survives_for_next_time() {
        let store = Store::in_memory().unwrap();
        store
            .remember_host("euc1-1.relay.n0.iroh.link.", 4, &["91.99.237.97".into()])
            .unwrap();
        assert_eq!(
            store.known_host("euc1-1.relay.n0.iroh.link.", 4).unwrap(),
            vec!["91.99.237.97".to_string()],
            "адрес ретранслятора обязан пережить падение DNS"
        );
    }

    #[test]
    fn unknown_host_is_empty_not_an_error() {
        let store = Store::in_memory().unwrap();
        assert!(store.known_host("нет.такого.имени.", 4).unwrap().is_empty());
    }

    #[test]
    fn newer_answer_replaces_the_old_one() {
        let store = Store::in_memory().unwrap();
        store
            .remember_host("relay.", 4, &["1.1.1.1".into()])
            .unwrap();
        store
            .remember_host("relay.", 4, &["2.2.2.2".into(), "3.3.3.3".into()])
            .unwrap();
        assert_eq!(
            store.known_host("relay.", 4).unwrap(),
            vec!["2.2.2.2".to_string(), "3.3.3.3".to_string()]
        );
    }

    #[test]
    fn empty_answer_does_not_erase_what_we_had() {
        let store = Store::in_memory().unwrap();
        store
            .remember_host("relay.", 4, &["1.1.1.1".into()])
            .unwrap();
        store.remember_host("relay.", 4, &[]).unwrap();
        assert_eq!(
            store.known_host("relay.", 4).unwrap(),
            vec!["1.1.1.1".to_string()]
        );
    }
}
