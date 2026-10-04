//! Власть в пространстве: кто владелец, кто администратор, кого исключили,
//! какие ключи и приглашения действуют.
//!
//! Правила проверяет каждый участник сам, и прийти все должны к одному. Поэтому
//! решение о событии зависит только от самого события и набора остальных, но
//! не от того, в каком порядке они доехали, и не от логических часов — их
//! выбирает автор:
//!
//! - **владелец** считается по всем заявкам сразу;
//! - **права и исключение** привязаны к номеру события в логе его автора:
//!   решение владельца несёт отметку «последнее, что я видел от этого
//!   человека», и всё, что тот подписал позже, оценивается по решению;
//! - **ключи** выстраиваются в цепочку, и голова цепочки одна у всех.
//!
//! Если решение о правах доехало позже событий, которых оно касается,
//! производные таблицы пространства пересобираются из лога.

use anyhow::Result;
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::HashSet;

use super::{id_from_row, key_from_row, Store};
use crate::domain::{governance, Event, EventKind, Id, SignedEvent, SpaceId};

pub(super) const SCHEMA: &str = r#"
-- Заявки на владение: «Основано» (доказуемо) и старое «Создано» (на слово).
CREATE TABLE IF NOT EXISTS space_claims (
    space   BLOB NOT NULL,
    author  BLOB NOT NULL,
    founded INTEGER NOT NULL,
    PRIMARY KEY (space, author, founded)
);

-- Решения владельца о ролях — в порядке его собственного лога.
CREATE TABLE IF NOT EXISTS role_grants (
    space     BLOB NOT NULL,
    member    BLOB NOT NULL,
    owner_seq INTEGER NOT NULL,
    admin     INTEGER NOT NULL,
    cut       INTEGER NOT NULL,
    PRIMARY KEY (space, member, owner_seq)
);

-- Исключённые: события с номером больше `cut` силы не имеют.
CREATE TABLE IF NOT EXISTS removals (
    space  BLOB NOT NULL,
    member BLOB NOT NULL,
    cut    INTEGER NOT NULL,
    PRIMARY KEY (space, member)
);

-- Смены ключа от тех, кто вправе их делать. Действует голова цепочки.
CREATE TABLE IF NOT EXISTS key_rotations (
    id        BLOB PRIMARY KEY,
    space     BLOB NOT NULL,
    epoch     INTEGER NOT NULL,
    prev      BLOB,
    author    BLOB NOT NULL,
    ephemeral BLOB NOT NULL,
    wraps     BLOB NOT NULL,
    checksum  BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS key_rotations_space ON key_rotations(space);

-- Ключи прошлых эпох. Ими открываются только запросы отставших — ответ им
-- одна раздача нового ключа, и больше ничего.
CREATE TABLE IF NOT EXISTS past_keys (
    space BLOB NOT NULL,
    key   BLOB NOT NULL,
    PRIMARY KEY (space, key)
);

CREATE TABLE IF NOT EXISTS invites (
    id      BLOB PRIMARY KEY,
    space   BLOB NOT NULL,
    author  BLOB NOT NULL,
    proof   BLOB NOT NULL,
    expires INTEGER NOT NULL,
    uses    INTEGER NOT NULL,
    created INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS invites_space ON invites(space);

-- Отзыв отдельно от приглашения: в любом порядке доезжают оба.
CREATE TABLE IF NOT EXISTS invite_revokes (
    invite BLOB NOT NULL,
    author BLOB NOT NULL,
    PRIMARY KEY (invite, author)
);

-- Кто кого впустил. Вход засчитывается, когда гость заговорил в пространстве:
-- иначе любой участник выжег бы лимит записями о несуществующих гостях.
CREATE TABLE IF NOT EXISTS invite_entries (
    invite BLOB NOT NULL,
    member BLOB NOT NULL,
    author BLOB NOT NULL,
    PRIMARY KEY (invite, member, author)
);
"#;

/// Роль участника в пространстве сейчас.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Admin,
    Member,
}

/// Смена ключа из головы цепочки.
#[derive(Debug, Clone)]
pub struct Rotation {
    pub id: Id,
    pub epoch: u64,
    pub prev: Option<Id>,
    pub author: Id,
    pub ephemeral: Id,
    pub wraps: Vec<governance::KeyWrap>,
    pub check: Id,
}

/// Приглашение и его состояние сейчас.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InviteRow {
    pub id: Id,
    pub space: SpaceId,
    pub author: Id,
    #[serde(skip)]
    pub proof: Id,
    pub expires: i64,
    pub uses: u32,
    pub used: u32,
    /// Погашено автором или модератором — или автора исключили.
    pub revoked: bool,
    pub created: i64,
}

