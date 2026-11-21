//! Файл приезжает сам: получателю не нужно его открывать.
//!
//! Раньше байты просил только интерфейс, когда человек долистывал до картинки.
//! Если автор к тому времени уходил, картинка не приходила никогда, а скриншот
//! больше двух мегабайт вовсе висел плашкой «скачать». Здесь проверяется, что
//! ядро тянет файлы само, сразу как узнало о сообщении, — и несколько подряд,
//! по одному переиспользуемому соединению.

use std::time::Duration;

use bred_lib::app::App;

fn workspace(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bred-eager-{}-{tag}-{}",
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
async fn files_arrive_without_being_asked_for() {
    let dir_a = workspace("a");
    let dir_b = workspace("b");
    let (alice, _na) = App::start(&dir_a.join("bred.sqlite")).await.expect("узел А");
    let (boris, _nb) = App::start(&dir_b.join("bred.sqlite")).await.expect("узел Б");

    let space = alice.create_space("Проверка").await.expect("пространство");
    until("адрес узла появится", Duration::from_secs(20), || {
        !alice.net.addr_now().is_empty()
    })
    .await;
    let invite = alice.invite(space).await.expect("приглашение");
    boris.join_space(&invite).await.expect("вход по ссылке");
    let channel = alice.channels(space).expect("каналы")[0].id;

    // Скриншот с ретины — несколько мегабайт: ровно то, что раньше приходило
    // плашкой. И два маленьких следом — по тому же соединению.
    let mut sent = Vec::new();
    for (name, size) in [("снимок.png", 6_000_000usize), ("a.png", 40_000), ("b.png", 50_000)] {
        let source = dir_a.join(name);
        let payload: Vec<u8> = (0..size).map(|i| (i * 7 % 251) as u8).collect();
        std::fs::write(&source, &payload).expect("исходный файл");
        let attachment = alice.attach(&source).await.expect("вложение");
        alice
            .send_message(space, channel, "лови", None, None, vec![attachment.clone()])
            .await
            .expect("отправка");
        sent.push((attachment.hash, payload));
    }

    // Никаких `download`: только ждём, пока файлы окажутся у Бориса.
    until("файлы приедут сами", Duration::from_secs(60), || {
        sent.iter().all(|(hash, _)| {
            boris
                .store
                .blob_path(*hash)
                .ok()
                .flatten()
                .is_some_and(|p| p.exists())
        })
    })
    .await;

    for (hash, payload) in &sent {
        let path = boris.store.blob_path(*hash).unwrap().unwrap();
        assert_eq!(&std::fs::read(path).unwrap(), payload, "байты обязаны совпасть");
    }

    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}
