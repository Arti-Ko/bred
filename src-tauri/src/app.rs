//! Прикладной слой: собирает хранилище, личность и сеть в одно целое
//! и предоставляет операции, которыми пользуется UI.

use anyhow::{anyhow, Result};
use std::{path::Path, sync::Arc};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::{
    domain::{
        now_ms, Attachment, Clock, Event, EventKind, Hello, Id, Invite, SignedEvent, Space, SpaceId,
    },
    identity::{load_avatar, load_nick, save_avatar, save_nick, Identity},
    net::{ctx::Notice, Ctx, Net},
    store::{ChannelRow, MemberRow, MessageRow, SpaceRow, Store},
};

/// Сколько сообщений отдаём в ленту за раз.
pub const PAGE: usize = 200;
/// Потолок на файл, который можно показать прямо в ленте: всё, что больше,
/// не картинка для превью, а вложение, и гнать его через IPC незачем.
const PREVIEW_LIMIT: u64 = 16 * 1024 * 1024;

pub struct App {
    pub store: Arc<Store>,
    /// Ключ согласования для личных переписок.
    dh: x25519_dalek::StaticSecret,
    pub ctx: Arc<Ctx>,
    pub net: Arc<Net>,
}

impl App {
    /// Полная инициализация. Возвращает приложение и поток уведомлений для UI.
    pub async fn start(db_path: &Path) -> Result<(Arc<Self>, UnboundedReceiver<Notice>)> {
        let store = Arc::new(Store::open(db_path)?);
        let identity = Identity::load_or_create(&store)?;
        store.set_me(identity.id());

        let spaces = store.spaces()?;
        let clock = Clock::new(store.max_lamport()?);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

        // Вложения кладём рядом с базой: один каталог данных на всё приложение.
        let blob_dir = db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("blobs");
        let dh = crate::identity::load_or_create_dh(&store)?;
        let ctx = Arc::new(Ctx::new(
            store.clone(),
            identity,
            spaces,
            clock,
            tx,
            &blob_dir,
        ));
        let net = Net::spawn(ctx.clone()).await?;
        tracing::info!(
            me = %ctx.identity.id().short(),
            endpoint = %net.endpoint_id(),
            spaces = ctx.space_list().len(),
            "БРЕД запущен"
        );

        Ok((
            Arc::new(Self {
                store,
                dh,
                ctx,
                net,
            }),
            rx,
        ))
    }

    /// Публичная половина ключа согласования — её видят собеседники.
    pub fn dh_public(&self) -> Id {
        crate::identity::dh_public(&self.dh)
    }

    /// Личная визитка: ссылка, по которой с тобой можно связаться напрямую,
    /// не имея ни одного общего пространства.
    pub fn personal_link(&self) -> String {
        Hello {
            id: self.me(),
            nick: self.nick(),
            dh: self.dh_public(),
            addr: self.net.addr_now(),
        }
        .encode()
    }

    /// Открыть личную переписку по чужой визитке.
    pub async fn open_direct_link(&self, link: &str) -> Result<SpaceId> {
        let hello = Hello::decode(link)?;
        if hello.id == self.me() {
            return Err(anyhow!("это ваша собственная ссылка"));
        }

        let shared = crate::identity::shared_secret(&self.dh, hello.dh);
        let mut space = crate::domain::direct_space(self.me(), hello.id, shared);
        space.name = hello.nick.clone();

        if self.ctx.space(space.id).is_none() {
            self.store.save_space(&space)?;
            self.ctx.add_space(space.clone());
        }
        // Запоминаем собеседника, иначе после перезапуска мы не сможем
        // ни назвать его, ни завести переписку заново.
        self.store
            .remember_peer(space.id, hello.id, &hello.nick, hello.dh)?;

        let bootstrap = if hello.addr.is_empty() {
            Vec::new()
        } else {
            vec![hello.addr]
        };
        self.net.join_via(space.clone(), &bootstrap).await?;

        if self.store.channels(space.id)?.is_empty() {
            self.create_channel(space.id, "личное", "личное", false)
                .await?;
        }
        self.announce_profile(space.id).await?;
        Ok(space.id)
    }

