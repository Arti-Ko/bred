//! Обнаружение в локальной сети.
//!
//! В локалке никакой внешней инфраструктуры не нужно вообще: раз в несколько
//! секунд шлём UDP-мультикаст со своим адресом и метками пространств.
//!
//! Чужой в той же сети не должен узнать ни устройство, ни пространства. Адрес
//! закрыт одноразовым ключом, ключ — маской из ключа каждого пространства, а
//! метки — производные от ключа и текущего десятиминутного окна. Через десять
//! минут тот же ноутбук в том же кафе выглядит как другой набор случайных байт.

use anyhow::Result;
use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
    time::Duration,
};
use tokio::{net::UdpSocket, time};

use super::{
    ctx::Ctx,
    wire::{self, Beacon, BeaconSlot},
};
use crate::domain::{Space, SpaceId};

const GROUP: Ipv4Addr = Ipv4Addr::new(239, 42, 42, 42);
const PORT: u16 = 47421;
/// Как часто кричим о себе в локальную сеть.
///
/// Это единственный способ найтись там, где интернета нет вовсе, и первый
/// способ там, где он есть: пакет уходит соседям напрямую, без ретранслятора и
/// без публичного справочника.
const INTERVAL: Duration = Duration::from_secs(2);
/// Мультикаст-датаграмма должна помещаться в один пакет без фрагментации.
const MAX_BEACON: usize = 1400;
/// Сколько пространств помещается в один маячок с запасом. Если их больше,
/// маячок уходит несколькими пакетами.
const SLOTS_PER_BEACON: usize = 16;
/// Окно, в течение которого метки пространств не меняются.
const TAG_WINDOW_MS: i64 = 10 * 60 * 1000;

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
            let addr = addr_bytes.read().clone();
            let spaces = ctx.space_list();
            if addr.is_empty() || spaces.is_empty() {
                continue;
            }
            let window = window_of(crate::domain::now_ms());
            for group in spaces.chunks(SLOTS_PER_BEACON) {
                match build(&addr, group, window).and_then(|b| Ok(postcard::to_stdvec(&b)?)) {
                    Ok(raw) if raw.len() <= MAX_BEACON => {
                        let _ = socket.send_to(&raw, target).await;
                    }
                    Ok(raw) => {
                        tracing::debug!(len = raw.len(), "маячок слишком большой, пропускаем")
                    }
                    Err(err) => tracing::debug!(%err, "не удалось собрать маячок"),
                }
            }
        }
    });

    rx
}

fn window_of(now_ms: i64) -> u64 {
    now_ms.max(0).div_euclid(TAG_WINDOW_MS) as u64
}

/// Маска для одноразового ключа в месте этого пространства.
fn mask(space: &Space, salt: &[u8; 16], window: u64) -> [u8; 32] {
    let mut input = salt.to_vec();
    input.extend_from_slice(&window.to_le_bytes());
    *blake3::keyed_hash(&space.lan_key(), &input).as_bytes()
}

fn xor(a: [u8; 32], b: [u8; 32]) -> [u8; 32] {
    std::array::from_fn(|i| a[i] ^ b[i])
}

/// Маячок для группы пространств в данном окне времени.
fn build(addr: &[u8], spaces: &[Space], window: u64) -> Result<Beacon> {
    let once: [u8; 32] = rand::random();
    let salt: [u8; 16] = rand::random();
    Ok(Beacon {
        sealed: wire::seal(&once, &addr.to_vec())?,
        salt,
        slots: spaces
            .iter()
            .map(|space| BeaconSlot {
                tag: space.lan_tag(window),
                key: xor(once, mask(space, &salt, window)),
            })
            .collect(),
    })
}