impl InviteRow {
    /// Можно ли по нему войти прямо сейчас.
    pub fn live(&self, now: i64) -> bool {
        !self.revoked && !self.expired(now) && !self.exhausted()
    }

    pub fn expired(&self, now: i64) -> bool {
        self.expires != 0 && now >= self.expires
    }

    pub fn exhausted(&self) -> bool {
        self.uses != 0 && self.used >= self.uses
    }
}

/// До этого момента (4 октября 2026, выпуск 0.8) в пространствах не было
/// прав: каналы и эмодзи мог убрать любой участник, и люди так и делали.
/// Пересудить ту историю по новым правилам значило бы вернуть из небытия
/// удалённые каналы — поэтому то, что сделано и создано до выпуска, живёт по
/// прежним правилам, а всё новое — по новым.
///
/// Часы события ставит автор, и подправленный клиент может «состарить»
/// удаление. Поэтому старое правило требует, чтобы до выпуска появилось и то,
/// что удаляют: канал, созданный после, так не тронуть.
pub(super) const LEGACY_UNTIL_MS: i64 = 1_791_072_000_000;

pub(super) fn legacy(ts: i64) -> bool {
    ts < LEGACY_UNTIL_MS
}

// ── правила ─────────────────────────────────────────────────────────────────

/// Владелец пространства, если его можно установить однозначно.
///
/// Доказуемая заявка («Основано») бьёт любые другие: её не подделать. Без неё
/// — пространства старых версий — верим «Создано», но только если оно одно:
/// две заявки от разных людей значат, что кто-то врёт, и тогда власти нет ни
/// у кого. Так подделка может разве что выключить модерацию, но не захватить её.
pub(super) fn owner(tx: &Transaction<'_>, space: SpaceId) -> Result<Option<Id>> {
    let mut stmt = tx.prepare_cached(
        "SELECT author, founded FROM space_claims WHERE space = ?1 ORDER BY founded DESC",
    )?;
    let claims: Vec<(Id, bool)> = stmt
        .query_map(params![&space.0[..]], |r| {
            Ok((id_from_row(r.get(0)?), r.get::<_, i64>(1)? != 0))
        })?
        .collect::<Result<_, _>>()?;

    if let Some((author, _)) = claims.iter().find(|(_, founded)| *founded) {
        return Ok(Some(*author));
    }
    let mut legacy = claims.iter().map(|(author, _)| *author);
    let first = legacy.next();
    Ok(match (first, legacy.next()) {
        (Some(only), None) => Some(only),
        _ => None,
    })
}