    /// Завести или открыть личную переписку. Приглашение не нужно: адрес и ключ
    /// обе стороны выводят из своих ключей и ключа собеседника.
    pub async fn open_direct(&self, peer: Id) -> Result<SpaceId> {
        if peer == self.me() {
            return Err(anyhow!("нельзя писать самому себе"));
        }
        let peer_dh = self
            .store
            .peer_dh(peer)?
            .ok_or_else(|| anyhow!("у собеседника ещё нет ключа для личной переписки"))?;

        let shared = crate::identity::shared_secret(&self.dh, peer_dh);
        let mut space = crate::domain::direct_space(self.me(), peer, shared);
        space.name = self.store.peer_nick(peer)?.unwrap_or_else(|| peer.short());

        if self.ctx.space(space.id).is_some() {
            return Ok(space.id);
        }
        self.store.save_space(&space)?;
        self.ctx.add_space(space.clone());
        self.net.join(space.clone()).await?;

        // Канал в личной переписке ровно один, и завести его должны обе стороны
        // одинаково — иначе у каждого будет свой. Имя фиксировано, а слияние
        // двух одинаковых каналов делает обычная сходимость лога.
        if self.store.channels(space.id)?.is_empty() {
            self.create_channel(space.id, "личное", "личное", false)
                .await?;
        }
        self.announce_profile(space.id).await?;
        Ok(space.id)
    }

    // ── свои эмодзи и стикеры ───────────────────────────────────────────────

    /// Добавить свой эмодзи или стикер: картинка уезжает как вложение,
    /// имя становится доступным как `:имя:`.
    pub async fn add_emoji(
        &self,
        space: SpaceId,
        name: &str,
        path: &Path,
        sticker: bool,
    ) -> Result<()> {
        let name = name.trim().trim_matches(':').to_string();
        if name.is_empty() || name.chars().count() > 32 {
            return Err(anyhow!("имя эмодзи должно быть от 1 до 32 символов"));
        }
        if !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        {
            return Err(anyhow!(
                "в имени можно только буквы, цифры, дефис и подчёркивание"
            ));
        }

        let attachment = self.attach(path).await?;
        if !attachment.mime.starts_with("image/") {
            return Err(anyhow!("эмодзи может быть только картинкой"));
        }
        self.commit_and_publish(
            space,
            EventKind::EmojiAdd {
                name,
                hash: attachment.hash,
                sticker,
            },
        )
        .await?;
        Ok(())
    }

    pub async fn remove_emoji(&self, space: SpaceId, name: &str) -> Result<()> {
        self.commit_and_publish(
            space,
            EventKind::EmojiRemove {
                name: name.trim().trim_matches(':').to_string(),
            },
        )
        .await?;
        Ok(())
    }

    pub fn emojis(&self, space: SpaceId) -> Result<Vec<crate::store::EmojiRow>> {
        self.store.emojis(space)
    }

    pub fn me(&self) -> Id {
        self.ctx.identity.id()
    }

    pub fn nick(&self) -> String {
        load_nick(&self.store, self.me()).unwrap_or_else(|_| self.me().short())
    }

    // ── пространства ────────────────────────────────────────────────────────

    /// Создаёт новое пространство. Ключ генерируется здесь и дальше живёт
    /// только у тех, кому дали ссылку-приглашение.
    pub async fn create_space(&self, name: &str) -> Result<SpaceId> {
        let name = validate_name(name, "название пространства")?;

        let mut key = [0u8; 32];
        rand::Rng::fill(&mut rand::rng(), &mut key[..]);
        let mut id_bytes = [0u8; 32];
        rand::Rng::fill(&mut rand::rng(), &mut id_bytes[..]);

        let space = Space {
            id: Id(id_bytes),
            name: name.clone(),
            key,
            direct: None,
        };
        self.store.save_space(&space)?;
        self.ctx.add_space(space.clone());
        self.net.join(space.clone()).await?;

        self.commit_and_publish(space.id, EventKind::SpaceCreate { name })
            .await?;
        // Пространство без каналов бесполезно — сразу заводим общий.
        self.create_channel(space.id, "общий-канал", "общее", false)
            .await?;
        self.announce_profile(space.id).await?;
        Ok(space.id)
    }

