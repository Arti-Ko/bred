//! Вход по приглашению без ключа в ссылке.
//!
//! Гость подключается к кому-то из участников, перечисленных в ссылке, и
//! показывает секрет приглашения. Участник сверяет его хеш с логом — не
//! истекло ли, не погашено ли, остались ли входы, не исключён ли сам гость, —
//! записывает вход и только тогда отдаёт ключ пространства. Соединение QUIC
//! уже подтвердило, кто на том конце, так что выдать себя за участника,
//! которому гость стучится, нельзя.

use anyhow::{anyhow, Result};
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr,
};
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, OnceLock, Weak},
    time::Duration,
};
use tokio::io::AsyncWriteExt;

use super::{
    ctx::Ctx,
    wire::{read_frame, write_frame},
    Net,
};
use crate::domain::{
    governance::{invite_proof, Ticket},
    now_ms, EventId, EventKind, Id, SpaceId,
};

pub const INVITE_ALPN: &[u8] = b"bred/invite/1";

/// Сколько ждём каждого из участников, указанных в ссылке.
const KNOCK_TIMEOUT: Duration = Duration::from_secs(10);
/// Сколько всего стучимся. Через ретранслятор первое соединение свежего узла
/// поднимается секунды, и одна попытка на каждого — это отказ на ровном месте.
const KNOCK_DEADLINE: Duration = Duration::from_secs(40);
const KNOCK_PAUSE: Duration = Duration::from_secs(2);
const UNKNOWN_INVITE: &str = "такого приглашения здесь не знают";
/// Запрос гостя — два идентификатора; больше быть не может.
const MAX_REQUEST: usize = 256;
/// Сколько адресов соседей отдаём новичку как точки входа.
const PEERS_FOR_GUEST: usize = 8;

#[derive(Debug, Serialize, Deserialize)]
struct Knock {
    invite: EventId,
    secret: [u8; 32],
}

/// Что получает гость, которого впустили.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Admission {
    pub space: SpaceId,
    pub name: String,
    pub key: [u8; 32],
    pub epoch: u64,
    pub key_event: Option<EventId>,
    /// Адреса участников: с них новичок начнёт рой и историю.
    pub peers: Vec<Vec<u8>>,
}

#[derive(Debug, Serialize, Deserialize)]
enum Answer {
    Admitted(Admission),
    Refused(String),
}

/// Приём гостей. Сеть ставит себя в `net` после запуска: запись о входе
/// должна сразу разойтись по рою, иначе счётчик входов у соседей отстанет.
#[derive(Clone)]
pub struct InviteProtocol {
    ctx: Arc<Ctx>,
    net: Arc<OnceLock<Weak<Net>>>,
    /// Решение и запись входа — под одним замком: иначе два гостя, пришедших
    /// одновременно по ссылке на один вход, оба увидели бы «ещё можно».
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl InviteProtocol {
    pub fn new(ctx: Arc<Ctx>, net: Arc<OnceLock<Weak<Net>>>) -> Self {
        Self {
            ctx,
            net,
            gate: Arc::default(),
        }
    }

    /// Решение по гостю. Отказ — с объяснением, которое увидит человек.
    fn decide(&self, guest: Id, knock: &Knock) -> std::result::Result<Admission, String> {
        let invite = self
            .ctx
            .store
            .invite(knock.invite)
            .ok()
            .flatten()
            .ok_or(UNKNOWN_INVITE)?;
        let space = self
            .ctx
            .space(invite.space)
            .ok_or("пригласивший больше не состоит в пространстве")?;
        if invite_proof(&knock.secret) != invite.proof {
            return Err("ссылка повреждена".into());
        }
        if self.ctx.is_removed(space.id, guest) {
            return Err("вас исключили из этого пространства".into());
        }
        if invite.revoked {
            return Err("приглашение погашено".into());
        }
        if invite.expired(now_ms()) {
            return Err("срок приглашения истёк".into());
        }
        // Повторный стук того, кого мы сами уже впустили (оборвалась связь на
        // ответе), лимит не тратит. Чужой записи «он уже входил» не верим.
        let returning = self
            .ctx
            .store
            .admitted_by_me(invite.id, guest)
            .unwrap_or(false);
        if invite.exhausted() && !returning {
            return Err("по этому приглашению уже вошли столько раз, сколько было можно".into());
        }

        let (epoch, key_event) = self.ctx.store.key_epoch(space.id).unwrap_or((0, None));
        let mut peers: Vec<Vec<u8>> = self
            .ctx
            .store
            .known_peers(space.id)
            .unwrap_or_default()
            .into_iter()
            .filter(|(id, _)| *id != guest)
            .filter_map(|(_, addr)| addr)
            .take(PEERS_FOR_GUEST)
            .collect();
        if let Some(net) = self.net.get().and_then(Weak::upgrade) {
            peers.insert(0, net.addr_now());
        }
        Ok(Admission {
            space: space.id,
            name: space.name,
            key: space.key,
            epoch,
            key_event,
            peers,
        })
    }

    async fn serve(&self, connection: Connection) -> Result<()> {
        let guest = Id(*connection.remote_id().as_bytes());
        let (mut send, mut recv) = tokio::time::timeout(KNOCK_TIMEOUT, connection.accept_bi())
            .await
            .map_err(|_| anyhow!("гость так и не постучался"))??;
        // Срок и на само чтение: иначе соединение без единого байта висело бы
        // вечно, и таких можно было бы набрать сколько угодно.
        let raw = tokio::time::timeout(KNOCK_TIMEOUT, read_frame(&mut recv))
            .await
            .map_err(|_| anyhow!("гость так и не сказал, зачем пришёл"))??;
        if raw.len() > MAX_REQUEST {
            return Err(anyhow!("запрос гостя слишком велик"));
        }
        let knock: Knock = postcard::from_bytes(&raw)?;

        let gate = self.gate.clone();
        let _one_guest_at_a_time = gate.lock().await;
        let answer = match self.decide(guest, &knock) {
            Ok(admission) => {
                // Вход записываем до ответа: упади мы между ними, гость
                // войдёт повторно, но лишний вход не останется неучтённым.
                let first = !self
                    .ctx
                    .store
                    .admitted_by_me(knock.invite, guest)
                    .unwrap_or(false);
                if first {
                    self.record(admission.space, knock.invite, guest).await?;
                }
                tracing::info!(guest = %guest.short(), space = %admission.space.short(), "впустили по приглашению");
                Answer::Admitted(admission)
            }
            Err(reason) => {
                tracing::info!(guest = %guest.short(), %reason, "в приглашении отказано");
                Answer::Refused(reason)
            }
        };
        write_frame(&mut send, &postcard::to_stdvec(&answer)?).await?;
        send.flush().await?;
        send.shutdown().await?;
        let _ = tokio::time::timeout(Duration::from_secs(2), connection.closed()).await;
        Ok(())
    }

    async fn record(&self, space: SpaceId, invite: EventId, guest: Id) -> Result<()> {
        let signed = self.ctx.commit(
            space,
            EventKind::InviteUse {
                invite,
                member: guest,
            },
        )?;
        if let Some(net) = self.net.get().and_then(Weak::upgrade) {
            let _ = net.publish_event(space, &signed).await;
        }
        Ok(())
    }
}

impl std::fmt::Debug for InviteProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InviteProtocol")
    }
}

