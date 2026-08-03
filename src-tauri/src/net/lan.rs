//! Обнаружение в локальной сети.
//!
//! В локалке никакой внешней инфраструктуры не нужно вообще: раз в несколько
//! секунд шлём UDP-мультикаст со своим адресом и метками пространств. Метка —
//! производная от ключа пространства, поэтому чужой в той же сети видит только
//! случайные байты и не может понять, к чему мы вообще подключены.

use anyhow::Result;
use std::{
    collections::HashSet,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
    time::Duration,
};
use tokio::{net::UdpSocket, time};

use super::{ctx::Ctx, wire::Beacon};
use crate::domain::SpaceId;

const GROUP: Ipv4Addr = Ipv4Addr::new(239, 42, 42, 42);
const PORT: u16 = 47421;
const INTERVAL: Duration = Duration::from_secs(5);
/// Мультикаст-датаграмма должна помещаться в один пакет без фрагментации.
const MAX_BEACON: usize = 1400;

/// Кого нашли: адрес пира и пространства, которые у нас с ним общие.
pub type Found = (Vec<u8>, Vec<SpaceId>);

/// Поднимает маячок. Возвращает канал с найденными соседями.
pub fn spawn(
    ctx: Arc<Ctx>,
    addr_bytes: Arc<parking_lot::RwLock<Vec<u8>>>,
) -> tokio::sync::mpsc::UnboundedReceiver<Found> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(async move {
        // Отключаемый маячок: в тестах интернет-пути он бы подменял собой всё,
        // что мы там проверяем, и дыра в этом пути осталась бы незамеченной.
        if std::env::var("BRED_NO_LAN").is_ok() {
            tracing::info!("локальный маячок выключен переменной окружения");
            return;
        }
        let socket = match bind() {
            Ok(socket) => Arc::new(socket),
            Err(err) => {
                // Локалка — приятный бонус, а не обязательное условие:
                // без неё остаётся обычный интернет-путь через iroh.
                tracing::warn!(%err, "локальный маячок недоступен, работаем только через интернет");
                return;
            }
        };

        let listener = socket.clone();
        let ctx_rx = ctx.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; MAX_BEACON];
            loop {
                let Ok((len, _from)) = listener.recv_from(&mut buf).await else {
                    continue;
                };
                if let Some(found) = interpret(&ctx_rx, &buf[..len]) {
                    let _ = tx.send(found);
                }
            }
        });

        let target = SocketAddr::from(SocketAddrV4::new(GROUP, PORT));
        let mut ticker = time::interval(INTERVAL);
        loop {
            ticker.tick().await;
            let beacon = Beacon {
                addr: addr_bytes.read().clone(),
                tags: ctx.space_list().iter().map(|s| s.lan_tag()).collect(),
            };
            if beacon.addr.is_empty() || beacon.tags.is_empty() {
                continue;
            }
            match postcard::to_stdvec(&beacon) {
                Ok(raw) if raw.len() <= MAX_BEACON => {
                    let _ = socket.send_to(&raw, target).await;
                }
                Ok(raw) => tracing::debug!(len = raw.len(), "маячок слишком большой, пропускаем"),
                Err(err) => tracing::debug!(%err, "не удалось собрать маячок"),
            }
        }
    });

    rx
}

/// Разбор чужого маячка: оставляем только пространства, ключи которых у нас есть.
fn interpret(ctx: &Ctx, raw: &[u8]) -> Option<Found> {
    let beacon: Beacon = postcard::from_bytes(raw).ok()?;
    let tags: HashSet<[u8; 32]> = beacon.tags.into_iter().collect();

    let shared: Vec<SpaceId> = ctx
        .space_list()
        .into_iter()
        .filter(|space| tags.contains(&space.lan_tag()))
        .map(|space| space.id)
        .collect();

    if shared.is_empty() || beacon.addr.is_empty() {
        return None;
    }
    Some((beacon.addr, shared))
}

/// Сокет с переиспользованием адреса — иначе два экземпляра БРЕД
/// на одной машине (обычное дело при отладке) не поднимутся одновременно.
fn bind() -> Result<UdpSocket> {
    use socket2::{Domain, Protocol, SockAddr, Socket, Type};

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.bind(&SockAddr::from(SocketAddrV4::new(
        Ipv4Addr::UNSPECIFIED,
        PORT,
    )))?;
    socket.join_multicast_v4(&GROUP, &Ipv4Addr::UNSPECIFIED)?;
    socket.set_multicast_loop_v4(true)?;
    socket.set_nonblocking(true)?;

    Ok(UdpSocket::from_std(socket.into())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{domain::Space, identity::Identity, store::Store};

    fn ctx_with(space: Space) -> Arc<Ctx> {
        let store = Arc::new(Store::in_memory().unwrap());
        let identity = Identity::load_or_create(&store).unwrap();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        Arc::new(Ctx::new(
            store,
            identity,
            vec![space],
            crate::domain::Clock::default(),
            tx,
            std::env::temp_dir().join("bred-test-blobs"),
        ))
    }

    fn space(key: u8) -> Space {
        Space {
            id: crate::domain::Id([key; 32]),
            name: "тест".into(),
            key: [key; 32],
            direct: None,
        }
    }

    #[test]
    fn beacon_from_same_space_is_recognised() {
        let ctx = ctx_with(space(7));
        let beacon = Beacon {
            addr: vec![1, 2, 3],
            tags: vec![space(7).lan_tag()],
        };
        let raw = postcard::to_stdvec(&beacon).unwrap();
        let (addr, shared) = interpret(&ctx, &raw).expect("свой должен опознаться");
        assert_eq!(addr, vec![1, 2, 3]);
        assert_eq!(shared, vec![space(7).id]);
    }

    #[test]
    fn beacon_from_stranger_is_ignored() {
        let ctx = ctx_with(space(7));
        let beacon = Beacon {
            addr: vec![1, 2, 3],
            tags: vec![space(9).lan_tag()],
        };
        let raw = postcard::to_stdvec(&beacon).unwrap();
        assert!(
            interpret(&ctx, &raw).is_none(),
            "чужой ключ не должен совпасть"
        );
    }

    #[test]
    fn garbage_packet_does_not_panic() {
        let ctx = ctx_with(space(7));
        assert!(interpret(&ctx, &[0xff, 0xff, 0xff]).is_none());
    }
}
