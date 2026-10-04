//! Пространства и власть в них: основание, приглашения, роли, исключение и
//! смена ключа. Правила — в `store::governance`, здесь только действия.

use anyhow::{anyhow, bail, Result};
use serde::Serialize;

use super::{validate_name, App, LINK_WAIT};
use crate::{
    domain::{
        governance::{founded_id, invite_proof, Ticket},
        now_ms, EventKind, Id, Space, SpaceId,
    },
    net::{hide_ip_enabled, HIDE_IP_SETTING},
    store::Role,
};

/// Срок приглашения по умолчанию — как в Дискорде: неделя.
const DEFAULT_INVITE_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Дольше месяца приглашение не живёт, если только оно не бессрочное явно.
const MAX_INVITE_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;
const MAX_INVITE_USES: u32 = 1000;
const OWNERLESS: &str =
    "исключать и менять ключ можно только в пространствах, созданных в 0.8 или позже: \
     у старых владелец держится на слове, и подделка заявки стоила бы кому-то стёртой истории";
/// Сколько соседей, кроме себя, кладём в ссылку как запасных привратников.
const SPARE_DOORMEN: usize = 2;

/// Каким будет приглашение.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteOptions {
    /// Сколько живёт, мс; 0 — бессрочно.
    pub expires_in: i64,
    /// Сколько раз по нему можно войти; 0 — без ограничения.
    pub uses: u32,
}

impl Default for InviteOptions {
    fn default() -> Self {
        Self {
            expires_in: DEFAULT_INVITE_TTL_MS,
            uses: 0,
        }
    }
}

/// Приглашение для экрана «Приглашения».
#[derive(Debug, Clone, Serialize)]
pub struct InviteView {
    pub id: Id,
    pub author: Id,
    pub author_nick: String,
    pub created: i64,
    pub expires: i64,
    pub uses: u32,
    pub used: u32,
    pub revoked: bool,
    pub live: bool,
    pub mine: bool,
}

/// Кто я в пространстве и что мне здесь можно.
#[derive(Debug, Clone, Serialize)]
pub struct GovernanceView {
    pub role: Role,
    pub owner: Option<Id>,
    /// Владелец доказуем: пространство основано в 0.8 или позже. Только в
    /// таких можно исключать и менять ключ.
    pub founded: bool,
    pub direct: bool,
    /// Номер ключа: растёт с каждой сменой.
    pub epoch: u64,
    pub can_moderate: bool,
}

/// Настройки приватности сети и устройства.
#[derive(Debug, Clone, Serialize)]
pub struct PrivacyView {
    /// Что выбрано в настройках.
    pub hide_ip: bool,
    /// Что действует сейчас: настройка применяется при запуске.
    pub active: bool,
    /// Закрыта ли база код-паролем.
    pub passcode: bool,
    /// Где лежит ключ базы без кода: «system» — в хранилище системы,
    /// «file» — в файле рядом с базой.
    pub key_storage: &'static str,
}

impl App {
    /// Основать пространство. Идентификатор выводится из нашего ключа и соли,
    /// поэтому владельца проверит любой и подделать его нельзя.
    pub async fn create_space(&self, name: &str) -> Result<SpaceId> {
        let name = validate_name(name, "название пространства")?;
        let nonce: [u8; 32] = rand::random();
        let space = Space {
            id: founded_id(self.me(), &nonce),
            name: name.clone(),
            key: rand::random(),
            direct: None,
        };
        self.store.save_space(&space)?;
        self.ctx.add_space(space.clone());
        self.net.join(space.clone()).await?;

        self.commit_and_publish(space.id, EventKind::Founded { name, nonce })
            .await?;
        // Пространство без каналов бесполезно — сразу заводим общий.
        self.create_channel(space.id, "общий-канал", "общее", false)
            .await?;
        self.announce_profile(space.id).await?;
        Ok(space.id)
    }

    /// Войти по приглашению: постучаться к участникам, получить ключ.
    pub async fn join_space(&self, link: &str) -> Result<SpaceId> {
        let ticket = Ticket::decode(link)?;
        if self.ctx.space(ticket.space).is_some() {
            return Ok(ticket.space); // уже состоим — не считаем это ошибкой
        }
        // Своя связь с миром — раньше стука: без неё первый же участник из
        // ссылки «не ответит», хотя он на месте.
        let online = self.net.wait_reachable(LINK_WAIT).await;
        let admission = self.net.knock(&ticket).await.map_err(|err| {
            if online {
                err
            } else {
                anyhow!("{err}. Похоже, нет связи с интернетом у вас самих — проверьте сеть")
            }
        })?;
        let space = Space {
            id: admission.space,
            name: admission.name,
            key: admission.key,
            direct: None,
        };
        self.store.save_space(&space)?;
        self.store
            .adopt_key(space.id, &space.key, admission.epoch, admission.key_event)?;
        self.ctx.add_space(space.clone());
        self.net.join_via(space.clone(), &admission.peers).await?;
        self.announce_profile(space.id).await?;
        Ok(space.id)
    }

