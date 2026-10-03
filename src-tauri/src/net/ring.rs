//! Зов в комнату — прямым соединением к каждому позванному.
//!
//! Первый вариант ходил рассылкой по рою, и у него было три беды. Подписи под
//! рассылкой нет — ключ пространства общий, — так что позвать или дать отбой
//! «от имени» любого мог кто угодно внутри. Рассылка не гарантирует порядок, и
//! поздний повтор зова мог зазвонить уже после отбоя. А старые версии, не зная
//! нового вида сообщения, показывали всем «у собеседника другая версия».
//!
//! Прямое соединение снимает все три. Кто звонит, подтверждает сам QUIC: автор
//! зова — это `remote_id()` соединения, подделать его нельзя. Доставка
//! надёжная и с ответом, поэтому повторы не нужны, а звонящий знает, до кого
//! зов на самом деле дошёл. Старая версия просто не примет незнакомый ALPN —
//! молча, без ложных тревог. И зов получают только позванные, а не все в рое.

use anyhow::{anyhow, Result};
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr, EndpointId,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use super::{
    ctx::{Ctx, Notice},
    media::Media,
    wire::{read_frame, seal, write_frame},
};
use crate::domain::{ChannelId, Id, SpaceId};

pub const RING_ALPN: &[u8] = b"bred/ring/1";

/// Сколько ждём, пока позванный возьмёт трубку соединения и ответит.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(6);
/// Сколько помним зов и отбой: повтор того же зова второй раз не звонит, а
/// зов, уже отменённый, не зазвонит, даже если доехал после отбоя.
const RING_MEMORY: Duration = Duration::from_secs(60);
/// Не чаще одного нового звонка от одного человека за это время: зов — не
/// способ трезвонить соседу без остановки.
const RING_COOLDOWN: Duration = Duration::from_secs(5);
/// Кадр зова — десятки байт. Всё, что больше, — мусор или попытка съесть память.
const MAX_RING_FRAME: usize = 4096;

/// Что едет по соединению. Шифруется ключом пространства: без него зов не
/// прочитать, и заодно по нему же выясняется, о каком пространстве речь.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RingFrame {
    /// Пространство повторено внутри шифра: ключ подбирается перебором, и
    /// совпасть должно и то, и другое.
    pub space: SpaceId,
    /// Номер зова — им отбой находит свой звонок.
    pub id: u64,
    pub channel: ChannelId,
    /// Отбой: звонящий передумал или сам вышел из комнаты.
    pub cancel: bool,
}

/// Приём зовов.
#[derive(Clone)]
pub struct RingProtocol {
    ctx: Arc<Ctx>,
    media: Arc<Media>,
    /// Какие зовы уже были — и показанные, и отменённые. Отмена тоже
    /// запоминается: зов, доехавший после своего отбоя, звонить не должен.
    seen: Arc<Mutex<HashMap<(Id, u64), Instant>>>,
    /// Когда от человека последний раз пришёл новый звонок.
    last_from: Arc<Mutex<HashMap<Id, Instant>>>,
}

