//! БРЕД мимо VPN.
//!
//! Весь трафик Мака может забирать VPN с TUN-интерфейсом. Для P2P это хуже,
//! чем ничего: пробивка NAT через туннель не работает, ретранслятор живёт
//! ровно столько, сколько туннель, а в России туннель лежит часами. 21.11.2026
//! так и было: напрямую ретранслятор отдавал 8 МБ/с, а через туннель
//! соединение с ним умирало по таймауту, и никто никого не видел.
//!
//! Поэтому при запуске выбираем физический интерфейс — Wi-Fi или Ethernet со
//! шлюзом, не туннель — и проверяем, что через него есть интернет. Есть — все
//! сокеты iroh привязываются к нему (`bred-netbind`), VPN их не касается. Нет
//! (например, интернет только через корпоративный VPN) — работаем как раньше,
//! обычным маршрутом.
//!
//! `BRED_THROUGH_VPN=1` выключает обход — для проверки и на крайний случай.

use std::{net::SocketAddr, num::NonZeroU32, time::Duration};

/// Куда стучимся, проверяя, есть ли интернет напрямую. Три разных хозяина:
/// один из них провайдер может и резать.
const PROBES: [&str; 3] = ["1.1.1.1:443", "8.8.8.8:443", "77.88.8.8:443"];
/// Сколько ждём проверочного соединения.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Физический интерфейс, через который ходим.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub index: NonZeroU32,
    pub name: String,
}

/// Включить обход, если через физический интерфейс есть интернет.
/// Возвращает выбранный интерфейс; `None` — идём обычным маршрутом.
pub async fn engage() -> Option<Interface> {
    if std::env::var_os("BRED_THROUGH_VPN").is_some() {
        bred_netbind::set(None);
        tracing::info!("обход VPN выключен переменной BRED_THROUGH_VPN");
        return None;
    }
    for candidate in physical() {
        if online_via(candidate.index).await {
            bred_netbind::set(Some(candidate.index));
            tracing::info!(interface = %candidate.name, "сеть БРЕДа идёт мимо VPN");
            return Some(candidate);
        }
        tracing::debug!(interface = %candidate.name, "через интерфейс нет интернета");
    }
    bred_netbind::set(None);
    tracing::info!("физического интерфейса с интернетом нет — идём обычным маршрутом");
    None
}

/// Следить, не сменился ли физический интерфейс: ноутбук переехал с Wi-Fi на
/// кабель, Wi-Fi переподключился с другим номером. Система сама пересоздаёт
/// сокеты при такой перемене, и к этому моменту номер должен быть уже новым.
pub fn track(mut current: Interface) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        loop {
            ticker.tick().await;
            if bred_netbind::current().is_none() {
                return; // обход выключили — следить не за чем
            }
            let now = physical();
            if now.contains(&current) {
                continue;
            }
            // Прежнего интерфейса нет — берём первый живой. Интернет через
            // него проверим, но не дольше секунды на каждый: ждать нельзя,
            // система вот-вот пересоздаст сокеты.
            let next = match now.first() {
                Some(next) => next.clone(),
                None => continue, // сети нет вовсе — ждём, номер не трогаем
            };
            bred_netbind::set(Some(next.index));
            tracing::info!(from = %current.name, to = %next.name, "физический интерфейс сменился");
            current = next;
        }
    });
}

/// Как часто сторож смотрит на ретрансляторы.
const GUARD_TICK: Duration = Duration::from_secs(10);
/// За какое окно считаем неответившие пинги.
const GUARD_WINDOW: Duration = Duration::from_secs(60);
/// Сколько неответивших пингов за окно — уже не случайность, а заморозка.
const GUARD_TIMEOUTS: u64 = 3;
/// Не переключаемся чаще: после смены дороге нужно время показать себя.
const GUARD_HOLD: Duration = Duration::from_secs(120);

/// Сторож дороги: мимо VPN или через него.
///
/// Обход VPN спасает, когда туннель лежит. Но бывает наоборот: VPN жив, а
/// напрямую ТСПУ замораживает соединения с ретрансляторами на зарубежных
/// хостингах — пинги к ним раз за разом уходят без ответа, звук стоит по 15
/// секунд (27.11.2026, друг на Windows). Тогда пускаем БРЕД через туннель. Если
/// и там ретрансляторы молчат или туннель пропал — возвращаемся напрямую.
pub fn guard(endpoint: iroh::Endpoint, direct: Interface) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(GUARD_TICK);
        let mut samples: std::collections::VecDeque<(std::time::Instant, u64)> =
            std::collections::VecDeque::new();
        let mut switched = std::time::Instant::now();
        let mut through_vpn = false;
        loop {
            ticker.tick().await;
            if endpoint.is_closed() {
                return;
            }
            let now = std::time::Instant::now();
            samples.push_back((now, iroh::bred::relay_ping_timeouts()));
            while samples
                .front()
                .is_some_and(|(at, _)| now.duration_since(*at) > GUARD_WINDOW)
            {
                samples.pop_front();
            }
            let frozen = recent_timeouts(&samples) >= GUARD_TIMEOUTS;
            let held = now.duration_since(switched) >= GUARD_HOLD;
            let tunnel = tunnel_present();

            let next = decide(through_vpn, frozen, held, tunnel);
            if next == through_vpn {
                continue;
            }
            through_vpn = next;
            switched = now;
            samples.clear();
            if through_vpn {
                bred_netbind::set(None);
                tracing::warn!("ретрансляторы напрямую замерзают — идём через VPN");
            } else {
                // Физический интерфейс мог смениться, пока шли через туннель.
                let iface = physical()
                    .into_iter()
                    .find(|i| i == &direct)
                    .or_else(|| physical().into_iter().next())
                    .unwrap_or_else(|| direct.clone());
                bred_netbind::set(Some(iface.index));
                tracing::warn!(interface = %iface.name, "через VPN не лучше — снова мимо него");
                track(iface);
            }
            // Привязка новая — iroh пересоздаёт сокеты и переподключает ретранслятор.
            endpoint.network_change().await;
        }
    });
}

