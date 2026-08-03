//! Через сколько узел становится доступен из интернета.
//!
//! Ссылка-приглашение и присутствие несут наш адрес. Если снять его слишком
//! рано, там будут только адреса домашней сети — по ним из другого города не
//! дозвониться. Тест меряет, когда адрес становится годным.

use std::time::{Duration, Instant};

use iroh::{endpoint::presets, Endpoint};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "нужен интернет"]
async fn how_long_until_reachable() {
    let endpoint = Endpoint::builder(presets::N0)
        .bind()
        .await
        .expect("узел поднялся");

    let show = |when: &str, addr: iroh::EndpointAddr| {
        let all: Vec<String> = addr.addrs.iter().map(|a| format!("{a:?}")).collect();
        println!("{when}: адресов {} → {}", all.len(), all.join(", "));
    };

    show("сразу после запуска", endpoint.addr());

    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(30), endpoint.online())
        .await
        .expect("ретранслятор не выбран за 30 секунд");
    println!("ретранслятор выбран за {:?}", started.elapsed());

    show("после online()", endpoint.addr());

    // Ещё немного: публичный адрес узнаётся отдельно от ретранслятора.
    tokio::time::sleep(Duration::from_secs(3)).await;
    show("ещё через 3 секунды", endpoint.addr());

    endpoint.close().await;
}