    /// Ссылка-приглашение на неделю без ограничения входов.
    pub async fn invite(&self, space: SpaceId) -> Result<String> {
        self.invite_with(space, InviteOptions::default()).await
    }

    /// Ссылка-приглашение. Ключа в ней нет — только секрет, хеш которого
    /// уходит в лог, и адреса тех, кто может впустить.
    pub async fn invite_with(&self, space: SpaceId, options: InviteOptions) -> Result<String> {
        let info = self
            .ctx
            .space(space)
            .ok_or_else(|| anyhow!("пространство не найдено"))?;
        if info.direct.is_some() {
            bail!("в личную переписку не приглашают");
        }
        if options.expires_in < 0 || options.expires_in > MAX_INVITE_TTL_MS {
            bail!("срок приглашения — от часа до месяца или бессрочно");
        }
        if options.uses > MAX_INVITE_USES {
            bail!("входов по одной ссылке — не больше {MAX_INVITE_USES}");
        }
        self.net.wait_reachable(LINK_WAIT).await;

        let secret: [u8; 32] = rand::random();
        let expires = match options.expires_in {
            0 => 0,
            ttl => now_ms() + ttl,
        };
        let signed = self
            .commit_and_publish(
                space,
                EventKind::InviteCreate {
                    proof: invite_proof(&secret),
                    expires,
                    uses: options.uses,
                },
            )
            .await?;

        let mut bootstrap = vec![self.net.addr_now()];
        bootstrap.extend(
            self.store
                .known_peers(space)?
                .into_iter()
                .filter(|(id, _)| *id != self.me())
                .filter_map(|(_, addr)| addr)
                .take(SPARE_DOORMEN),
        );
        bootstrap.retain(|addr| !addr.is_empty());

        Ok(Ticket {
            space,
            name: info.name,
            invite: signed.id(),
            secret,
            bootstrap,
        }
        .encode())
    }

    /// Приглашения пространства. Модератор видит все, остальные — свои.
    pub fn invites(&self, space: SpaceId) -> Result<Vec<InviteView>> {
        let me = self.me();
        let moderator = self.governance(space)?.can_moderate;
        let now = now_ms();
        Ok(self
            .store
            .invites(space)?
            .into_iter()
            .filter(|row| moderator || row.author == me)
            .map(|row| InviteView {
                id: row.id,
                author: row.author,
                author_nick: self.nick_in(space, row.author),
                created: row.created,
                expires: row.expires,
                uses: row.uses,
                used: row.used,
                revoked: row.revoked,
                live: row.live(now),
                mine: row.author == me,
            })
            .collect())
    }

    pub async fn revoke_invite(&self, space: SpaceId, invite: Id) -> Result<()> {
        let row = self
            .store
            .invite(invite)?
            .filter(|row| row.space == space)
            .ok_or_else(|| anyhow!("приглашение не найдено"))?;
        if row.author != self.me() && !self.governance(space)?.can_moderate {
            bail!("погасить чужое приглашение может только администратор");
        }
        self.commit_and_publish(space, EventKind::InviteRevoke { invite })
            .await?;
        Ok(())
    }

    /// Исключить участника и сразу сменить ключ: без смены он по-прежнему
    /// читал бы всё новое, просто молча.
    pub async fn remove_member(&self, space: SpaceId, member: Id) -> Result<()> {
        let view = self.governance(space)?;
        if view.direct {
            bail!("из личной переписки не исключают");
        }
        if !view.founded {
            bail!("{OWNERLESS}");
        }
        if !view.can_moderate {
            bail!("исключать может только администратор");
        }
        if member == self.me() {
            bail!("себя не исключают — из пространства можно выйти");
        }
        // Отметка — последнее, что мы от него видели: всё, что он подпишет
        // после, силы иметь не будет, как бы ни подкрутил свои часы.
        let cut = self.store.decision_cut(space, member)?;
        self.commit_and_publish(space, EventKind::MemberRemove { member, cut })
            .await?;
        if !self.store.is_removed(space, member)? {
            bail!("этого участника исключить нельзя: он владелец или такой же администратор");
        }
        self.rotate_key(space).await?;
        Ok(())
    }

    /// Новый ключ пространства — каждому оставшемуся свой экземпляр.
    ///
    /// Заодно гасит все ссылки старого образца, в которых лежал прежний ключ.
    /// Возвращает, скольким участникам ключ разложен.
    pub async fn rotate_key(&self, space: SpaceId) -> Result<usize> {
        let view = self.governance(space)?;
        if view.direct || !view.can_moderate {
            bail!("сменить ключ может только администратор");
        }
        if !view.founded {
            bail!("{OWNERLESS}");
        }
        let kind = self.ctx.prepare_rotation(space)?;
        let count = match &kind {
            EventKind::KeyRotate { wraps, .. } => wraps.len(),
            _ => 0,
        };
        self.commit_and_publish(space, kind).await?;
        Ok(count)
    }

