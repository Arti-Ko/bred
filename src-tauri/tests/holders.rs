//! Файл переживает автора.
//!
//! Байты вложения есть у каждого, кто их однажды скачал. Раньше спрашивали
//! только автора события — и стоило ему закрыть ноутбук, как файл становился
//! недоступен всем остальным, хотя лежал у соседа. Со стороны это выглядело
//! ровно как «отправка файлов не работает».

use std::time::Duration;

use bred_lib::app::App;

fn workspace(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bred-hold-{}-{tag}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("каталог");
    dir
}

async fn until(what: &str, limit: Duration, mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("не дождались: {what}");
}

#[tokio::test(flavor = "multi_thread")]
async fn file_survives_the_author_leaving() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let dir_a = workspace("a");
    let dir_b = workspace("b");
    let dir_v = workspace("v");

    let (alice, _na) = App::start(&dir_a.join("bred.sqlite"))
        .await
        .expect("узел А");
    let (boris, _nb) = App::start(&dir_b.join("bred.sqlite"))
        .await
        .expect("узел Б");
    let (vera, _nv) = App::start(&dir_v.join("bred.sqlite"))
        .await
        .expect("узел В");

    let space = alice.create_space("Архив").await.expect("пространство");
    until(
        "адрес узла появится",
        Duration::from_secs(20),
        || !alice.net.addr_now().is_empty(),
    )
    .await;

    let invite = alice.invite(space).await.expect("приглашение");
    boris.join_space(&invite).await.expect("вход Бориса");
    vera.join_space(&invite).await.expect("вход Веры");

    let source = dir_a.join("документ.bin");
    let payload: Vec<u8> = (0..90_000u32).map(|i| (i % 249) as u8).collect();
    std::fs::write(&source, &payload).expect("исходный файл");

    let attachment = alice.attach(&source).await.expect("подготовка вложения");
    let channel = alice.channels(space).expect("каналы")[0].id;
    alice
        .send_message(
            space,
            channel,
            "держите",
            None,
            None,
            vec![attachment.clone()],
        )
        .await
        .expect("отправка");

    // Сообщение должно доехать до обоих: без него они не узнают о вложении.
    for (who, node) in [("Борис", &boris), ("Вера", &vera)] {
        until(
            &format!("{who} увидит сообщение"),
            Duration::from_secs(60),
            || {
                node.channels(space)
                    .map(|channels| {
                        channels.iter().any(|c| {
                            node.messages(c.id, None)
                                .map(|m| !m.is_empty())
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            },
        )
        .await;
    }

    // Борис забирает байты, пока автор ещё на связи.
    let saved = boris
        .download(space, attachment.hash)
        .await
        .expect("Борис скачал у автора");
    assert_eq!(
        std::fs::read(&saved).expect("файл Бориса"),
        payload,
        "у Бориса должны быть те самые байты"
    );

    // Вера должна знать Бориса как живого соседа: иначе спрашивать будет некого
    // не из-за ошибки, а просто потому, что рой ещё не сложился.
    until(
        "Вера увидит Бориса в сети",
        Duration::from_secs(60),
        || {
            vera.members(space)
                .map(|rows| rows.iter().any(|m| m.id == boris.me() && m.online))
                .unwrap_or(false)
        },
    )
    .await;

    // А теперь автор уходит совсем — с закрытием сокета, а не просто из виду.
    alice.net.shutdown().await;
    drop(alice);
    drop(_na);
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Вера файл не качала, автора нет — единственный источник это Борис.
    let saved = vera
        .download(space, attachment.hash)
        .await
        .expect("Вера должна забрать файл у Бориса, а не упереться в ушедшего автора");
    assert_eq!(
        std::fs::read(&saved).expect("файл Веры"),
        payload,
        "байты обязаны совпасть один в один"
    );

    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
    std::fs::remove_dir_all(&dir_v).ok();
}
