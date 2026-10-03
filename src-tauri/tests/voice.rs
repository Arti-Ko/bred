//! Голос от микрофона одного узла до динамика другого — целиком, по-настоящему.
//!
//! Две жалобы, ради которых написан этот файл. Первая: «зашёл в комнату, где
//! человек сидит полчаса, — не соединяет, помогает только перезайти». Вторая:
//! звук стал рваться. Поэтому здесь проверяется не формат кадра, а то, что
//! слышит человек: дошёл ли звук и вернулся ли он после того, как собеседник
//! перезапустил приложение.

use std::time::Duration;

use bred_lib::{app::App, net::Track};

fn workspace(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bred-voice-{}-{tag}-{}",
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
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("не дождались: {what}");
}

/// Полсекунды «речи»: тон с огибающей, чтобы кодек не схитрил на чистом синусе.
fn speech(ms: usize) -> Vec<f32> {
    let samples = 48 * ms;
    (0..samples)
        .map(|i| {
            let t = i as f32 / 48_000.0;
            0.3 * (0.5 + 0.5 * (t * 4.0 * std::f32::consts::TAU).sin())
                * (t * 220.0 * std::f32::consts::TAU).sin()
        })
        .collect()
}

/// Сколько громкого звука доехало до сведённого выхода.
fn loud_frames(rx: &mut tokio::sync::mpsc::Receiver<Vec<u8>>) -> usize {
    let mut loud = 0;
    while let Ok(frame) = rx.try_recv() {
        let samples: Vec<i16> = frame[bred_lib::audio::OUT_HEADER..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| i16::from_le_bytes(*p))
            .collect();
        let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        if peak > 1000 {
            loud += 1;
        }
    }
    loud
}