/// Сколько пингов осталось без ответа за окно выборок.
fn recent_timeouts(samples: &std::collections::VecDeque<(std::time::Instant, u64)>) -> u64 {
    match (samples.front(), samples.back()) {
        (Some((_, first)), Some((_, last))) => last.saturating_sub(*first),
        _ => 0,
    }
}

/// Куда идти дальше: `true` — через VPN.
fn decide(through_vpn: bool, frozen: bool, held: bool, tunnel: bool) -> bool {
    if through_vpn {
        // Туннель пропал — делать в нём нечего; молчит и там — пробуем напрямую.
        !(!tunnel || (frozen && held))
    } else {
        frozen && held && tunnel
    }
}

/// Есть ли в системе поднятый VPN-туннель.
pub fn tunnel_present() -> bool {
    netdev::get_interfaces().into_iter().any(|i| {
        i.is_up()
            && !i.is_loopback()
            && !i.ipv4.is_empty()
            && (i.is_tun() || i.is_point_to_point() || looks_like_tunnel(&i.name))
    })
}

/// Физические интерфейсы со шлюзом, по порядку системы. Туннели, петля и
/// точка-точка (PPP, большинство VPN) не годятся.
fn physical() -> Vec<Interface> {
    netdev::get_interfaces()
        .into_iter()
        .filter(|i| i.is_up() && !i.is_loopback() && !i.is_tun() && !i.is_point_to_point())
        .filter(|i| !looks_like_tunnel(&i.name))
        .filter(|i| i.gateway.is_some() && !i.ipv4.is_empty())
        .filter_map(|i| {
            Some(Interface {
                index: NonZeroU32::new(i.index)?,
                name: i.name,
            })
        })
        .collect()
}

/// IPv4-адрес интерфейса по его номеру — чтобы ходить по HTTP той же дорогой.
pub fn ipv4_of(index: NonZeroU32) -> Option<std::net::Ipv4Addr> {
    netdev::get_interfaces()
        .into_iter()
        .find(|i| i.index == index.get())
        .and_then(|i| i.ipv4.first().map(|net| net.addr()))
}

/// Туннели, которые система честно туннелями не называет.
fn looks_like_tunnel(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "utun",
        "tun",
        "tap",
        "wg",
        "ipsec",
        "ppp",
        "wintun",
        "zerotier",
        "tailscale",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

/// Есть ли интернет через этот интерфейс: хоть одно проверочное соединение.
async fn online_via(index: NonZeroU32) -> bool {
    let mut probes = tokio::task::JoinSet::new();
    for target in PROBES {
        let Ok(addr) = target.parse::<SocketAddr>() else {
            continue;
        };
        probes.spawn(async move {
            tokio::time::timeout(
                PROBE_TIMEOUT,
                bred_netbind::connect_tcp_via(addr, Some(index)),
            )
            .await
            .is_ok_and(|r| r.is_ok())
        });
    }
    while let Some(done) = probes.join_next().await {
        if done.unwrap_or(false) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goes_through_vpn_only_when_relays_freeze_and_tunnel_is_up() {
        // Мимо VPN: переключаемся, только если всё сразу.
        assert!(decide(false, true, true, true));
        assert!(!decide(false, true, true, false), "без туннеля некуда");
        assert!(
            !decide(false, true, false, true),
            "рано после прошлой смены"
        );
        assert!(!decide(false, false, true, true), "ретрансляторы живы");
        // Через VPN: остаёмся, пока туннель есть и там не хуже.
        assert!(decide(true, false, true, true));
        assert!(decide(true, true, false, true), "рано судить");
        assert!(
            !decide(true, false, false, false),
            "туннель пропал — сразу назад"
        );
        assert!(!decide(true, true, true, true), "и там молчат — назад");
    }

    #[test]
    fn counts_timeouts_within_window() {
        let t = std::time::Instant::now();
        let mut samples = std::collections::VecDeque::new();
        assert_eq!(recent_timeouts(&samples), 0);
        samples.push_back((t, 10));
        samples.push_back((t, 11));
        samples.push_back((t, 14));
        assert_eq!(recent_timeouts(&samples), 4);
    }

    #[test]
    fn tunnels_are_recognised_by_name() {
        for name in ["utun4", "tun0", "wg0", "Wintun", "ppp0", "tailscale0"] {
            assert!(looks_like_tunnel(name), "{name} — туннель");
        }
        for name in ["en0", "eth0", "Wi-Fi", "Ethernet"] {
            assert!(!looks_like_tunnel(name), "{name} — не туннель");
        }
    }
}
