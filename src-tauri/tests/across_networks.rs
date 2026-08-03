//! Разные сети: прямого пути нет, всё идёт через ретранслятор.
//!
//! Тот самый случай, который не ловился ничем: на одной машине прямое
//! соединение есть всегда, поэтому поломка интернет-пути оставалась невидимой,
//! а у людей в разных городах не работало ничего. Здесь прямые пути отключены
//! (`BRED_RELAY_ONLY`) и локальный маячок выключен (`BRED_NO_LAN`) — остаётся
//! ровно то, чем пользуются собеседники из разных сетей.

use std::time::Duration;

use bred_lib::app::App;

fn workspace(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bred-across-{}-{tag}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("каталог");
    dir
}

async fn until(what: &str, limit: Duration, mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    panic!("не дождались: {what}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "нужен интернет"]
async fn talk_and_share_across_networks() {
    unsafe {
        std::env::set_var("BRED_NO_LAN", "1");
        std::env::set_var("BRED_RELAY_ONLY", "1");
    }

    let dir_a = workspace("a");
    let dir_b = workspace("b");
    let db_a = dir_a.join("bred.sqlite");
    let db_b = dir_b.join("bred.sqlite");

    let space;
    let channel;
    {
        let (alice, _na) = App::start(&db_a).await.expect("узел А");
        let (boris, _nb) = App::start(&db_b).await.expect("узел Б");

        space = alice
            .create_space("Омск и Москва")
            .await
            .expect("пространство");
        let invite = alice.invite(space).await.expect("приглашение");
        boris.join_space(&invite).await.expect("вход по ссылке");

        channel = alice.channels(space).expect("каналы")[0].id;

        // Переписка через ретранслятор.
        alice
            .send_message(space, channel, "слышно меня?", None, None, vec![])
            .await
            .expect("отправка");
        until(
            "сообщение дойдёт",
            Duration::from_secs(60),
            || {
                boris
                    .messages(channel, None)
                    .map(|m| !m.is_empty())
                    .unwrap_or(false)
            },
        )
        .await;

        // И файл — по тому же пути.
        let source = dir_a.join("снимок.png");
        let payload: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&source, &payload).expect("исходный файл");
        let file = alice.attach(&source).await.expect("вложение");
        alice
            .send_message(space, channel, "лови", None, None, vec![file.clone()])
            .await
            .expect("отправка файла");

        until(
            "файл появится в ленте",
            Duration::from_secs(60),
            || {
                boris
                    .messages(channel, None)
                    .map(|m| m.iter().any(|row| !row.attachments.is_empty()))
                    .unwrap_or(false)
            },
        )
        .await;

        let saved = boris.download(space, file.hash).await.expect("скачивание");
        let got = std::fs::read(&saved).expect("файл на диске");
        assert_eq!(got, payload, "байты обязаны совпасть");
    }

    // Оба закрыли приложение — как вечером после работы.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let (alice, _na) = App::start(&db_a).await.expect("узел А снова");
    let (boris, _nb) = App::start(&db_b).await.expect("узел Б снова");

    alice
        .send_message(space, channel, "на следующий день", None, None, vec![])
        .await
        .expect("отправка");

    until(
        "встретятся сами, без новой ссылки",
        Duration::from_secs(120),
        || {
            boris
                .messages(channel, None)
                .map(|m| m.iter().any(|row| row.body == "на следующий день"))
                .unwrap_or(false)
        },
    )
    .await;

    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}
