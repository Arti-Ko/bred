//! Сокеты БРЕДа мимо VPN.
//!
//! VPN с TUN-интерфейсом (sing-box, WireGuard, v2RayTun) забирает весь трафик
//! себе маршрутом по умолчанию. Для P2P это худшее, что может случиться:
//! пробивка NAT через туннель не работает, а ретранслятор живёт ровно столько,
//! сколько живёт туннель. Обход — тот же, каким пользуется сам VPN для своих
//! соединений: сокет привязывается к физическому интерфейсу флагом ядра, и
//! маршрут по умолчанию его больше не касается.
//!
//! Номер интерфейса один на процесс: его выбирает приложение, а сетевые
//! библиотеки (наши копии `netwatch` и `iroh-relay`) только применяют его к
//! каждому новому сокету. Ноль — обход выключен, сокеты как обычно.

use std::{
    io,
    net::SocketAddr,
    num::NonZeroU32,
    sync::atomic::{AtomicU32, Ordering},
};

static INTERFACE: AtomicU32 = AtomicU32::new(0);

/// Привязывать новые сокеты к интерфейсу с этим номером. `None` — не привязывать.
pub fn set(index: Option<NonZeroU32>) {
    INTERFACE.store(index.map_or(0, NonZeroU32::get), Ordering::Relaxed);
}

/// Номер интерфейса, к которому сейчас привязываются сокеты.
pub fn current() -> Option<NonZeroU32> {
    NonZeroU32::new(INTERFACE.load(Ordering::Relaxed))
}

/// Привязать сокет к выбранному интерфейсу, если обход включён.
///
/// Ошибка здесь не повод отказываться от сокета: без привязки он просто пойдёт
/// обычным маршрутом, как до обхода.
pub fn apply(socket: &socket2::Socket, v6: bool) -> io::Result<()> {
    match current() {
        Some(index) => bind_to(socket, index, v6),
        None => Ok(()),
    }
}

/// Привязать сокет к конкретному интерфейсу — для проверки, есть ли через него
/// интернет, ещё до того, как обход включён.
pub fn bind_to(socket: &socket2::Socket, index: NonZeroU32, v6: bool) -> io::Result<()> {
    platform::bind(socket, index, v6)
}

/// TCP-соединение через выбранный интерфейс — для ретранслятора.
pub async fn connect_tcp(addr: SocketAddr) -> io::Result<tokio::net::TcpStream> {
    connect_tcp_via(addr, current()).await
}

/// TCP-соединение через конкретный интерфейс (или обычным маршрутом).
pub async fn connect_tcp_via(
    addr: SocketAddr,
    index: Option<NonZeroU32>,
) -> io::Result<tokio::net::TcpStream> {
    let Some(index) = index else {
        return tokio::net::TcpStream::connect(addr).await;
    };
    let domain = socket2::Domain::for_address(addr);
    let socket = socket2::Socket::new(domain, socket2::Type::STREAM, Some(socket2::Protocol::TCP))?;
    bind_to(&socket, index, addr.is_ipv6())?;
    socket.set_nonblocking(true)?;
    let std: std::net::TcpStream = socket.into();
    tokio::net::TcpSocket::from_std_stream(std).connect(addr).await
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod platform {
    use super::*;

    pub fn bind(socket: &socket2::Socket, index: NonZeroU32, v6: bool) -> io::Result<()> {
        if v6 {
            socket.bind_device_by_index_v6(Some(index))
        } else {
            socket.bind_device_by_index_v4(Some(index))
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::io::AsRawSocket;

    const IPPROTO_IP: i32 = 0;
    const IPPROTO_IPV6: i32 = 41;
    const IP_UNICAST_IF: i32 = 31;
    const IPV6_UNICAST_IF: i32 = 31;

    #[link(name = "ws2_32")]
    extern "system" {
        fn setsockopt(s: usize, level: i32, optname: i32, optval: *const u8, optlen: i32) -> i32;
        fn WSAGetLastError() -> i32;
    }

    pub fn bind(socket: &socket2::Socket, index: NonZeroU32, v6: bool) -> io::Result<()> {
        // Для IPv4 номер интерфейса передаётся в сетевом порядке байт, для
        // IPv6 — в обычном. Так устроен API Windows, не опечатка.
        let (level, name, value) = if v6 {
            (IPPROTO_IPV6, IPV6_UNICAST_IF, index.get())
        } else {
            (IPPROTO_IP, IP_UNICAST_IF, index.get().to_be())
        };
        // SAFETY: сокет живой (держим ссылку), указатель на `value` действителен
        // на время вызова, длина совпадает с размером u32, как требует API.
        let result = unsafe {
            setsockopt(
                socket.as_raw_socket() as usize,
                level,
                name,
                (&value as *const u32).cast(),
                std::mem::size_of::<u32>() as i32,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            // SAFETY: без аргументов, читает код ошибки текущего потока.
            Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }))
        }
    }
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
mod platform {
    use super::*;

    pub fn bind(socket: &socket2::Socket, index: NonZeroU32, v6: bool) -> io::Result<()> {
        if v6 {
            socket.bind_device_by_index_v6(Some(index))
        } else {
            socket.bind_device_by_index_v4(Some(index))
        }
    }
}