/// Разбор чужого маячка: оставляем только пространства, ключи которых у нас есть.
///
/// Метки сверяем с соседними окнами тоже: часы у соседей расходятся, а граница
/// окна приходится на середину чьего-нибудь звонка.
fn interpret(ctx: &Ctx, raw: &[u8]) -> Option<Found> {
    let beacon: Beacon = postcard::from_bytes(raw).ok()?;
    let now = window_of(crate::domain::now_ms());
    let windows = [now.saturating_sub(1), now, now + 1];

    let mut addr: Option<Vec<u8>> = None;
    let mut shared = Vec::new();
    for space in ctx.space_list() {
        let found = windows.iter().find_map(|&window| {
            let tag = space.lan_tag(window);
            let slot = beacon.slots.iter().find(|slot| slot.tag == tag)?;
            let once = xor(slot.key, mask(&space, &beacon.salt, window));
            wire::open::<Vec<u8>>(&once, &beacon.sealed).ok()
        });
        if let Some(opened) = found {
            // Все места маячка закрывают один и тот же адрес; первого хватает.
            addr.get_or_insert(opened);
            shared.push(space.id);
        }
    }

    let addr = addr.filter(|addr| !addr.is_empty())?;
    (!shared.is_empty()).then_some((addr, shared))
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
    use crate::{identity::Identity, store::Store};

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

    fn now_window(_: &Ctx) -> u64 {
        window_of(crate::domain::now_ms())
    }

    fn raw(beacon: &Beacon) -> Vec<u8> {
        postcard::to_stdvec(beacon).unwrap()
    }

    #[test]
    fn beacon_from_same_space_is_recognised() {
        let ctx = ctx_with(space(7));
        let beacon = build(&[1, 2, 3], &[space(7)], now_window(&ctx)).unwrap();
        let (addr, shared) = interpret(&ctx, &raw(&beacon)).expect("свой должен опознаться");
        assert_eq!(addr, vec![1, 2, 3]);
        assert_eq!(shared, vec![space(7).id]);
    }

    #[test]
    fn beacon_from_stranger_is_ignored() {
        let ctx = ctx_with(space(7));
        let beacon = build(&[1, 2, 3], &[space(9)], now_window(&ctx)).unwrap();
        assert!(
            interpret(&ctx, &raw(&beacon)).is_none(),
            "чужой ключ не должен совпасть"
        );
    }

    #[test]
    fn address_is_not_visible_on_the_wire() {
        let addr = b"endpoint-address-with-a-stable-node-id".to_vec();
        let packet = raw(&build(&addr, &[space(7)], 1).unwrap());
        assert!(
            !packet
                .windows(addr.len())
                .any(|chunk| chunk == addr.as_slice()),
            "адрес узла не должен уходить открытым текстом"
        );
    }

    #[test]
    fn tags_rotate_between_windows_and_beacons_differ() {
        let first = build(&[1], &[space(7)], 100).unwrap();
        let again = build(&[1], &[space(7)], 100).unwrap();
        let later = build(&[1], &[space(7)], 101).unwrap();
        assert_eq!(first.slots[0].tag, again.slots[0].tag);
        assert_ne!(
            first.slots[0].tag, later.slots[0].tag,
            "метка обязана смениться"
        );
        assert_ne!(
            first.sealed, again.sealed,
            "одинаковых маячков быть не должно"
        );
        assert_ne!(first.slots[0].key, again.slots[0].key);
    }

    #[test]
    fn neighbouring_window_is_still_accepted() {
        let ctx = ctx_with(space(7));
        let now = now_window(&ctx);
        for window in [now - 1, now + 1] {
            let beacon = build(&[4, 2], &[space(7)], window).unwrap();
            assert!(interpret(&ctx, &raw(&beacon)).is_some(), "окно {window}");
        }
        let stale = build(&[4, 2], &[space(7)], now - 3).unwrap();
        assert!(
            interpret(&ctx, &raw(&stale)).is_none(),
            "старый маячок не повторить"
        );
    }

    #[test]
    fn forged_key_does_not_open_address() {
        let ctx = ctx_with(space(7));
        let mut beacon = build(&[1, 2, 3], &[space(7)], now_window(&ctx)).unwrap();
        beacon.slots[0].key[0] ^= 1;
        assert!(interpret(&ctx, &raw(&beacon)).is_none());
    }

    #[test]
    fn many_spaces_split_into_packets_that_fit() {
        let spaces: Vec<Space> = (0..40).map(space).collect();
        let addr = vec![9u8; 300];
        for group in spaces.chunks(SLOTS_PER_BEACON) {
            let packet = raw(&build(&addr, group, 5).unwrap());
            assert!(packet.len() <= MAX_BEACON, "{} байт", packet.len());
        }
    }

    #[test]
    fn garbage_packet_does_not_panic() {
        let ctx = ctx_with(space(7));
        assert!(interpret(&ctx, &[0xff, 0xff, 0xff]).is_none());
    }
}
