//! Разрешение имён, которое переживает кривую сеть.
//!
//! Библиотека по умолчанию спрашивает имена сама, голым UDP на 53-й порт мимо
//! системного резолвера. В нормальной сети это правильно и быстро, а в той, где
//! живут наши люди, — первое, что отваливается: туннели и провайдерские фильтры
//! глотают такие запросы молча. Симптом при этом выглядит как поломка всего
//! приложения: не находится ретранслятор, не публикуется свой адрес, соседи
//! «то появляются, то исчезают».
//!
//! Поэтому здесь два слоя:
//!
//! 1. спрашиваем по HTTPS и по TCP, а не только по UDP — то, что режут чаще
//!    всего, перестаёт быть единственным путём;
//! 2. удачные ответы складываем на диск и, если разрешить имя не вышло вовсе,
//!    отвечаем последним известным адресом. Ретранслятор меняет адрес редко,
//!    а связь без него не поднимается совсем.

use std::{
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
};

use iroh::dns::{
    BoxIter, DnsError, DnsProtocol, DnsResolver, Resolver, TxtRecordData, DNS_TIMEOUT,
};
use n0_future::boxed::BoxFuture;

use crate::store::Store;

/// Публичные резолверы, которых спрашиваем поверх HTTPS и TCP.
///
/// Именно адресами, а не именами: чтобы узнать имя, нужен резолвер — круг
/// замкнулся бы.
const OVER_HTTPS: [&str; 2] = ["1.1.1.1:443", "8.8.8.8:443"];
const OVER_TCP: [&str; 2] = ["1.1.1.1:53", "8.8.8.8:53"];

/// Собрать резолвер: системный плюс запасные пути, и всё это под кешем.
pub fn resolver(store: Arc<Store>) -> DnsResolver {
    let mut builder = DnsResolver::builder();
    // Системная настройка первой: дома и в офисе она и правильная, и быстрая.
    builder = builder.with_system_defaults();
    for address in OVER_HTTPS {
        if let Ok(addr) = address.parse::<SocketAddr>() {
            builder = builder.with_nameserver(addr, DnsProtocol::Https);
        }
    }
    for address in OVER_TCP {
        if let Ok(addr) = address.parse::<SocketAddr>() {
            builder = builder.with_nameserver(addr, DnsProtocol::Tcp);
        }
    }

    DnsResolver::custom(Steady {
        inner: builder.build(),
        store,
    })
}

/// Резолвер, который помнит вчерашний ответ.
struct Steady {
    inner: DnsResolver,
    store: Arc<Store>,
}

// Печатаем руками: хранилище выводить незачем, а трейт требует `Debug`.
impl std::fmt::Debug for Steady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Steady")
    }
}

impl Resolver for Steady {
    fn lookup_ipv4(&self, host: String) -> BoxFuture<Result<BoxIter<Ipv4Addr>, DnsError>> {
        let inner = self.inner.clone();
        let store = self.store.clone();
        Box::pin(async move {
            match inner.lookup_ipv4(host.clone(), DNS_TIMEOUT).await {
                Ok(found) => {
                    let list: Vec<Ipv4Addr> = found
                        .filter_map(|addr| match addr {
                            std::net::IpAddr::V4(v4) => Some(v4),
                            std::net::IpAddr::V6(_) => None,
                        })
                        .collect();
                    let _ = store.remember_host(
                        &host,
                        4,
                        &list.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                    );
                    Ok(Box::new(list.into_iter()) as BoxIter<Ipv4Addr>)
                }
                Err(err) => {
                    let remembered: Vec<Ipv4Addr> = store
                        .known_host(&host, 4)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|text| text.parse().ok())
                        .collect();
                    if remembered.is_empty() {
                        return Err(err);
                    }
                    tracing::warn!(%host, %err, "имя не разрешилось, берём последний известный адрес");
                    Ok(Box::new(remembered.into_iter()) as BoxIter<Ipv4Addr>)
                }
            }
        })
    }

    fn lookup_ipv6(&self, host: String) -> BoxFuture<Result<BoxIter<Ipv6Addr>, DnsError>> {
        let inner = self.inner.clone();
        let store = self.store.clone();
        Box::pin(async move {
            match inner.lookup_ipv6(host.clone(), DNS_TIMEOUT).await {
                Ok(found) => {
                    let list: Vec<Ipv6Addr> = found
                        .filter_map(|addr| match addr {
                            std::net::IpAddr::V6(v6) => Some(v6),
                            std::net::IpAddr::V4(_) => None,
                        })
                        .collect();
                    let _ = store.remember_host(
                        &host,
                        6,
                        &list.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                    );
                    Ok(Box::new(list.into_iter()) as BoxIter<Ipv6Addr>)
                }
                Err(err) => {
                    let remembered: Vec<Ipv6Addr> = store
                        .known_host(&host, 6)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|text| text.parse().ok())
                        .collect();
                    if remembered.is_empty() {
                        return Err(err);
                    }
                    Ok(Box::new(remembered.into_iter()) as BoxIter<Ipv6Addr>)
                }
            }
        })
    }

    /// Записи TXT не кешируем: по ним ищут адреса живых людей, и вчерашний
    /// ответ здесь хуже честного «не нашёл».
    fn lookup_txt(&self, host: String) -> BoxFuture<Result<BoxIter<TxtRecordData>, DnsError>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            // Собираем сразу: итератор держит ссылку на резолвер, а отдать его
            // наружу надо самостоятельным.
            let found: Vec<TxtRecordData> = inner.lookup_txt(host, DNS_TIMEOUT).await?.collect();
            Ok(Box::new(found.into_iter()) as BoxIter<TxtRecordData>)
        })
    }

    fn clear_cache(&self) {
        self.inner.clear_cache();
    }

    /// Пересобрать после смены сети. Ввода-вывода здесь быть не должно —
    /// только новый экземпляр поверх тех же настроек.
    fn reset(&self) -> Box<dyn Resolver> {
        self.inner.reset();
        Box::new(Steady {
            inner: self.inner.clone(),
            store: self.store.clone(),
        })
    }
}