    /// Присоединение по ссылке-приглашению.
    pub async fn join_space(&self, ticket: &str) -> Result<SpaceId> {
        let invite = Invite::decode(ticket)?;
        if self.ctx.space(invite.space).is_some() {
            return Ok(invite.space); // уже состоим — не считаем это ошибкой
        }

        let space = Space {
            id: invite.space,
            name: invite.name,
            key: invite.key,
            direct: None,
        };
        self.store.save_space(&space)?;
        self.ctx.add_space(space.clone());
        // Адреса из ссылки — единственная зацепка при входе через интернет.
        self.net.join_via(space.clone(), &invite.bootstrap).await?;
        self.announce_profile(space.id).await?;
        Ok(space.id)
    }

    /// Ссылка-приглашение: несёт ключ пространства и наш адрес как точку входа.
    pub fn invite(&self, space: SpaceId) -> Result<String> {
        let space = self
            .ctx
            .space(space)
            .ok_or_else(|| anyhow!("пространство не найдено"))?;
        let addr = self.net.addr_now();
        let bootstrap = if addr.is_empty() {
            Vec::new()
        } else {
            vec![addr]
        };

        Ok(Invite {
            space: space.id,
            name: space.name,
            key: space.key,
            bootstrap,
        }
        .encode())
    }

    /// Выйти из пространства и стереть его историю с этого устройства.
    pub async fn leave_space(&self, space: SpaceId) -> Result<()> {
        if self.ctx.space(space).is_none() {
            return Err(anyhow!("пространство не найдено"));
        }
        self.net.leave(space);
        self.store.forget_space(space)?;
        Ok(())
    }

    pub fn spaces(&self) -> Result<Vec<SpaceRow>> {
        self.store.space_rows()
    }

    // ── каналы и сообщения ──────────────────────────────────────────────────

    pub async fn create_channel(
        &self,
        space: SpaceId,
        name: &str,
        category: &str,
        voice: bool,
    ) -> Result<Id> {
        let name = validate_name(name, "название канала")?;
        let signed = self
            .commit_and_publish(
                space,
                EventKind::ChannelCreate {
                    name,
                    category: category.trim().to_string(),
                    voice,
                },
            )
            .await?;
        Ok(signed.id())
    }

    pub fn channels(&self, space: SpaceId) -> Result<Vec<ChannelRow>> {
        self.store.channels(space)
    }

    pub fn messages(&self, channel: Id, before: Option<u64>) -> Result<Vec<MessageRow>> {
        self.store.messages(channel, PAGE, before)
    }

    pub fn thread(&self, root: Id) -> Result<Vec<MessageRow>> {
        self.store.thread(root)
    }

    /// Байты вложения для показа прямо в ленте. Только то, что уже скачано.
    pub async fn attachment_bytes(&self, hash: Id) -> Result<Vec<u8>> {
        let path = self
            .store
            .blob_path(hash)?
            .filter(|p| p.exists())
            .ok_or_else(|| anyhow!("файл ещё не скачан"))?;
        let meta = tokio::fs::metadata(&path).await?;
        if meta.len() > PREVIEW_LIMIT {
            return Err(anyhow!("файл слишком большой для показа"));
        }
        Ok(tokio::fs::read(&path).await?)
    }

    /// Убрать с диска файлы, на которые больше нет ни одной ссылки.
    /// Возвращает, сколько байт освободилось.
    pub async fn collect_garbage(&self) -> Result<u64> {
        let mut freed = 0;
        for (hash, path) in self.store.orphan_blobs()? {
            let size = tokio::fs::metadata(&path)
                .await
                .map(|m| m.len())
                .unwrap_or(0);
            if tokio::fs::remove_file(&path).await.is_ok() || !path.exists() {
                self.store.forget_blob(hash)?;
                freed += size;
            }
        }
        if freed > 0 {
            tracing::info!(freed, "убрали неиспользуемые вложения");
        }
        Ok(freed)
    }

    pub fn message(&self, id: Id) -> Result<Option<MessageRow>> {
        self.store.message(id)
    }

    pub async fn send_message(
        &self,
        space: SpaceId,
        channel: Id,
        body: &str,
        reply_to: Option<Id>,
        thread: Option<Id>,
        attachments: Vec<Attachment>,
    ) -> Result<Id> {
        let body = body.trim();
        if body.is_empty() && attachments.is_empty() {
            return Err(anyhow!("пустое сообщение"));
        }
        if body.chars().count() > 2000 {
            return Err(anyhow!("сообщение длиннее 2000 символов"));
        }

        let signed = self
            .commit_and_publish(
                space,
                EventKind::Message {
                    channel,
                    body: body.to_string(),
                    reply_to,
                    thread,
                    attachments,
                },
            )
            .await?;
        Ok(signed.id())
    }