/// Владелец доказуем. Только в таких пространствах можно исключать и менять
/// ключ: у старых владелец держится на слове, и подделавший заявку стёр бы
/// чужие данные исключением раньше, чем подделка вскроется.
pub(super) fn founded(tx: &Transaction<'_>, space: SpaceId) -> Result<bool> {
    Ok(tx
        .query_row(
            "SELECT 1 FROM space_claims WHERE space = ?1 AND founded = 1",
            params![&space.0[..]],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn is_direct(tx: &Transaction<'_>, space: SpaceId) -> Result<bool> {
    Ok(tx
        .query_row(
            "SELECT direct IS NOT NULL FROM spaces WHERE id = ?1",
            params![&space.0[..]],
            |r| r.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false))
}

/// Отрезки номеров событий участника, на которых он администратор:
/// `(после, до включительно)`, верхняя граница `None` — по сей день.
fn admin_spans(tx: &Transaction<'_>, space: SpaceId, who: Id) -> Result<Vec<(u64, Option<u64>)>> {
    let mut stmt = tx.prepare_cached(
        "SELECT admin, cut FROM role_grants WHERE space = ?1 AND member = ?2 ORDER BY owner_seq",
    )?;
    let decisions: Vec<(bool, u64)> = stmt
        .query_map(params![&space.0[..], &who.0[..]], |r| {
            Ok((r.get::<_, bool>(0)?, r.get::<_, i64>(1)? as u64))
        })?
        .collect::<Result<_, _>>()?;
    let mut spans = Vec::new();
    let mut open: Option<u64> = None;
    for (admin, cut) in decisions {
        match (admin, open) {
            (true, None) => open = Some(cut),
            (false, Some(start)) => {
                spans.push((start, Some(cut)));
                open = None;
            }
            _ => {}
        }
    }
    if let Some(start) = open {
        spans.push((start, None));
    }
    Ok(spans)
}

/// Был ли человек администратором, когда подписывал своё событие номер `seq`.
fn admin_at(tx: &Transaction<'_>, space: SpaceId, who: Id, seq: u64) -> Result<bool> {
    Ok(admin_spans(tx, space, who)?
        .iter()
        .any(|(after, until)| seq > *after && until.is_none_or(|end| seq <= end)))
}

/// Роль сейчас — для показа и для правила «администратор не исключает
/// администратора».
pub(super) fn role(tx: &Transaction<'_>, space: SpaceId, who: Id) -> Result<Role> {
    if owner(tx, space)? == Some(who) {
        return Ok(Role::Owner);
    }
    let admin = admin_spans(tx, space, who)?
        .last()
        .is_some_and(|(_, until)| until.is_none());
    Ok(if admin { Role::Admin } else { Role::Member })
}

/// Вправе ли автор события наводить порядок: удалять каналы и чужие
/// сообщения. В личной переписке на двоих — оба.
pub(super) fn moderator_at(tx: &Transaction<'_>, ev: &Event) -> Result<bool> {
    Ok(is_direct(tx, ev.space)?
        || owner(tx, ev.space)? == Some(ev.author)
        || admin_at(tx, ev.space, ev.author, ev.seq)?)
}

/// То же, но сейчас — для решений, которые принимаются при чтении.
pub(super) fn moderator_now(tx: &Transaction<'_>, space: SpaceId, who: Id) -> Result<bool> {
    Ok(is_direct(tx, space)? || role(tx, space, who)? != Role::Member)
}

/// Подписано ли событие автором уже после его исключения.
pub(super) fn silenced(tx: &Transaction<'_>, ev: &Event) -> Result<bool> {
    Ok(tx
        .query_row(
            "SELECT 1 FROM removals WHERE space = ?1 AND member = ?2 AND cut < ?3",
            params![&ev.space.0[..], &ev.author.0[..], ev.seq as i64],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Записать заявку на владение. Название пространства меняет только владелец.
/// `true` — владелец от этого сменился.
pub(super) fn claim(tx: &Transaction<'_>, ev: &Event, name: &str, founded: bool) -> Result<bool> {
    let before = owner(tx, ev.space)?;
    tx.execute(
        "INSERT OR IGNORE INTO space_claims(space, author, founded) VALUES(?1, ?2, ?3)",
        params![&ev.space.0[..], &ev.author.0[..], founded as i64],
    )?;
    let after = owner(tx, ev.space)?;
    if after == Some(ev.author) {
        tx.execute(
            "UPDATE spaces SET name = ?2 WHERE id = ?1",
            params![&ev.space.0[..], name],
        )?;
    }
    Ok(before != after)
}

/// Новые виды событий: роли, исключения, ключи и приглашения.
///
/// Возвращает, изменились ли чьи-то права. Только такое событие может
/// потребовать пересборки — недействительное не меняет ничего и пересборку
/// не вызывает, сколько его ни присылай.
pub(super) fn materialize(tx: &Transaction<'_>, id: &Id, ev: &Event) -> Result<bool> {
    let space = ev.space;
    let direct = is_direct(tx, space)?;
    let changed = match &ev.kind {
        EventKind::Founded { name, nonce } => {
            governance::founded_id(ev.author, nonce) == space && claim(tx, ev, name, true)?
        }
        EventKind::RoleSet { member, admin, cut } => {
            let owner = owner(tx, space)?;
            if direct || owner != Some(ev.author) || owner == Some(*member) {
                return Ok(false);
            }
            tx.execute(
                "INSERT OR IGNORE INTO role_grants(space, member, owner_seq, admin, cut)
                 VALUES(?1, ?2, ?3, ?4, ?5)",
                params![
                    &space.0[..],
                    &member.0[..],
                    ev.seq as i64,
                    *admin as i64,
                    *cut as i64
                ],
            )? > 0
        }
        EventKind::MemberRemove { member, cut } => {
            if direct || !founded(tx, space)? || *member == ev.author {
                return Ok(false);
            }
            let by_owner = owner(tx, space)? == Some(ev.author);
            let allowed = match role(tx, space, *member)? {
                Role::Owner => false,
                Role::Admin => by_owner,
                Role::Member => by_owner || admin_at(tx, space, ev.author, ev.seq)?,
            };
            if !allowed {
                return Ok(false);
            }
            // Два исключения одного человека — действует более раннее. Двигать
            // отметку у уже исключённого вправе только владелец: иначе
            // администратор гонял бы пересборку, опуская её по единице.
            let already = tx
                .query_row(
                    "SELECT 1 FROM removals WHERE space = ?1 AND member = ?2",
                    params![&space.0[..], &member.0[..]],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if already && !by_owner {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO removals(space, member, cut) VALUES(?1, ?2, ?3)
                 ON CONFLICT(space, member) DO UPDATE SET cut = excluded.cut
                 WHERE excluded.cut < removals.cut",
                params![&space.0[..], &member.0[..], *cut as i64],
            )? > 0
        }
        EventKind::KeyRotate {
            epoch,
            prev,
            ephemeral,
            wraps,
            check,
        } => {
            if !direct && founded(tx, space)? && moderator_at(tx, ev)? {
                tx.execute(
                    "INSERT OR IGNORE INTO key_rotations
                       (id, space, epoch, prev, author, ephemeral, wraps, checksum)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        &id.0[..],
                        &space.0[..],
                        i64::try_from(*epoch).unwrap_or(i64::MAX),
                        prev.map(|p| p.0.to_vec()),
                        &ev.author.0[..],
                        &ephemeral.0[..],
                        postcard::to_stdvec(wraps)?,
                        &check.0[..]
                    ],
                )?;
            }
            false
        }
        EventKind::InviteCreate {
            proof,
            expires,
            uses,
        } => {
            if !direct {
                tx.execute(
                    "INSERT OR IGNORE INTO invites(id, space, author, proof, expires, uses, created)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        &id.0[..],
                        &space.0[..],
                        &ev.author.0[..],
                        &proof.0[..],
                        expires,
                        *uses as i64,
                        ev.ts
                    ],
                )?;
            }
            false
        }
        EventKind::InviteRevoke { invite } => {
            tx.execute(
                "INSERT OR IGNORE INTO invite_revokes(invite, author) VALUES(?1, ?2)",
                params![&invite.0[..], &ev.author.0[..]],
            )?;
            false
        }
        EventKind::InviteUse { invite, member } => {
            tx.execute(
                "INSERT OR IGNORE INTO invite_entries(invite, member, author) VALUES(?1, ?2, ?3)",
                params![&invite.0[..], &member.0[..], &ev.author.0[..]],
            )?;
            false
        }
        _ => false,
    };
    Ok(changed)
}

/// Касается ли изменившееся решение уже записанного.
pub(super) fn affects_stored(tx: &Transaction<'_>, ev: &Event) -> Result<bool> {
    let (member, cut) = match &ev.kind {
        // Сменился владелец — пересчитать придётся всё.
        EventKind::Founded { .. } | EventKind::SpaceCreate { .. } => return Ok(true),
        // Роль меняет и цену чужих решений о человеке: исключение админа,
        // отвергнутое раньше, после снятия прав становится действительным.
        // Решения владельца редки, пересобрать по каждому не жалко.
        EventKind::RoleSet { .. } => return Ok(true),
        EventKind::MemberRemove { member, cut } => (*member, *cut),
        _ => return Ok(false),
    };
    Ok(tx
        .query_row(
            "SELECT 1 FROM events WHERE space = ?1 AND author = ?2 AND seq > ?3 LIMIT 1",
            params![&ev.space.0[..], &member.0[..], cut as i64],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Пересобрать производные таблицы пространства из лога в глобальном порядке.
///
/// Дорого, но редко: только когда действительное решение о правах приехало
/// позже событий, которых оно касается. Заявки на владение собираются заново
/// до разбора — владелец должен быть известен, прежде чем первое событие
/// начнут проверять.
pub(super) fn rebuild(tx: &Transaction<'_>, space: SpaceId) -> Result<()> {
    let id = &space.0[..];
    tx.execute(
        "DELETE FROM reactions WHERE target IN (SELECT id FROM messages WHERE space = ?1)",
        params![id],
    )?;
    tx.execute(
        "DELETE FROM attachments WHERE message IN (SELECT id FROM events WHERE space = ?1)",
        params![id],
    )?;
    tx.execute(
        "DELETE FROM invite_entries WHERE invite IN (SELECT id FROM events WHERE space = ?1)",
        params![id],
    )?;
    tx.execute(
        "DELETE FROM invite_revokes WHERE invite IN (SELECT id FROM events WHERE space = ?1)",
        params![id],
    )?;
    for table in [
        "messages",
        "channels",
        "emojis",
        "space_claims",
        "role_grants",
        "removals",
        "key_rotations",
        "invites",
    ] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE space = ?1"),
            params![id],
        )?;
    }

    let mut stmt =
        tx.prepare("SELECT id, raw FROM events WHERE space = ?1 ORDER BY lamport, author, seq")?;
    let rows: Vec<(Vec<u8>, Vec<u8>)> = stmt
        .query_map(params![id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    drop(stmt);
    let events: Vec<(Id, Event)> = rows
        .into_iter()
        .map(|(event_id, raw)| {
            let signed: SignedEvent = postcard::from_bytes(&raw)?;
            Ok((id_from_row(event_id), signed.event))
        })
        .collect::<Result<_>>()?;

    for (_, event) in &events {
        let founded = match &event.kind {
            EventKind::Founded { nonce, .. } => {
                if governance::founded_id(event.author, nonce) != space {
                    continue;
                }
                true
            }
            EventKind::SpaceCreate { .. } => false,
            _ => continue,
        };
        tx.execute(
            "INSERT OR IGNORE INTO space_claims(space, author, founded) VALUES(?1, ?2, ?3)",
            params![id, &event.author.0[..], founded as i64],
        )?;
    }
    // Решения о ролях и исключения — тоже до разбора остального: от них
    // зависит, чего стоят события, а их место в логе здесь не важно. Порядок
    // проходов — по зависимостям: роли раздаёт только владелец; администратора
    // исключает только владелец; значит, после двух первых проходов уже
    // известно, кто из администраторов сам исключён, и третий проход это учтёт.
    let owner = owner(tx, space)?;
    let by_owner = |event: &Event| Some(event.author) == owner;
    let passes: [&dyn Fn(&Event) -> bool; 3] = [
        &|e| matches!(e.kind, EventKind::RoleSet { .. }),
        &|e| matches!(e.kind, EventKind::MemberRemove { .. }) && by_owner(e),
        &|e| matches!(e.kind, EventKind::MemberRemove { .. }) && !by_owner(e),
    ];
    for pass in passes {
        for (event_id, event) in &events {
            if pass(event) && !silenced(tx, event)? {
                materialize(tx, event_id, event)?;
            }
        }
    }
    for (event_id, event) in &events {
        super::materialize(tx, event_id, event)?;
    }
    tracing::info!(space = %space.short(), "права в пространстве пересчитаны по логу");
    Ok(())
}

/// Разовый пересчёт при переходе на протокол 4.
///
/// История, записанная прежними версиями, не знает ни заявок на владение, ни
/// новых правил. Пересобираем её по ним один раз — так у старожилов то же
/// самое, что увидит новичок, получивший ту же историю с нуля.
pub(super) fn upgrade(conn: &mut rusqlite::Connection) -> Result<()> {
    const DONE: &str = "store.governance_v2";
    let done: Option<Vec<u8>> = conn
        .query_row("SELECT v FROM settings WHERE k = ?1", params![DONE], |r| {
            r.get(0)
        })
        .optional()?;
    if done.is_some() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    let spaces: Vec<Vec<u8>> = {
        let mut stmt = tx.prepare("SELECT id FROM spaces")?;
        let rows = stmt
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        rows
    };
    for space in &spaces {
        rebuild(&tx, id_from_row(space.clone()))?;
    }
    tx.execute(
        "INSERT INTO settings(k, v) VALUES(?1, x'01') ON CONFLICT(k) DO NOTHING",
        params![DONE],
    )?;
    tx.commit()?;
    if !spaces.is_empty() {
        tracing::info!(
            spaces = spaces.len(),
            "история пересчитана по правилам протокола 4"
        );
    }
    Ok(())
}

/// Голова цепочки смен ключа.
///
/// Цепочка растёт от исходного ключа: у каждой смены `prev` — предыдущая
/// голова, `epoch` — длина цепочки. Из нескольких смен с одним `prev`
/// побеждает смена владельца, а среди прочих — с меньшим идентификатором.
/// Администратор не может ни перескочить номера, ни обойти владельца: смена,
/// в раздаче которой владельца нет, недействительна.
fn rotation_head(tx: &Transaction<'_>, space: SpaceId) -> Result<Option<Rotation>> {
    let owner = owner(tx, space)?;
    let mut stmt = tx.prepare_cached(
        "SELECT id, epoch, prev, author, ephemeral, wraps, checksum
         FROM key_rotations WHERE space = ?1",
    )?;
    let rows: Vec<Rotation> = stmt
        .query_map(params![&space.0[..]], |r| {
            Ok((
                id_from_row(r.get(0)?),
                r.get::<_, i64>(1)?,
                r.get::<_, Option<Vec<u8>>>(2)?,
                id_from_row(r.get(3)?),
                id_from_row(r.get(4)?),
                r.get::<_, Vec<u8>>(5)?,
                id_from_row(r.get(6)?),
            ))
        })?
        .filter_map(|row| {
            let (id, epoch, prev, author, ephemeral, wraps, check) = row.ok()?;
            Some(Rotation {
                id,
                epoch: u64::try_from(epoch).ok()?,
                prev: prev.and_then(|raw| Id::from_slice(&raw)),
                author,
                ephemeral,
                wraps: postcard::from_bytes(&wraps).ok()?,
                check,
            })
        })
        .collect();
    drop(stmt);

    let Some(owner) = owner else {
        return Ok(None);
    };
    let mut head: Option<Rotation> = None;
    loop {
        let prev = head.as_ref().map(|h| h.id);
        let epoch = head.as_ref().map_or(1, |h| h.epoch + 1);
        let next = rows
            .iter()
            .filter(|r| r.prev == prev && r.epoch == epoch)
            .filter(|r| r.author == owner || r.wraps.iter().any(|w| w.member == owner))
            .min_by_key(|r| (r.author != owner, r.id.0));
        match next {
            Some(next) => head = Some(next.clone()),
            None => return Ok(head),
        }
    }
}

// ── чтение ──────────────────────────────────────────────────────────────────

impl Store {
    pub fn role(&self, space: SpaceId, who: Id) -> Result<Role> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        role(&tx, space, who)
    }

    /// Владелец, если он установлен. `None` — заявок нет или они спорят.
    pub fn owner(&self, space: SpaceId) -> Result<Option<Id>> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        owner(&tx, space)
    }

    /// Доказуем ли владелец — только тогда можно исключать и менять ключ.
    pub fn founded(&self, space: SpaceId) -> Result<bool> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        founded(&tx, space)
    }

    pub fn is_removed(&self, space: SpaceId, who: Id) -> Result<bool> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT 1 FROM removals WHERE space = ?1 AND member = ?2",
                params![&space.0[..], &who.0[..]],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn removed(&self, space: SpaceId) -> Result<HashSet<Id>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT member FROM removals WHERE space = ?1")?;
        let rows = stmt
            .query_map(params![&space.0[..]], |r| Ok(id_from_row(r.get(0)?)))?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Отметка для решения о человеке: до какого номера его лог у нас
    /// непрерывен. Не наибольший номер — тот можно заранее «застолбить»
    /// событием с номером в миллиард и растянуть себе права на всё до него.
    pub fn decision_cut(&self, space: SpaceId, who: Id) -> Result<u64> {
        let conn = self.conn.lock();
        super::contiguous_prefix(&conn, space, who)
    }

    /// Роли всех, у кого она не обычная.
    pub fn roles(&self, space: SpaceId) -> Result<Vec<(Id, Role)>> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut out = Vec::new();
        if let Some(owner) = owner(&tx, space)? {
            out.push((owner, Role::Owner));
        }
        let mut stmt = tx.prepare("SELECT DISTINCT member FROM role_grants WHERE space = ?1")?;
        let members: Vec<Id> = stmt
            .query_map(params![&space.0[..]], |r| Ok(id_from_row(r.get(0)?)))?
            .collect::<Result<_, _>>()?;
        drop(stmt);
        for member in members {
            if role(&tx, space, member)? == Role::Admin {
                out.push((member, Role::Admin));
            }
        }
        Ok(out)
    }

    /// Голова цепочки смен ключа — см. `rotation_head`.
    pub fn rotation_head(&self, space: SpaceId) -> Result<Option<Rotation>> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        rotation_head(&tx, space)
    }

    /// Знает ли действующий ключ кто-то из исключённых. Тогда его пора
    /// сменить — например, после двух одновременных исключений, где каждая
    /// раздача оставила ключ другому исключённому.
    pub fn key_compromised(&self, space: SpaceId) -> Result<bool> {
        let removed = self.removed(space)?;
        if removed.is_empty() {
            return Ok(false);
        }
        Ok(match self.rotation_head(space)? {
            None => true,
            Some(head) => head.wraps.iter().any(|w| removed.contains(&w.member)),
        })
    }

    /// На какой смене ключа мы сейчас: эпоха и событие, из которого он взят.
    pub fn key_epoch(&self, space: SpaceId) -> Result<(u64, Option<Id>)> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT epoch, key_event FROM spaces WHERE id = ?1",
                params![&space.0[..]],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)? as u64,
                        r.get::<_, Option<Vec<u8>>>(1)?
                            .and_then(|raw| Id::from_slice(&raw)),
                    ))
                },
            )
            .optional()?
            .unwrap_or((0, None)))
    }

    /// Перейти на новый ключ. Прежний остаётся в `past_keys`: по нему ещё
    /// придут отставшие, и им надо ответить раздачей нового.
    pub fn adopt_key(
        &self,
        space: SpaceId,
        key: &[u8; 32],
        epoch: u64,
        event: Option<Id>,
    ) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO past_keys(space, key) SELECT id, key FROM spaces WHERE id = ?1",
            params![&space.0[..]],
        )?;
        tx.execute(
            "UPDATE spaces SET key = ?2, epoch = ?3, key_event = ?4 WHERE id = ?1",
            params![
                &space.0[..],
                &key[..],
                i64::try_from(epoch).unwrap_or(i64::MAX),
                event.map(|e| e.0.to_vec())
            ],
        )?;
        tx.execute(
            "DELETE FROM past_keys WHERE space = ?1 AND key = ?2",
            params![&space.0[..], &key[..]],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn past_keys(&self) -> Result<Vec<(SpaceId, [u8; 32])>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT space, key FROM past_keys")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((id_from_row(r.get(0)?), key_from_row(r.get(1)?)))
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// События, которые нужны отставшему, чтобы догнать ключ: заявки,
    /// роли, исключения и раздачи. Переписки среди них нет.
    pub fn governance_events(&self, space: SpaceId) -> Result<Vec<SignedEvent>> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare("SELECT raw FROM events WHERE space = ?1 ORDER BY lamport, author, seq")?;
        let rows = stmt.query_map(params![&space.0[..]], |r| r.get::<_, Vec<u8>>(0))?;
        let mut out = Vec::new();
        for raw in rows {
            let signed: SignedEvent = postcard::from_bytes(&raw?)?;
            if matches!(
                signed.event.kind,
                EventKind::Founded { .. }
                    | EventKind::SpaceCreate { .. }
                    | EventKind::RoleSet { .. }
                    | EventKind::MemberRemove { .. }
                    | EventKind::KeyRotate { .. }
            ) {
                out.push(signed);
            }
        }
        Ok(out)
    }

    /// Впускали ли мы сами этого человека по этому приглашению. Только своей
    /// записи и верим: чужая запись «он уже входил» открыла бы обход отзыва.
    pub fn admitted_by_me(&self, invite: Id, member: Id) -> Result<bool> {
        let me = *self.me.lock();
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT 1 FROM invite_entries WHERE invite = ?1 AND member = ?2 AND author = ?3",
                params![&invite.0[..], &member.0[..], &me.0[..]],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    pub fn invite(&self, id: Id) -> Result<Option<InviteRow>> {
        Ok(self.invites_where("i.id = ?1", &id.0[..])?.pop())
    }

    pub fn invites(&self, space: SpaceId) -> Result<Vec<InviteRow>> {
        self.invites_where("i.space = ?1", &space.0[..])
    }

    fn invites_where(&self, filter: &str, value: &[u8]) -> Result<Vec<InviteRow>> {
        let me = *self.me.lock();
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        // Вход считается, если его записали мы сами или если гость уже
        // что-то подписал в пространстве: выдуманный гость не заговорит.
        let mut stmt = tx.prepare(&format!(
            "SELECT i.id, i.space, i.author, i.proof, i.expires, i.uses, i.created,
                    (SELECT COUNT(DISTINCT e.member) FROM invite_entries e
                      WHERE e.invite = i.id
                        AND NOT EXISTS (SELECT 1 FROM removals x
                                         WHERE x.space = i.space AND x.member = e.author)
                        AND (e.author = ?2 OR EXISTS (SELECT 1 FROM events v
                                         WHERE v.space = i.space AND v.author = e.member))),
                    EXISTS (SELECT 1 FROM removals x WHERE x.space = i.space AND x.member = i.author)
             FROM invites i WHERE {filter} ORDER BY i.created DESC"
        ))?;
        let rows: Vec<InviteRow> = stmt
            .query_map(params![value, &me.0[..]], |r| {
                Ok(InviteRow {
                    id: id_from_row(r.get(0)?),
                    space: id_from_row(r.get(1)?),
                    author: id_from_row(r.get(2)?),
                    proof: id_from_row(r.get(3)?),
                    expires: r.get(4)?,
                    uses: r.get::<_, i64>(5)? as u32,
                    created: r.get(6)?,
                    used: r.get::<_, i64>(7)? as u32,
                    // Приглашение исключённого гаснет вместе с ним: иначе он
                    // впустил бы себя же под новым именем.
                    revoked: r.get::<_, bool>(8)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        drop(stmt);

        // Отзыв засчитывается от автора приглашения или от того, кто вправе
        // наводить порядок. Чужой «отзыв» от рядового участника — шум.
        let mut out = Vec::with_capacity(rows.len());
        for mut row in rows {
            let mut stmt =
                tx.prepare_cached("SELECT author FROM invite_revokes WHERE invite = ?1")?;
            let revokers: Vec<Id> = stmt
                .query_map(params![&row.id.0[..]], |r| Ok(id_from_row(r.get(0)?)))?
                .collect::<Result<_, _>>()?;
            drop(stmt);
            for by in revokers {
                if row.revoked {
                    break;
                }
                row.revoked = by == row.author || moderator_now(&tx, row.space, by)?;
            }
            out.push(row);
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "governance_tests.rs"]
mod tests;
