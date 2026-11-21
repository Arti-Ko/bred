//! Мимо VPN: внешний адрес узла — адрес провайдера, а не VPN-сервера.
//!
//! Проверяется только на машине с включённым VPN, поэтому вручную:
//! `cargo test --test bypass -- --ignored --nocapture` — и сравнить внешний
//! адрес с выводом `curl ifconfig.me` при выключенном VPN.

use std::time::Duration;

use iroh::{endpoint::presets, Endpoint};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "нужен интернет и, для смысла, включённый VPN"]
async fn endpoint_goes_around_the_vpn() {
    let interface = bred_lib::net::bypass::engage().await;
    println!("интерфейс: {interface:?}");
    assert!(
        interface.is_some(),
        "физический интерфейс с интернетом не найден"
    );

    let endpoint = Endpoint::builder(presets::N0).bind().await.expect("узел");
    tokio::time::timeout(Duration::from_secs(30), endpoint.online())
        .await
        .expect("ретранслятор не выбран за 30 секунд");
    tokio::time::sleep(Duration::from_secs(3)).await;
    let addrs: Vec<String> = endpoint
        .addr()
        .addrs
        .iter()
        .map(|a| format!("{a:?}"))
        .collect();
    println!("адреса: {}", addrs.join(", "));
    endpoint.close().await;
}