impl RingProtocol {
    pub fn new(ctx: Arc<Ctx>, media: Arc<Media>) -> Self {
        Self {
            ctx,
            media,
            seen: Arc::new(Mutex::new(HashMap::new())),
            last_from: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Разобрать зов. `from` — из соединения, его подтвердил QUIC.
    fn on_frame(&self, space: SpaceId, from: Id, frame: RingFrame) {
        if from == self.ctx.identity.id() {
            return;
        }
        let key = (from, frame.id);

        if frame.cancel {
            // Отбой оставляет надгробие: зов, доехавший после него, уже не
            // зазвонит. И гасит звонок, если тот звонит.
            self.seen.lock().insert(key, Instant::now());
            let _ = self.ctx.notices.send(Notice::RingCancel {
                space,
                from,
                id: frame.id,
            });
            return;
        }

        // Звать можно только туда, где сидишь сам: иначе это не приглашение,
        // а спам. Присутствие и так у нас, проверка бесплатная.
        let inside = self
            .ctx
            .presence_of(space)
            .into_iter()
            .any(|p| p.author == from && p.voice == Some(frame.channel));
        if !inside {
            tracing::debug!("зов из-за пределов комнаты — отброшен");
            return;
        }
        if self.media.active() == Some((space, frame.channel)) {
            return; // уже здесь
        }

        {
            let mut seen = self.seen.lock();
            seen.retain(|_, at| at.elapsed() < RING_MEMORY);
            if seen.contains_key(&key) {
                return; // повтор или уже отменённый
            }
            let mut last = self.last_from.lock();
            last.retain(|_, at| at.elapsed() < RING_MEMORY);
            if last
                .get(&from)
                .is_some_and(|at| at.elapsed() < RING_COOLDOWN)
            {
                tracing::debug!("слишком частые зовы — отброшен");
                return;
            }
            last.insert(from, Instant::now());
            seen.insert(key, Instant::now());
        }

        let nick = self.ctx.nick_of(space, from);
        let _ = self.ctx.notices.send(Notice::Ring {
            space,
            channel: frame.channel,
            from,
            nick,
            id: frame.id,
        });
    }

    async fn serve(&self, connection: Connection) -> Result<()> {
        let from = Id(*connection.remote_id().as_bytes());
        let (mut send, mut recv) = tokio::time::timeout(DELIVERY_TIMEOUT, connection.accept_bi())
            .await
            .map_err(|_| anyhow!("зов так и не пришёл"))??;
        let raw = read_frame(&mut recv).await?;
        if raw.len() > MAX_RING_FRAME {
            return Err(anyhow!("кадр зова слишком велик"));
        }
        let (space, frame) = self
            .ctx
            .open_with_known_key::<RingFrame>(&raw)
            .ok_or_else(|| anyhow!("зов из чужого пространства"))?;
        if frame.space != space {
            return Err(anyhow!("пространство в зове не совпало с ключом"));
        }
        self.on_frame(space, from, frame);
        // Подтверждение: звонящему важно знать, до кого зов дошёл.
        send.write_all(&[1]).await?;
        send.finish()?;
        let _ = tokio::time::timeout(Duration::from_secs(2), connection.closed()).await;
        Ok(())
    }
}

impl std::fmt::Debug for RingProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RingProtocol")
    }
}

impl ProtocolHandler for RingProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        if let Err(err) = self.serve(connection).await {
            tracing::debug!(%err, "приём зова не удался");
        }
        Ok(())
    }
}

/// Доставить зов или отбой одному человеку. `Ok` — он его получил.
pub async fn deliver(
    endpoint: &Endpoint,
    key: &[u8; 32],
    peer: EndpointId,
    frame: &RingFrame,
) -> Result<()> {
    let sealed = seal(key, frame)?;
    let attempt = async {
        let connection = endpoint
            .connect(EndpointAddr::from(peer), RING_ALPN)
            .await?;
        let (mut send, mut recv) = connection.open_bi().await?;
        write_frame(&mut send, &sealed).await?;
        send.finish()?;
        let mut ack = [0u8; 1];
        recv.read_exact(&mut ack).await?;
        connection.close(0u32.into(), b"ok");
        anyhow::Ok(())
    };
    tokio::time::timeout(DELIVERY_TIMEOUT, attempt)
        .await
        .map_err(|_| anyhow!("позванный не ответил вовремя"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_frame_survives_sealing() {
        let key = [4u8; 32];
        let frame = RingFrame {
            space: Id([1u8; 32]),
            id: 99,
            channel: Id([2u8; 32]),
            cancel: false,
        };
        let sealed = seal(&key, &frame).unwrap();
        let back: RingFrame = super::super::wire::open(&key, &sealed).unwrap();
        assert_eq!(back, frame);
        assert!(super::super::wire::open::<RingFrame>(&[9u8; 32], &sealed).is_err());
    }
}
