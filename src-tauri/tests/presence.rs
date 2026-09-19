//! Кто в сети — и как быстро это становится видно.
//!
//! Жалоба, ради которой написан этот файл, звучала так: «сидишь в пространстве
//! один, а у человека приложение уже открыто». Оба узла здесь настоящие, связь
//! настоящая, и проверяется ровно то, что человек видит своими глазами —
//! список участников.

use std::time::Duration;

use bred_lib::app::App;

fn workspace(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bred-presence-{}-{tag}-{}",
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

/// Новичок вошёл по ссылке и молчит. Его всё равно должно быть видно — и не
/// через полминуты, а сразу.
///
/// Раньше о себе напоминали раз в десять секунд и только тем, с кем связь уже
/// была: рой не пересказывает новичку то, что разлетелось до его прихода.
/// Поэтому обе стороны ждали очередного удара сердца, глядя в пустой список.
#[tokio::test(flavor = "multi_thread")]
async fn each_side_sees_the_other_within_seconds() {
    let dir_a = workspace("a");
    let dir_b = workspace("b");

    let (alice, _na) = App::start(&dir_a.join("bred.sqlite")).await.expect("узел А");
    let (boris, _nb) = App::start(&dir_b.join("bred.sqlite")).await.expect("узел Б");

    let space = alice.create_space("Кто здесь").await.expect("пространство");
    until("адрес узла появится", Duration::from_secs(20), || {
        !alice.net.addr_now().is_empty()
    })
    .await;

    let invite = alice.invite(space).await.expect("приглашение");
    boris.join_space(&invite).await.expect("вход по ссылке");

    let sees = |who: &App, whom: bred_lib::domain::Id| {
        who.members(space)
            .map(|rows| rows.iter().any(|r| r.id == whom && r.online))
            .unwrap_or(false)
    };

    until("хозяин увидит новичка", Duration::from_secs(6), || {
        sees(&alice, boris.me())
    })
    .await;
    until("новичок увидит хозяина", Duration::from_secs(6), || {
        sees(&boris, alice.me())
    })
    .await;

    alice.net.shutdown().await;
    boris.net.shutdown().await;
    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}

/// Ушедший не должен висеть «в сети» до скончания века: у присутствия есть
/// срок годности, и отсчитывается он по нашим часам.
#[tokio::test(flavor = "multi_thread")]
async fn someone_who_left_stops_being_online() {
    let dir_a = workspace("gone-a");
    let dir_b = workspace("gone-b");

    let (alice, _na) = App::start(&dir_a.join("bred.sqlite")).await.expect("узел А");
    let (boris, _nb) = App::start(&dir_b.join("bred.sqlite")).await.expect("узел Б");

    let space = alice.create_space("Уходят").await.expect("пространство");
    until("адрес узла появится", Duration::from_secs(20), || {
        !alice.net.addr_now().is_empty()
    })
    .await;
    let invite = alice.invite(space).await.expect("приглашение");
    boris.join_space(&invite).await.expect("вход по ссылке");

    let boris_id = boris.me();
    until("новичок появится", Duration::from_secs(6), || {
        alice
            .members(space)
            .map(|rows| rows.iter().any(|r| r.id == boris_id && r.online))
            .unwrap_or(false)
    })
    .await;

    boris.net.shutdown().await;
    drop(boris);

    until("исчезнет из сети", Duration::from_secs(20), || {
        alice
            .members(space)
            .map(|rows| !rows.iter().any(|r| r.id == boris_id && r.online))
            .unwrap_or(false)
    })
    .await;

    alice.net.shutdown().await;
    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}

/// Двое зашли в один голосовой канал — и увидели друг друга в комнате.
///
/// Комната не имеет отдельного сигнального сервера: кто в канале, видно
/// исключительно по присутствию. Поэтому «зашли в один канал и не соединились»
/// — это ровно та же поломка, что и пустой список участников, и проверять её
/// надо отдельно: цена ошибки здесь выше, разговор просто не состоится.
#[tokio::test(flavor = "multi_thread")]
async fn two_in_one_voice_channel_see_each_other() {
    let dir_a = workspace("call-a");
    let dir_b = workspace("call-b");

    let (alice, _na) = App::start(&dir_a.join("bred.sqlite")).await.expect("узел А");
    let (boris, _nb) = App::start(&dir_b.join("bred.sqlite")).await.expect("узел Б");

    let space = alice.create_space("Созвон").await.expect("пространство");
    until("адрес узла появится", Duration::from_secs(20), || {
        !alice.net.addr_now().is_empty()
    })
    .await;
    let voice = alice
        .create_channel(space, "созвон", "голос", true)
        .await
        .expect("голосовой канал");

    let invite = alice.invite(space).await.expect("приглашение");
    boris.join_space(&invite).await.expect("вход по ссылке");
    until("канал доедет до новичка", Duration::from_secs(10), || {
        boris
            .channels(space)
            .map(|rows| rows.iter().any(|c| c.id == voice))
            .unwrap_or(false)
    })
    .await;

    alice.join_call(space, voice).await.expect("А входит в звонок");
    boris.join_call(space, voice).await.expect("Б входит в звонок");

    until("А увидит Б в комнате", Duration::from_secs(8), || {
        alice.call_participants().contains(&boris.me())
    })
    .await;
    until("Б увидит А в комнате", Duration::from_secs(8), || {
        boris.call_participants().contains(&alice.me())
    })
    .await;

    // И обратно: вышел — пропал из комнаты, а не висит там до перезапуска.
    boris.leave_call().await.expect("Б выходит");
    until("ушедший исчезает из комнаты", Duration::from_secs(8), || {
        !alice.call_participants().contains(&boris.me())
    })
    .await;

    alice.net.shutdown().await;
    boris.net.shutdown().await;
    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}