    /// Назначить администратора или снять. Только владелец.
    pub async fn set_admin(&self, space: SpaceId, member: Id, admin: bool) -> Result<()> {
        if self.governance(space)?.role != Role::Owner {
            bail!("назначать администраторов может только владелец");
        }
        if member == self.me() {
            bail!("владелец и так может всё");
        }
        if self.store.is_removed(space, member)? {
            bail!("этот человек исключён");
        }
        let cut = self.store.decision_cut(space, member)?;
        self.commit_and_publish(space, EventKind::RoleSet { member, admin, cut })
            .await?;
        Ok(())
    }

    pub fn governance(&self, space: SpaceId) -> Result<GovernanceView> {
        let info = self
            .ctx
            .space(space)
            .ok_or_else(|| anyhow!("пространство не найдено"))?;
        let direct = info.direct.is_some();
        let role = self.store.role(space, self.me())?;
        Ok(GovernanceView {
            role,
            owner: self.store.owner(space)?,
            founded: self.store.founded(space)?,
            direct,
            epoch: self.store.key_epoch(space)?.0,
            can_moderate: direct || role != Role::Member,
        })
    }

    pub fn privacy(&self) -> PrivacyView {
        PrivacyView {
            hide_ip: hide_ip_enabled(&self.store),
            active: self.net.hides_ip(),
            passcode: self.passcode_enabled(),
            key_storage: if cfg!(windows) { "system" } else { "file" },
        }
    }

    /// Запомнить выбор. Сеть поднимается с ним при следующем запуске:
    /// перестроить транспорт на ходу — значит оборвать все связи разом.
    pub fn set_hide_ip(&self, hide: bool) -> Result<PrivacyView> {
        if hide {
            self.store.set_setting(HIDE_IP_SETTING, &[1])?;
        } else {
            self.store.delete_setting(HIDE_IP_SETTING)?;
        }
        Ok(self.privacy())
    }

    /// Стоит ли код-пароль на базе.
    pub fn passcode_enabled(&self) -> bool {
        matches!(self.vault.lock(), Ok(Some(crate::vault::Lock::Passcode)))
    }

    /// Поставить или сменить код. Если код уже есть, без него не сменить:
    /// иначе любой у открытого ноутбука запер бы хозяина снаружи.
    ///
    /// Ключ базы при этом меняется. Иначе код защищал бы только будущее:
    /// старая копия сейфа без кода — в резервной копии диска, например, —
    /// по-прежнему открывала бы базу.
    pub fn set_passcode(&self, current: Option<&str>, fresh: &str) -> Result<()> {
        if fresh.chars().count() < crate::vault::MIN_PASSCODE {
            bail!(
                "код-пароль — не короче {} знаков",
                crate::vault::MIN_PASSCODE
            );
        }
        let mut db_key = self.db_key.lock();
        self.check_passcode(&db_key, current)?;
        let old = *db_key;
        let new_key: [u8; 32] = rand::random();
        // Сначала сейф узнаёт оба ключа: оборвись дальше что угодно — при
        // следующем запуске база откроется тем из них, на котором осталась.
        self.vault.set_passcode(&new_key, fresh, Some(&old))?;
        if let Err(err) = self.store.rekey(&new_key) {
            // База осталась на прежнем ключе — возвращаем сейф как было.
            let restored = match current {
                Some(code) => self.vault.set_passcode(&old, code, None),
                None => self.vault.clear_passcode(&old),
            };
            if let Err(restore) = restored {
                tracing::error!(%restore, "сейф не вернулся к прежнему ключу — откроется кодом через запасной");
            }
            return Err(err);
        }
        *db_key = new_key;
        // Переход закончен: прежний ключ из сейфа больше не нужен.
        if let Err(err) = self.vault.set_passcode(&new_key, fresh, None) {
            tracing::warn!(%err, "прежний ключ остался в сейфе под кодом — уберётся при следующей смене");
        }
        Ok(())
    }

    pub fn clear_passcode(&self, current: &str) -> Result<()> {
        let db_key = self.db_key.lock();
        self.check_passcode(&db_key, Some(current))?;
        self.vault.clear_passcode(&db_key)
    }

    fn check_passcode(&self, db_key: &[u8; 32], current: Option<&str>) -> Result<()> {
        if !self.passcode_enabled() {
            return Ok(());
        }
        let current = current.ok_or_else(|| anyhow!("сначала введите нынешний код"))?;
        if self.vault.unlock(current)? != *db_key {
            bail!("неверный код-пароль");
        }
        Ok(())
    }

    fn nick_in(&self, space: SpaceId, who: Id) -> String {
        if who == self.me() {
            return self.nick();
        }
        self.ctx.nick_of(space, who)
    }
}
