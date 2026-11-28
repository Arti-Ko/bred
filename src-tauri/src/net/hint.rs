//! Подсказка своего внешнего адреса, когда iroh не смог узнать его сам.
//!
//! Внешний адрес iroh узнаёт у ретрансляторов (QAD — QUIC поверх UDP). Если
//! провайдер режет UDP к зарубежным хостингам, а ретрансляторы n0 стоят на
//! Hetzner, ответа нет, и узел знает о себе только адрес домашней сети. Тогда
//! пробивать NAT собеседнику не к чему, и весь звонок идёт через ретранслятор,
//! который ТСПУ время от времени замораживает. 27.11.2026 так и было у друга
//! на Windows: прямого пути нет, звук стоит по 15 секунд.
//!
//! Поэтому, пока внешнего адреса нет, спрашиваем свой публичный IP по HTTPS —
//! первым у Яндекса, его не режут — и подсказываем iroh «IP + порт нашего
//! сокета». Большинство домашних роутеров сохраняют порт при трансляции, и
//! такой адрес годится для пробивки. Не угадали — вреда нет: лишний кандидат
//! просто не ответит. Как только iroh узнал адрес сам, подсказку убираем.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use iroh::Endpoint;

/// Кого спрашиваем о своём IP — по порядку, до первого ответа.
const ECHO: [&str; 3] = [
    "https://ipv4-internet.yandex.net/api/v0/ip",
    "https://api.ipify.org",
    "https://icanhazip.com",
];
/// Сколько ждём ответа одного сервиса.
const ECHO_TIMEOUT: Duration = Duration::from_secs(5);
/// Первая проверка — когда iroh уже успел спросить ретрансляторы сам.
const FIRST_CHECK: Duration = Duration::from_secs(15);
/// Дальше проверяем раз в минуту: сеть могла смениться.
const RECHECK: Duration = Duration::from_secs(60);

/// Следить за внешним адресом, пока жив endpoint.
pub fn watch(endpoint: Endpoint) {
    tokio::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        let mut hinted: Option<SocketAddr> = None;
        loop {
            hinted = refresh(&endpoint, hinted).await;
            if endpoint.is_closed() {
                return;
            }
            tokio::time::sleep(RECHECK).await;
        }
    });
}

/// Один круг: убрать подсказку, если адрес уже известен, или поставить свежую.
async fn refresh(endpoint: &Endpoint, hinted: Option<SocketAddr>) -> Option<SocketAddr> {
    let addr = endpoint.addr();
    let known = addr
        .ip_addrs()
        .copied()
        .filter(|a| Some(*a) != hinted)
        .any(|a| is_public(a.ip()));
    if known {
        if let Some(old) = hinted {
            endpoint.remove_external_addr(&old).await;
            tracing::debug!(%old, "внешний адрес известен, подсказка не нужна");
        }
        return None;
    }

    // Идём через VPN — спросили бы адрес VPN-сервера, а он нам не поможет.
    if bred_netbind::current().is_none() && super::bypass::tunnel_present() {
        return hinted;
    }
    let Some(port) = v4_port(&endpoint.bound_sockets()) else {
        return hinted;
    };
    let Some(ip) = ask_public_ip().await else {
        tracing::debug!("свой публичный IP узнать не удалось");
        return hinted;
    };
    let next = SocketAddr::new(IpAddr::V4(ip), port);
    if hinted == Some(next) {
        return hinted;
    }
    if let Some(old) = hinted {
        endpoint.remove_external_addr(&old).await;
    }
    endpoint.add_external_addr(next).await;
    tracing::info!(addr = %next, "внешний адрес подсказан по HTTPS");
    Some(next)
}

/// Свой публичный IPv4 — у первого ответившего сервиса.
async fn ask_public_ip() -> Option<Ipv4Addr> {
    let client = echo_client()?;
    for url in ECHO {
        let Ok(resp) = client.get(url).send().await else {
            continue;
        };
        let Ok(body) = resp.text().await else {
            continue;
        };
        if let Some(ip) = parse_ip(&body).filter(|ip| is_public(IpAddr::V4(*ip))) {
            return Some(ip);
        }
    }
    None
}

/// HTTP-клиент той же дорогой, что и iroh: если сеть БРЕДа идёт мимо VPN,
/// спрашиваем через тот же физический интерфейс, иначе узнаем адрес VPN.
fn echo_client() -> Option<reqwest::Client> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut builder = reqwest::Client::builder()
        .timeout(ECHO_TIMEOUT)
        .user_agent(concat!("BRED/", env!("CARGO_PKG_VERSION")));
    if let Some(ip) = bred_netbind::current().and_then(super::bypass::ipv4_of) {
        builder = builder.local_address(IpAddr::V4(ip));
    }
    builder.build().ok()
}

/// Разобрать ответ сервиса: голый адрес, возможно в кавычках и с переводом строки.
fn parse_ip(body: &str) -> Option<Ipv4Addr> {
    body.trim().trim_matches('"').trim().parse().ok()
}

/// Порт нашего IPv4-сокета.
fn v4_port(bound: &[SocketAddr]) -> Option<u16> {
    bound
        .iter()
        .find(|a| a.is_ipv4() && a.port() != 0)
        .map(|a| a.port())
}

/// Адрес из интернета, а не из домашней сети, CGNAT провайдера или петли.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            let cgnat = a == 100 && (64..128).contains(&b);
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || cgnat
                || a == 0
                || a >= 224)
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            // Только глобальные одноадресные 2000::/3.
            (0x2000..0x4000).contains(&first)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_quoted_and_padded_answers() {
        assert_eq!(
            parse_ip("78.156.235.37"),
            Some(Ipv4Addr::new(78, 156, 235, 37))
        );
        assert_eq!(
            parse_ip("\"78.156.235.37\""),
            Some(Ipv4Addr::new(78, 156, 235, 37))
        );
        assert_eq!(parse_ip(" 5.6.7.8\n"), Some(Ipv4Addr::new(5, 6, 7, 8)));
        assert_eq!(parse_ip("<html>blocked</html>"), None);
        assert_eq!(parse_ip(""), None);
    }

    #[test]
    fn home_cgnat_and_loopback_are_not_public() {
        for ip in [
            "192.168.31.246",
            "10.0.0.1",
            "172.19.0.1",
            "100.64.1.1",
            "127.0.0.1",
            "169.254.1.1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip} — не публичный");
        }
        for ip in ["78.156.235.37", "91.99.237.97", "100.128.0.1"] {
            assert!(is_public(ip.parse().unwrap()), "{ip} — публичный");
        }
        assert!(is_public("2a00:1450::1".parse().unwrap()));
        assert!(!is_public("fe80::1".parse().unwrap()));
        assert!(!is_public("fd00::1".parse().unwrap()));
    }

    #[test]
    fn takes_port_of_ipv4_socket() {
        let bound: Vec<SocketAddr> = vec![
            "[::]:51000".parse().unwrap(),
            "0.0.0.0:62701".parse().unwrap(),
        ];
        assert_eq!(v4_port(&bound), Some(62701));
        assert_eq!(v4_port(&["[::]:51000".parse().unwrap()]), None);
        assert_eq!(v4_port(&["0.0.0.0:0".parse().unwrap()]), None);
    }
}
