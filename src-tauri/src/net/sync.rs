//! Досинхронизация истории между двумя узлами.
//!
//! Gossip разносит только то, что происходит прямо сейчас. Всё, что было, пока
//! ты был офлайн, приезжает этим протоколом: при встрече с пиром обмениваемся
//! векторами версий (`автор -> максимальный seq`) и досылаем друг другу
//! недостающее. Это дёшево: вектор — несколько десятков байт, а не вся история.

use anyhow::{anyhow, Result};
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr,
};
use std::sync::Arc;

use super::{
    ctx::Ctx,
    wire::{
        open, read_frame, seal, vector_from_wire, vector_to_wire, write_frame, SyncFrame,
        SYNC_BATCH,
    },
};
use crate::domain::SpaceId;

pub const SYNC_ALPN: &[u8] = b"bred/sync/1";

/// Обработчик входящих запросов досинхронизации.
#[derive(Clone)]
pub struct SyncProtocol {
    ctx: Arc<Ctx>,
}

impl SyncProtocol {
    pub fn new(ctx: Arc<Ctx>) -> Self {
        Self { ctx }
    }
}

impl std::fmt::Debug for SyncProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SyncProtocol")
    }
}

impl ProtocolHandler for SyncProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let ctx = self.ctx.clone();
        if let Err(err) = serve(ctx, connection).await {
            // Разрыв соединения — норма для P2P, не повод шуметь в ошибках.
            tracing::debug!(%err, "сессия досинхронизации завершилась");
        }
        Ok(())
    }
}

async fn serve(ctx: Arc<Ctx>, connection: Connection) -> Result<()> {
    // Раундов может быть несколько: за один заход отдаём не больше пачки,
    // а история бывает длиннее. Цикл живёт, пока пир не закроет соединение.
    while let Ok((send, recv)) = connection.accept_bi().await {
        serve_round(&ctx, send, recv).await?;
    }
    Ok(())
}

/// Один обмен «запрос — ответ — досылка» поверх пары потоков.
/// Вынесено из соединения, чтобы протокол можно было прогнать в тесте трубой.
pub async fn serve_round<W, R>(ctx: &Ctx, mut send: W, mut recv: R) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncReadExt + Unpin,
{
    let raw = read_frame(&mut recv).await?;
    // Пространство в запросе не указано открытым текстом: перебираем свои ключи,
    // пока один не подойдёт. Заодно это отсекает чужаков без ключа.
    let Some((space, frame)) = ctx.open_with_known_key::<SyncFrame>(&raw) else {
        return Err(anyhow!("запрос не подошёл ни к одному известному ключу"));
    };

    let SyncFrame::Request { space: asked, have } = frame else {
        return Err(anyhow!("ожидался Request"));
    };
    if asked != space {
        return Err(anyhow!("пространство в запросе не совпало с ключом"));
    }

    let key = ctx
        .space(space)
        .ok_or_else(|| anyhow!("неизвестное пространство"))?
        .key;

    let peer_has = vector_from_wire(have);
    let events = ctx.store.events_missing_for(space, &peer_has, SYNC_BATCH)?;
    let mine = vector_to_wire(&ctx.store.version_vector(space)?);
    tracing::debug!(space = %space.short(), count = events.len(), "отдаём историю");

    write_frame(
        &mut send,
        &seal(&key, &SyncFrame::Response { events, have: mine })?,
    )
    .await?;

    // Пир досылает то, чего не хватало нам.
    if let Ok(raw) = read_frame(&mut recv).await {
        if let Ok(SyncFrame::Push { events }) = open::<SyncFrame>(&key, &raw) {
            let fresh = ctx.apply_batch(&events)?;
            tracing::debug!(space = %space.short(), fresh, "приняли историю");
        }
    }

    send.flush().await?;
    send.shutdown().await?;
    Ok(())
}

/// Сколько раундов готовы отработать за одну встречу. Потолок нужен, чтобы
/// пир, который отвечает вечно, не держал нас бесконечно.
const MAX_ROUNDS: usize = 64;

/// Активная сторона: подключиться к пиру и обменяться историей.
pub async fn sync_with(
    ctx: Arc<Ctx>,
    endpoint: &Endpoint,
    peer: EndpointAddr,
    space: SpaceId,
) -> Result<usize> {
    let connection = endpoint.connect(peer, SYNC_ALPN).await?;
    let total = sync_over(&ctx, &connection, space).await;
    connection.close(0u32.into(), "готово".as_bytes());
    total
}

/// Обмен историей поверх **уже поднятого** соединения.
///
/// Отделено от дозвона намеренно. Сверка идёт к каждому соседу каждые пять
/// секунд, и раньше каждая заводила своё соединение: рукопожатие QUIC, обмен
/// ключами TLS — и всё это выбрасывалось через мгновение. Дорого здесь не
/// столько процессорное время, сколько то, что свежее соединение начинает
/// подбор скорости с нуля и может заново дёрнуть пробивку NAT — ровно поперёк
/// того звонка, который идёт по соседнему пути того же endpoint.
///
/// Соединение остаётся открытым, а каждый раунд просит у него новую пару
/// потоков: принимающая сторона (`serve`) для этого и написана циклом.
pub async fn sync_over(ctx: &Ctx, connection: &Connection, space: SpaceId) -> Result<usize> {
    let key = ctx
        .space(space)
        .ok_or_else(|| anyhow!("неизвестное пространство"))?
        .key;

    let mut total = 0usize;
    for _ in 0..MAX_ROUNDS {
        let (send, recv) = connection.open_bi().await?;
        let round = sync_round(ctx, send, recv, space, &key).await?;
        total += round.received;
        // Обе стороны исчерпались — больше ходить не за чем.
        if round.received == 0 && round.sent == 0 {
            break;
        }
    }
    Ok(total)
}

/// Итог одного раунда: сколько приняли и сколько отдали.
pub struct Round {
    pub received: usize,
    pub sent: usize,
}

/// Один раунд активной стороны поверх пары потоков.
pub async fn sync_round<W, R>(
    ctx: &Ctx,
    mut send: W,
    mut recv: R,
    space: SpaceId,
    key: &[u8; 32],
) -> Result<Round>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    R: tokio::io::AsyncReadExt + Unpin,
{
    let have = vector_to_wire(&ctx.store.version_vector(space)?);
    write_frame(&mut send, &seal(key, &SyncFrame::Request { space, have })?).await?;
    send.flush().await?;

    let raw = read_frame(&mut recv).await?;
    let SyncFrame::Response { events, have } = open::<SyncFrame>(key, &raw)? else {
        return Err(anyhow!("ожидался Response"));
    };

    let received = ctx.apply_batch(&events)?;

    // Симметрично досылаем то, чего нет у пира.
    let peer_has = vector_from_wire(have);
    let missing = ctx.store.events_missing_for(space, &peer_has, SYNC_BATCH)?;
    let sent = missing.len();
    write_frame(&mut send, &seal(key, &SyncFrame::Push { events: missing })?).await?;
    send.flush().await?;
    send.shutdown().await?;

    Ok(Round { received, sent })
}