impl ProtocolHandler for InviteProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        if let Err(err) = self.serve(connection).await {
            tracing::debug!(%err, "приём гостя не удался");
        }
        Ok(())
    }
}

/// Постучаться к участникам из ссылки по очереди, пока кто-то не впустит.
///
/// Отказ — окончательный ответ: участник проверил приглашение, и другой
/// скажет то же самое. Молчание — не ответ: стучимся по кругу до срока.
pub async fn knock(endpoint: &Endpoint, ticket: &Ticket) -> Result<Admission> {
    let doors: Vec<EndpointAddr> = ticket
        .bootstrap
        .iter()
        .filter_map(|raw| postcard::from_bytes::<EndpointAddr>(raw).ok())
        .filter(|addr| addr.id != endpoint.id())
        .collect();
    if doors.is_empty() {
        return Err(anyhow!("в ссылке нет ни одного адреса — попросите новую"));
    }

    let deadline = tokio::time::Instant::now() + KNOCK_DEADLINE;
    let mut refusal: Option<String> = None;
    while tokio::time::Instant::now() < deadline {
        for door in &doors {
            match tokio::time::timeout(KNOCK_TIMEOUT, knock_one(endpoint, door.clone(), ticket))
                .await
            {
                Ok(Ok(Answer::Admitted(admission))) if admission.space == ticket.space => {
                    return Ok(admission);
                }
                Ok(Ok(Answer::Admitted(_))) => {
                    refusal = Some("ответил участник другого пространства".into());
                }
                // «Не знаю такого приглашения» может сказать сосед, до которого
                // запись о нём ещё не доехала, — спросим следующего.
                Ok(Ok(Answer::Refused(reason))) if reason == UNKNOWN_INVITE => {
                    refusal.get_or_insert(reason);
                }
                Ok(Ok(Answer::Refused(reason))) => return Err(anyhow!("{reason}")),
                Ok(Err(err)) => tracing::debug!(%err, "участник из ссылки не ответил"),
                Err(_) => tracing::debug!("участник из ссылки не ответил вовремя"),
            }
        }
        if refusal.is_some() {
            break;
        }
        tokio::time::sleep(KNOCK_PAUSE).await;
    }
    Err(match refusal {
        Some(reason) => anyhow!("{reason}"),
        None => anyhow!(
            "никого из участников сейчас нет в сети — попробуйте, когда пригласивший будет онлайн"
        ),
    })
}

async fn knock_one(endpoint: &Endpoint, addr: EndpointAddr, ticket: &Ticket) -> Result<Answer> {
    let connection = endpoint.connect(addr, INVITE_ALPN).await?;
    let (mut send, mut recv) = connection.open_bi().await?;
    let knock = Knock {
        invite: ticket.invite,
        secret: ticket.secret,
    };
    write_frame(&mut send, &postcard::to_stdvec(&knock)?).await?;
    send.flush().await?;
    let raw = read_frame(&mut recv).await?;
    let _ = send.shutdown().await;
    connection.close(0u32.into(), "готово".as_bytes());
    Ok(postcard::from_bytes(&raw)?)
}
