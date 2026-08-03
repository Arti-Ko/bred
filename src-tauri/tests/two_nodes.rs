//! Два настоящих узла: приглашение, переписка, вложение.
//!
//! Ровно тот тест, которого не хватало. Всё остальное проверялось на трубах и
//! на одном процессе, поэтому дыра в том, как узел дозванивается до другого
//! узла, жила незамеченной несколько релизов подряд.

use std::time::Duration;

use bred_lib::app::App;

/// Отдельный каталог данных на каждый узел: иначе они подхватят один ключ
/// и окажутся одним и тем же участником.
fn workspace(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bred-two-{}-{tag}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("каталог");
    dir
}

/// Ждём условие, опрашивая его: сеть не отвечает мгновенно, а спать наугад —
/// значит либо тормозить тест, либо ловить мигание.
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
async fn attachment_reaches_the_other_side() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
    let dir_a = workspace("a");
    let dir_b = workspace("b");

    let (alice, _na) = App::start(&dir_a.join("bred.sqlite"))
        .await
        .expect("узел А");
    let (boris, _nb) = App::start(&dir_b.join("bred.sqlite"))
        .await
        .expect("узел Б");

    // Алиса заводит пространство и зовёт Бориса ссылкой.
    let space = alice.create_space("Проверка").await.expect("пространство");
    until(
        "адрес узла появится",
        Duration::from_secs(20),
        || !alice.net.addr_now().is_empty(),
    )
    .await;

    let invite = alice.invite(space).expect("приглашение");
    boris.join_space(&invite).await.expect("вход по ссылке");

    // Файл, который поедет по сети.
    let source = dir_a.join("картинка.png");
    let payload: Vec<u8> = (0..60_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&source, &payload).expect("исходный файл");

    let attachment = alice.attach(&source).await.expect("подготовка вложения");
    let channel = alice.channels(space).expect("каналы")[0].id;
    alice
        .send_message(space, channel, "лови", None, None, vec![attachment.clone()])
        .await
        .expect("отправка");

    // Сообщение должно доехать до Бориса само.
    until(
        "сообщение доедет",
        Duration::from_secs(40),
        || {
            boris
                .channels(space)
                .map(|channels| {
                    channels.iter().any(|c| {
                        boris
                            .messages(c.id, None)
                            .map(|m| !m.is_empty())
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        },
    )
    .await;

    // А вот и главное: Борис забирает байты у Алисы.
    let saved = boris
        .download(space, attachment.hash)
        .await
        .expect("скачивание вложения");

    let received = std::fs::read(&saved).expect("полученный файл");
    assert_eq!(received, payload, "байты обязаны совпасть один в один");

    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}

/// Аватар и стикер едут не сообщением, а событиями профиля и набора эмодзи.
/// Путь до байтов у них общий с вложениями, но узнаёт о них получатель иначе —
/// поэтому проверяем отдельно: именно на это жаловались («видит только автор»).
#[tokio::test(flavor = "multi_thread")]
async fn avatar_and_sticker_reach_the_other_side() {
    let dir_a = workspace("av-a");
    let dir_b = workspace("av-b");

    let (alice, _na) = App::start(&dir_a.join("bred.sqlite"))
        .await
        .expect("узел А");
    let (boris, _nb) = App::start(&dir_b.join("bred.sqlite"))
        .await
        .expect("узел Б");

    let space = alice.create_space("Картинки").await.expect("пространство");
    until(
        "адрес узла появится",
        Duration::from_secs(20),
        || !alice.net.addr_now().is_empty(),
    )
    .await;

    // Крошечный, но настоящий PNG: важно, что байты сойдутся один в один.
    let png: Vec<u8> = {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        bytes.extend((0..4096u32).map(|i| (i % 253) as u8));
        bytes
    };
    let face = dir_a.join("лицо.png");
    let sticker = dir_a.join("кот.png");
    std::fs::write(&face, &png).expect("файл аватара");
    std::fs::write(&sticker, &png).expect("файл стикера");

    alice.set_avatar(&face).await.expect("аватар");
    alice
        .add_emoji(space, "кот", &sticker, true)
        .await
        .expect("стикер");

    let invite = alice.invite(space).expect("приглашение");
    boris.join_space(&invite).await.expect("вход по ссылке");

    // Борис должен увидеть и профиль Алисы, и её стикер.
    until(
        "аватар и стикер доедут",
        Duration::from_secs(40),
        || {
            let has_avatar = boris
                .members(space)
                .map(|m| m.iter().any(|p| p.avatar.is_some()))
                .unwrap_or(false);
            let has_sticker = boris.emojis(space).map(|e| !e.is_empty()).unwrap_or(false);
            has_avatar && has_sticker
        },
    )
    .await;

    let avatar_hash = boris
        .members(space)
        .expect("участники")
        .into_iter()
        .find_map(|p| p.avatar)
        .expect("хеш аватара");
    let sticker_hash = boris.emojis(space).expect("эмодзи")[0].hash;

    for (what, hash) in [("аватар", avatar_hash), ("стикер", sticker_hash)] {
        let saved = boris
            .download(space, hash)
            .await
            .unwrap_or_else(|e| panic!("{what} не скачался: {e:#}"));
        let bytes = std::fs::read(&saved).expect("файл на диске");
        assert_eq!(bytes, png, "{what}: байты обязаны совпасть");
    }

    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}
