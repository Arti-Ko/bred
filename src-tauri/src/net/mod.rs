//! Сетевой слой БРЕД.
//!
//! Три пути доставки, все без нашего сервера:
//!   1. локальная сеть — UDP-маячок, прямое QUIC-соединение;
//!   2. интернет — пробивка NAT средствами iroh, тоже прямое соединение;
//!   3. релей n0 — только если пробить NAT не удалось. Через него едет
//!      зашифрованный поток, содержимое релею недоступно.
//!
//! Живые события расходятся по gossip-рою, история догоняется отдельным
//! протоколом (см. [`sync`]).

pub mod blobs;
pub mod ctx;
mod lan;
pub mod media;
pub mod sync;
pub mod wire;

pub use blobs::import as import_blob;
pub use ctx::{Ctx, Notice};
pub use media::{Media, Track};

use anyhow::Result;
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint,
    EndpointAddr, EndpointId,
};
use iroh_gossip::{
    api::{Event as GossipEvent, GossipSender},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
};
use n0_future::StreamExt;
use parking_lot::RwLock;
use std::{collections::HashMap, sync::Arc, time::Duration};

use crate::domain::{now_ms, SignedEvent, Space, SpaceId};
use wire::{Broadcast, Presence};

/// Как часто напоминаем о себе соседям по пространству.
const PRESENCE_INTERVAL: Duration = Duration::from_secs(10);

pub struct Net {
    endpoint: Endpoint,
    gossip: Gossip,
    lookup: MemoryLookup,
    ctx: Arc<Ctx>,
    media: Arc<Media>,
    senders: RwLock<HashMap<SpaceId, GossipSender>>,
    addr_bytes: Arc<RwLock<Vec<u8>>>,
    _router: Router,
}

impl Net {
    /// Поднимает узел: endpoint, gossip, приём досинхронизации, локальный маячок.
    pub async fn spawn(ctx: Arc<Ctx>) -> Result<Arc<Self>> {
        let lookup = MemoryLookup::new();
        let endpoint = Endpoint::builder(presets::N0)
            .secret_key(ctx.identity.secret().clone())
            .address_lookup(lookup.clone())
            .bind()
            .await?;

        let gossip = Gossip::builder().spawn(endpoint.clone());
        let media = Media::new(ctx.clone(), endpoint.clone());
        let router = Router::builder(endpoint.clone())
            .accept(GOSSIP_ALPN, gossip.clone())
            .accept(sync::SYNC_ALPN, sync::SyncProtocol::new(ctx.clone()))
            .accept(blobs::BLOB_ALPN, blobs::BlobProtocol::new(ctx.clone()))
            .accept(media::MEDIA_ALPN, media::MediaProtocol::new(media.clone()))
            .spawn();

        let net = Arc::new(Self {
            endpoint,
            gossip,
            lookup,
            ctx,
            media,
            senders: RwLock::new(HashMap::new()),
            addr_bytes: Arc::new(RwLock::new(Vec::new())),
            _router: router,
        });

        net.clone().sweep_presence();
        net.clone().watch_own_addr();
        net.clone().serve_lan();

        for space in net.ctx.space_list() {
            if let Err(err) = net.join(space.clone()).await {
                tracing::warn!(space = %space.id.short(), %err, "не удалось подключиться к пространству");
            }
        }

        Ok(net)
    }

    pub fn media(&self) -> &Arc<Media> {
        &self.media
    }

    /// Немедленно сообщить о себе. Вызывается при входе и выходе из звонка:
    /// ждать очередного удара сердца — это до десяти секунд, за которые
    /// собеседники не увидят, что мы подключились.
    pub async fn announce_presence(&self, space: SpaceId) -> Result<()> {
        self.publish(space, &Broadcast::Presence(self.presence_for(space)))
            .await
    }

    fn presence_for(&self, space: SpaceId) -> Presence {
        let nick = crate::identity::load_nick(&self.ctx.store, self.ctx.identity.id())
            .unwrap_or_else(|_| self.ctx.identity.id().short());
        Presence {
            author: self.ctx.identity.id(),
            // Адрес едет вместе с присутствием: без него до человека нельзя
            // дозвониться напрямую за файлом или за видео.
            addr: self.addr_now(),
            nick,
            // Голосовой канал показываем только соседям по тому же пространству.
            voice: self
                .media
                .active()
                .filter(|(active, _)| *active == space)
                .map(|(_, channel)| channel),
            ts: now_ms(),
        }
    }

    pub fn endpoint_id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// Последний известный адрес — им пользуются маячок и индикатор сети.
    pub fn addr_bytes(&self) -> Vec<u8> {
        self.addr_bytes.read().clone()
    }

    /// Адрес прямо сейчас, для ссылки-приглашения.
    ///
    /// Кеш обновляется раз в три секунды, и в первые секунды после запуска он
    /// пуст — ссылка, сделанная в этот момент, оказалась бы без точки входа,
    /// и друг не смог бы подключиться через интернет.
    pub fn addr_now(&self) -> Vec<u8> {
        postcard::to_stdvec(&self.endpoint.addr()).unwrap_or_else(|_| self.addr_bytes())
    }