    pub async fn react(&self, space: SpaceId, target: Id, emoji: &str, remove: bool) -> Result<()> {
        let emoji = emoji.trim();
        if emoji.is_empty() || emoji.chars().count() > 8 {
            return Err(anyhow!("некорректная реакция"));
        }
        self.commit_and_publish(
            space,
            EventKind::Reaction {
                target,
                emoji: emoji.to_string(),
                remove,
            },
        )
        .await?;
        Ok(())
    }

    pub async fn edit_message(&self, space: SpaceId, target: Id, body: &str) -> Result<()> {
        let body = body.trim();
        if body.is_empty() {
            return Err(anyhow!("пустое сообщение"));
        }
        self.commit_and_publish(
            space,
            EventKind::Edit {
                target,
                body: body.to_string(),
            },
        )
        .await?;
        Ok(())
    }

    pub async fn delete_message(&self, space: SpaceId, target: Id) -> Result<()> {
        self.commit_and_publish(space, EventKind::Delete { target })
            .await?;
        Ok(())
    }

    // ── звонки ──────────────────────────────────────────────────────────────

    /// Войти в звонок. Комната — это голосовой канал: достаточно объявить
    /// присутствие в нём, остальное сделает сетка соединений.
    pub async fn join_call(&self, space: SpaceId, channel: Id) -> Result<()> {
        if self.ctx.space(space).is_none() {
            return Err(anyhow!("пространство не найдено"));
        }
        self.net.media().join(space, channel);
        self.net.announce_presence(space).await
    }

    pub async fn leave_call(&self) -> Result<()> {
        let active = self.net.media().active();
        self.net.media().leave();
        if let Some((space, _)) = active {
            self.net.announce_presence(space).await?;
        }
        Ok(())
    }

    pub fn call_state(&self) -> Option<(SpaceId, Id)> {
        self.net.media().active()
    }

    pub fn call_participants(&self) -> Vec<Id> {
        self.net.media().participants()
    }

    /// Кадр из интерфейса — в сеть.
    pub async fn send_media(&self, raw: &[u8]) -> Result<()> {
        let (track, keyframe, ts, data) = crate::net::media::decode_from_ui(raw)?;
        self.net.media().broadcast(track, keyframe, ts, data).await
    }

    /// Кто из участников в каком голосовом канале — для списка справа.
    pub fn voice_map(&self, space: SpaceId) -> Vec<(Id, Id)> {
        self.ctx
            .presence_of(space)
            .into_iter()
            .filter_map(|p| p.voice.map(|channel| (p.author, channel)))
            .collect()
    }

    // ── вложения ────────────────────────────────────────────────────────────

    /// Подготовить локальный файл к отправке: посчитать хеш и положить в
    /// хранилище вложений. Сами байты уедут получателю по требованию.
    pub async fn attach(&self, path: &Path) -> Result<Attachment> {
        crate::net::import_blob(&self.ctx, path).await
    }

    /// Скачать вложение у того, у кого оно есть.
    pub async fn download(&self, space: SpaceId, hash: Id) -> Result<String> {
        if let Some(path) = self.store.blob_path(hash)? {
            if path.exists() {
                return Ok(path.to_string_lossy().to_string());
            }
        }
        let meta = self
            .store
            .attachment(hash)?
            .ok_or_else(|| anyhow!("вложение неизвестно"))?;
        let holders = self.store.attachment_holders(hash)?;

        let path = self
            .net
            .fetch_blob(space, hash, meta.size, &holders)
            .await?;
        let _ = self
            .ctx
            .notices
            .send(Notice::Applied { space, event: hash });
        Ok(path.to_string_lossy().to_string())
    }

    pub fn mark_read(&self, channel: Id) -> Result<()> {
        self.store.mark_read(channel)
    }

    pub async fn typing(&self, space: SpaceId, channel: Id) -> Result<()> {
        self.net.publish_typing(space, channel).await
    }

    // ── участники ───────────────────────────────────────────────────────────