/// Говорить в звонок блоками по двадцать миллисекунд, как это делает вебвью.
async fn talk(app: &App, ms: usize) {
    for block in speech(ms).chunks(960) {
        app.audio
            .send_pcm(Track::Audio, block)
            .expect("звук ушёл в кодировщик");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn voice_reaches_the_other_side_and_survives_a_restart() {
    let dir_a = workspace("a");
    let dir_b = workspace("b");

    let (alice, _na) = App::start(&dir_a.join("bred.sqlite"))
        .await
        .expect("узел А");
    let (boris, _nb) = App::start(&dir_b.join("bred.sqlite"))
        .await
        .expect("узел Б");

    let space = alice.create_space("Голос").await.expect("пространство");
    until(
        "адрес узла появится",
        Duration::from_secs(20),
        || !alice.net.addr_now().is_empty(),
    )
    .await;
    let room = alice
        .create_channel(space, "комната", "голос", true)
        .await
        .expect("голосовой канал");
    let invite = alice.invite(space).await.expect("приглашение");
    boris.join_space(&invite).await.expect("вход по ссылке");
    until(
        "канал доедет до новичка",
        Duration::from_secs(15),
        || {
            boris
                .channels(space)
                .map(|rows| rows.iter().any(|c| c.id == room))
                .unwrap_or(false)
        },
    )
    .await;

    alice.join_call(space, room).await.expect("А в звонке");
    boris.join_call(space, room).await.expect("Б в звонке");
    until(
        "медиа-соединение поднимется",
        Duration::from_secs(15),
        || {
            boris.net.media().connected().contains(&alice.me())
                && alice.net.media().connected().contains(&boris.me())
        },
    )
    .await;

    // ── звук доходит ───────────────────────────────────────────────────────
    let (tx, mut heard) = tokio::sync::mpsc::channel(512);
    boris.audio.set_sink(tx);
    talk(&alice, 1500).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let loud = loud_frames(&mut heard);
    assert!(
        loud > 40,
        "из семидесяти пяти кадров речи у собеседника прозвучало {loud}"
    );

    // ── собеседник перезапустил приложение ──────────────────────────────────
    // А всё это время сидит в комнате и никуда не выходит — ровно та жалоба.
    let boris_id = boris.me();
    boris.net.shutdown().await;
    drop(boris);
    tokio::time::sleep(Duration::from_secs(1)).await;

    let (boris, _nb) = App::start(&dir_b.join("bred.sqlite"))
        .await
        .expect("Б снова запущен");
    assert_eq!(boris.me(), boris_id, "тот же человек, тот же ключ");
    boris
        .join_call(space, room)
        .await
        .expect("Б снова в звонке");
    until(
        "А и вернувшийся Б снова слышат друг друга — без перезахода А",
        Duration::from_secs(25),
        || {
            boris.net.media().connected().contains(&alice.me())
                && alice.net.media().connected().contains(&boris.me())
        },
    )
    .await;

    let (tx, mut heard) = tokio::sync::mpsc::channel(512);
    boris.audio.set_sink(tx);
    talk(&alice, 1500).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let loud = loud_frames(&mut heard);
    assert!(
        loud > 40,
        "после перезапуска прозвучало {loud} из семидесяти пяти"
    );

    alice.net.shutdown().await;
    boris.net.shutdown().await;
    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}

/// Позвать друга в комнату: у него звонит звонок с именем позвавшего, а
/// отбой — когда позвавший ушёл — этот звонок гасит.
#[tokio::test(flavor = "multi_thread")]
async fn ring_reaches_a_friend_and_is_called_off() {
    use bred_lib::net::Notice;

    let dir_a = workspace("ring-a");
    let dir_b = workspace("ring-b");
    let (alice, _na) = App::start(&dir_a.join("bred.sqlite"))
        .await
        .expect("узел А");
    let (boris, mut nb) = App::start(&dir_b.join("bred.sqlite"))
        .await
        .expect("узел Б");
    alice.set_nick("Алиса").await.expect("имя");

    let space = alice.create_space("Зов").await.expect("пространство");
    until(
        "адрес узла появится",
        Duration::from_secs(20),
        || !alice.net.addr_now().is_empty(),
    )
    .await;
    let room = alice
        .create_channel(space, "комната", "голос", true)
        .await
        .expect("голосовой канал");
    let invite = alice.invite(space).await.expect("приглашение");
    boris.join_space(&invite).await.expect("вход по ссылке");
    until(
        "Б увидит А в сети",
        Duration::from_secs(15),
        || {
            boris
                .members(space)
                .map(|rows| rows.iter().any(|m| m.id == alice.me() && m.online))
                .unwrap_or(false)
        },
    )
    .await;

    alice.join_call(space, room).await.expect("А в комнате");
    // Присутствие с комнатой должно доехать до Б раньше зова — иначе зов
    // отбросят как пришедший «снаружи». Человек тоже зовёт не мгновенно.
    until(
        "Б увидит А в комнате",
        Duration::from_secs(10),
        || {
            boris
                .voice_map(space)
                .iter()
                .any(|(who, _)| *who == alice.me())
        },
    )
    .await;
    let called = alice.ring(Vec::new()).await.expect("зов ушёл");
    assert_eq!(called, 1, "зов доставлен единственному позванному в сети");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut rang = None;
    while rang.is_none() {
        let notice = tokio::time::timeout_at(deadline, nb.recv())
            .await
            .expect("звонок так и не позвонил")
            .expect("канал уведомлений открыт");
        if let Notice::Ring {
            channel,
            from,
            nick,
            id,
            ..
        } = notice
        {
            assert_eq!(channel, room);
            assert_eq!(from, alice.me());
            assert_eq!(nick, "Алиса", "видно, кто зовёт");
            rang = Some(id);
        }
    }

    // Один зов — один звонок.
    let quiet_until = tokio::time::Instant::now() + Duration::from_secs(3);
    while let Ok(Some(notice)) = tokio::time::timeout_at(quiet_until, nb.recv()).await {
        assert!(
            !matches!(notice, Notice::Ring { .. }),
            "один зов не должен звонить дважды"
        );
    }

    // Второй зов сразу следом — частый трезвон от одного человека отсекается.
    alice.ring(Vec::new()).await.expect("второй зов ушёл");
    let quiet_until = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut cancelled_first = false;
    while let Ok(Some(notice)) = tokio::time::timeout_at(quiet_until, nb.recv()).await {
        match notice {
            Notice::Ring { .. } => panic!("второй зов через секунду не должен звонить"),
            // А первый при этом погашен: звонящий позвал заново.
            Notice::RingCancel { id, .. } if Some(id) == rang => cancelled_first = true,
            _ => {}
        }
    }
    assert!(cancelled_first, "прежний зов гасится, когда зовут заново");

    alice.leave_call().await.expect("А ушла");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let notice = tokio::time::timeout_at(deadline, nb.recv())
            .await
            .expect("отбой так и не пришёл")
            .expect("канал уведомлений открыт");
        // Отбой второго зова: тот так и не зазвонил, но отменён должен быть.
        if let Notice::RingCancel { id, .. } = notice {
            assert_ne!(Some(id), rang, "это отбой второго зова, первый уже погашен");
            break;
        }
    }

    alice.net.shutdown().await;
    boris.net.shutdown().await;
    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}