    /// Подписка на пространство без точек входа: так входят в уже известные
    /// пространства при старте — соседи найдутся сами через локалку или рой.
    pub async fn join(self: &Arc<Self>, space: Space) -> Result<()> {
        self.join_via(space, &[]).await
    }

    /// Вход по приглашению: `bootstrap` — сериализованные адреса пригласившего.
    ///
    /// Без них новичок в интернете никого бы не нашёл: рой gossip умеет
    /// разрастаться, но кто-то должен быть первым, к кому подключиться.
    /// В локалке эту роль берёт на себя UDP-маячок, а через интернет — вот эти
    /// адреса из ссылки.
    pub async fn join_via(self: &Arc<Self>, space: Space, bootstrap: &[Vec<u8>]) -> Result<()> {
        let mut entry_points = Vec::new();
        for raw in bootstrap {
            let Ok(addr) = postcard::from_bytes::<EndpointAddr>(raw) else {
                continue;
            };
            if addr.id == self.endpoint.id() {
                continue;
            }
            // Кладём адрес в справочник, иначе gossip не сможет дозвониться:
            // идентификатор он знает, а как до него добраться — нет.
            self.lookup.add_endpoint_info(addr.clone());
            entry_points.push(addr.id);
        }

        let topic = TopicId::from_bytes(space.topic());
        let subscription = self.gossip.subscribe(topic, entry_points.clone()).await?;
        let (sender, mut receiver) = subscription.split();

        self.senders.write().insert(space.id, sender);

        let me = self.clone();
        let space_id = space.id;
        let key = space.key;
        tokio::spawn(async move {
            while let Some(event) = receiver.next().await {
                match event {
                    Ok(GossipEvent::Received(message)) => {
                        me.on_message(space_id, &key, &message.content);
                    }
                    Ok(GossipEvent::NeighborUp(peer)) => {
                        tracing::debug!(peer = %peer.fmt_short(), "новый сосед");
                        me.clone().catch_up(space_id, peer);
                        let _ = me.ctx.notices.send(Notice::Net);
                    }
                    Ok(GossipEvent::NeighborDown(peer)) => {
                        me.ctx
                            .drop_presence(space_id, crate::domain::Id(*peer.as_bytes()));
                        let _ = me.ctx.notices.send(Notice::Net);
                    }
                    Ok(GossipEvent::Lagged) => {
                        // Не успели вычитать поток — историю добираем досинхроном.
                        tracing::debug!(space = %space_id.short(), "отстали от роя");
                    }
                    Err(err) => {
                        tracing::debug!(%err, "поток gossip прерван");
                        break;
                    }
                }
            }
        });

        // Не ждём, пока рой сам заметит соседа: историю тянем сразу.
        for peer in entry_points {
            self.clone().catch_up(space.id, peer);
        }

        self.clone().heartbeat(space);
        Ok(())
    }

    /// Выйти из пространства: перестать слушать рой и забыть ключ.
    pub fn leave(&self, space: SpaceId) {
        self.senders.write().remove(&space);
        self.ctx.spaces.write().remove(&space);
        self.ctx.presence.write().remove(&space);
        // Если мы были в звонке этого пространства — звонка больше нет.
        if self.media.active().map(|(s, _)| s) == Some(space) {
            self.media.leave();
        }
    }

    /// Разослать событие лога соседям по пространству.
    pub async fn publish_event(&self, space: SpaceId, signed: &SignedEvent) -> Result<()> {
        self.publish(space, &Broadcast::Event(Box::new(signed.clone())))
            .await
    }

    pub async fn publish_typing(
        &self,
        space: SpaceId,
        channel: crate::domain::ChannelId,
    ) -> Result<()> {
        self.publish(
            space,
            &Broadcast::Typing {
                channel,
                author: self.ctx.identity.id(),
            },
        )
        .await
    }

    async fn publish(&self, space: SpaceId, message: &Broadcast) -> Result<()> {
        let Some(key) = self.ctx.space(space).map(|s| s.key) else {
            return Ok(());
        };
        let Some(sender) = self.senders.read().get(&space).cloned() else {
            return Ok(());
        };
        let sealed = wire::wrap(&key, message)?;
        sender.broadcast(sealed.into()).await?;
        Ok(())
    }