    /// Список участников: то, что видели в истории, плюс отметка «в сети»
    /// из эфемерного присутствия.
    pub fn members(&self, space: SpaceId) -> Result<Vec<MemberRow>> {
        let online = self.ctx.presence_of(space);
        let mut rows = self.store.members(space)?;

        for row in rows.iter_mut() {
            if let Some(p) = online.iter().find(|p| p.author == row.id) {
                row.online = true;
                if !p.nick.is_empty() {
                    row.nick = p.nick.clone();
                }
            }
            if row.nick.is_empty() {
                row.nick = row.id.short();
            }
        }
        // Свой профиль в списке участников должен быть всегда.
        if !rows.iter().any(|r| r.id == self.me()) {
            rows.push(MemberRow {
                id: self.me(),
                nick: self.nick(),
                avatar: load_avatar(&self.store).ok().flatten(),
                dh: Some(self.dh_public()),
                online: true,
                last_seen: now_ms(),
            });
        }
        rows.sort_by(|a, b| b.online.cmp(&a.online).then(a.nick.cmp(&b.nick)));
        Ok(rows)
    }

    pub async fn set_nick(&self, nick: &str) -> Result<()> {
        let nick = validate_name(nick, "имя")?;
        save_nick(&self.store, &nick)?;
        for space in self.ctx.space_list() {
            self.commit_and_publish(
                space.id,
                EventKind::Profile {
                    nick: nick.clone(),
                    avatar: load_avatar(&self.store)?,
                    dh: Some(self.dh_public()),
                },
            )
            .await?;
        }
        Ok(())
    }

    /// Поставить картинку профиля. Файл кладём в хранилище вложений и
    /// рассылаем новый профиль во все пространства, где мы состоим.
    pub async fn set_avatar(&self, path: &Path) -> Result<Attachment> {
        let attachment = self.attach(path).await?;
        if !attachment.mime.starts_with("image/") {
            return Err(anyhow!("аватаром может быть только картинка"));
        }
        save_avatar(&self.store, attachment.hash)?;

        let nick = self.nick();
        for space in self.ctx.space_list() {
            self.commit_and_publish(
                space.id,
                EventKind::Profile {
                    nick: nick.clone(),
                    avatar: Some(attachment.hash),
                    dh: Some(self.dh_public()),
                },
            )
            .await?;
        }
        Ok(attachment)
    }

    async fn announce_profile(&self, space: SpaceId) -> Result<()> {
        let nick = self.nick();
        self.commit_and_publish(
            space,
            EventKind::Profile {
                nick,
                avatar: load_avatar(&self.store)?,
                dh: Some(self.dh_public()),
            },
        )
        .await?;
        Ok(())
    }

    // ── запись в лог ────────────────────────────────────────────────────────

    /// Создаёт собственное событие: подписывает, кладёт в лог, рассылает.
    ///
    /// Порядок важен: сначала локальная запись, потом сеть. Отправленное
    /// сообщение обязано появиться в своей ленте мгновенно, даже если сейчас
    /// нет ни одного соседа — соседи получат его при первой же встрече.
    async fn commit_and_publish(&self, space: SpaceId, kind: EventKind) -> Result<SignedEvent> {
        let author = self.me();
        let seq = self.store.next_seq(space, author)?;
        let lamport = self.ctx.clock.lock().tick();

        let event = Event {
            space,
            author,
            seq,
            lamport,
            ts: now_ms(),
            kind,
        };
        let sig = self.ctx.identity.sign(&event.canonical_bytes());
        let signed = SignedEvent { event, sig };

        self.store.apply(&signed)?;
        let _ = self.ctx.notices.send(Notice::Applied {
            space,
            event: signed.id(),
        });

        if let Err(err) = self.net.publish_event(space, &signed).await {
            // Не откатываем: событие уже в логе и уедет при досинхронизации.
            tracing::warn!(%err, "событие не ушло в сеть, разойдётся позже");
        }
        Ok(signed)
    }
}

fn validate_name(value: &str, what: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("{what} не может быть пустым"));
    }
    if trimmed.chars().count() > 64 {
        return Err(anyhow!("{what} длиннее 64 символов"));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation_trims_and_rejects_empty() {
        assert_eq!(validate_name("  общий  ", "имя").unwrap(), "общий");
        assert!(validate_name("   ", "имя").is_err());
        assert!(validate_name(&"я".repeat(65), "имя").is_err());
    }
}