    /// Разбор входящего сообщения роя.
    fn on_message(&self, space: SpaceId, key: &[u8; 32], raw: &[u8]) {
        let message = match wire::unwrap(key, raw) {
            Ok(message) => message,
            Err(None) => return, // чужой ключ или мусор — в открытом рое это норма
            Err(Some(their)) => {
                // Версии разошлись. Молчать нельзя: со стороны это выглядит как
                // «человек онлайн, но его сообщения не приходят».
                let _ = self.ctx.notices.send(Notice::Version {
                    space,
                    theirs: their,
                    ours: wire::PROTOCOL,
                });
                return;
            }
        };
        match message {
            Broadcast::Event(signed) => {
                if let Err(err) = self.ctx.apply(&signed) {
                    tracing::warn!(%err, "не удалось применить событие");
                }
            }
            Broadcast::Presence(presence) => {
                // Запоминаем адрес — именно он позволяет дозвониться до человека,
                // не полагаясь на внешние службы имён.
                if !presence.addr.is_empty() {
                    if let Ok(addr) = postcard::from_bytes::<EndpointAddr>(&presence.addr) {
                        if addr.id != self.endpoint.id() {
                            self.lookup.add_endpoint_info(addr);
                        }
                    }
                }
                self.ctx.note_presence(space, presence);
            }
            Broadcast::Typing { channel, author } => {
                let nick = self.ctx.nick_of(space, author);
                let _ = self.ctx.notices.send(Notice::Typing {
                    space,
                    channel,
                    author,
                    nick,
                });
            }
        }
    }

    /// Скачать вложение. Перебираем тех, кто его упоминал, пока кто-то не отдаст:
    /// автор может быть офлайн, но файл уже есть у любого, кто его получил.
    pub async fn fetch_blob(
        &self,
        space: SpaceId,
        hash: crate::domain::Id,
        size: u64,
        holders: &[crate::domain::Id],
    ) -> Result<std::path::PathBuf> {
        let mut last: Option<anyhow::Error> = None;
        for holder in holders {
            if *holder == self.ctx.identity.id() {
                continue;
            }
            let Ok(peer) = iroh::PublicKey::from_bytes(&holder.0) else {
                continue;
            };
            match blobs::fetch(
                self.ctx.clone(),
                &self.endpoint,
                EndpointAddr::from(peer),
                space,
                hash,
                size,
            )
            .await
            {
                Ok(path) => return Ok(path),
                Err(err) => {
                    tracing::debug!(holder = %holder.short(), %err, "не отдал файл, пробуем следующего");
                    last = Some(err);
                }
            }
        }
        Err(last
            .unwrap_or_else(|| anyhow::anyhow!("некого спросить: нет ни одного держателя файла")))
    }

    /// Догнать историю у только что появившегося соседа.
    fn catch_up(self: Arc<Self>, space: SpaceId, peer: EndpointId) {
        tokio::spawn(async move {
            let addr = EndpointAddr::from(peer);
            match sync::sync_with(self.ctx.clone(), &self.endpoint, addr, space).await {
                Ok(0) => tracing::debug!(space = %space.short(), "история уже совпадала"),
                Ok(n) => tracing::info!(space = %space.short(), events = n, "догнали историю"),
                Err(err) => tracing::debug!(%err, "досинхронизация не удалась"),
            }
        });
    }

    /// Периодически сообщаем соседям, что мы здесь.
    fn heartbeat(self: Arc<Self>, space: Space) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(PRESENCE_INTERVAL);
            loop {
                ticker.tick().await;
                if !self.senders.read().contains_key(&space.id) {
                    break;
                }
                let presence = self.presence_for(space.id);
                let _ = self.publish(space.id, &Broadcast::Presence(presence)).await;
            }
        });
    }

    /// Периодически выбрасываем присутствие тех, кто пропал без прощания.
    fn sweep_presence(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(10));
            loop {
                ticker.tick().await;
                for space in self.ctx.sweep_presence() {
                    let _ = self.ctx.notices.send(Notice::Presence { space });
                }
            }
        });
    }

    /// Держим сериализованный собственный адрес актуальным — он нужен и
    /// маячку, и ссылке-приглашению.
    fn watch_own_addr(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(3));
            loop {
                ticker.tick().await;
                if let Ok(raw) = postcard::to_stdvec(&self.endpoint.addr()) {
                    let changed = *self.addr_bytes.read() != raw;
                    if changed {
                        *self.addr_bytes.write() = raw;
                        let _ = self.ctx.notices.send(Notice::Net);
                    }
                }
            }
        });
    }

    /// Локальная сеть: слушаем маячки и подключаемся к найденным.
    fn serve_lan(self: Arc<Self>) {
        let mut found = lan::spawn(self.ctx.clone(), self.addr_bytes.clone());
        tokio::spawn(async move {
            while let Some((addr_bytes, spaces)) = found.recv().await {
                let Ok(addr) = postcard::from_bytes::<EndpointAddr>(&addr_bytes) else {
                    continue;
                };
                if addr.id == self.endpoint.id() {
                    continue; // собственный маячок вернулся по петле
                }
                // Кладём адрес в справочник, чтобы gossip мог до него дозвониться.
                self.lookup.add_endpoint_info(addr.clone());

                for space in spaces {
                    // Клонируем отправителя и сразу отпускаем блокировку:
                    // держать её через .await нельзя — future перестаёт быть Send.
                    let sender = self.senders.read().get(&space).cloned();
                    if let Some(sender) = sender {
                        let _ = sender.join_peers(vec![addr.id]).await;
                    }
                    self.clone().catch_up(space, addr.id);
                }
            }
        });
    }
}
