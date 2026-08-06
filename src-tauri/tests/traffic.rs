//! Пропущенное сообщение обязано найтись само.
//!
//! Событие уезжает в рой ровно один раз. Присутствие и «печатает» — наоборот,
//! повторяются каждые десять секунд, поэтому потерянный удар сердца сам собой
//! заменяется следующим. Отсюда и вид поломки: человек видит «в сети», видит
//! «печатает» и не видит сообщений. Разница не в транспорте — он один и тот же,
//! — а в том, что у эфемерного есть повтор, а у события его нет.
//!
//! Второй шанс у события один: сверка истории с соседом. Но заводится она
//! только на появлении нового соседа. Пока соседи на месте — а они на месте, их
//! видно в настройках, — сверять историю некому. Лента стоит до перезапуска.

use std::time::Duration;

use bred_lib::{
    app::App,
    domain::{now_ms, Event, EventKind, SignedEvent},
};

fn workspace(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bred-traffic-{}-{tag}-{}",
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

/// Сколько ждём, пока пропущенное само доедет.
///
/// Сверка истории должна случаться по часам, а не по случаю. Минуты на это
/// более чем достаточно; «никогда» в минуту не укладывается.
const PATIENCE: Duration = Duration::from_secs(60);

#[tokio::test(flavor = "multi_thread")]
async fn missed_message_arrives_without_restart() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    // Гасим локальный маячок. Он зовёт сверку истории каждые пять секунд и на
    // одной машине подобрал бы потерянное сам — проверка бы прошла, а дыра
    // осталась. Именно так она и дожила до пользователя: на одном вайфае
    // маячок её закрывал, на разных вайфаях закрывать было нечем.
    unsafe { std::env::set_var("BRED_NO_LAN", "1") };

    let dir_a = workspace("a");
    let dir_b = workspace("b");

    let (alice, _na) = App::start(&dir_a.join("bred.sqlite"))
        .await
        .expect("узел А");
    let (boris, _nb) = App::start(&dir_b.join("bred.sqlite"))
        .await
        .expect("узел Б");

    let space = alice.create_space("Работа").await.expect("пространство");
    until(
        "адрес узла появится",
        Duration::from_secs(20),
        || !alice.net.addr_now().is_empty(),
    )
    .await;

    let invite = alice.invite(space).await.expect("приглашение");
    boris.join_space(&invite).await.expect("вход Бориса");

    let channel = alice.channels(space).expect("каналы")[0].id;

    alice
        .send_message(space, channel, "первое", None, None, vec![])
        .await
        .expect("отправка");
    until(
        "Борис увидит первое",
        Duration::from_secs(60),
        || {
            boris
                .messages(channel, None)
                .map(|m| m.iter().any(|r| r.body == "первое"))
                .unwrap_or(false)
        },
    )
    .await;

    // Даём рою устояться. Пока он складывается, соседи появляются и пропадают,
    // и каждое такое появление тянет за собой сверку истории — она бы и подобрала
    // потерянное, но по случайности, а не потому, что так задумано. Человека же
    // застаёт установившийся рой: соседи давно на месте, ничего не меняется.
    tokio::time::sleep(Duration::from_secs(20)).await;

    // Сообщение теряется по дороге. Кладём его Алисе в лог, минуя рассылку:
    // так выглядит потерянный пакет, моргнувший вайфай или заснувший на секунду
    // ноутбук. Важно, что рой при этом цел — подписки на месте, соседи никуда
    // не делись, присутствие и «печатает» ходят как ни в чём не бывало. Ровно
    // та картина, которую человек видит у себя в настройках: «соседей на связи
    // 2», статус живой, сообщений нет.
    let lost = {
        let author = alice.me();
        let event = Event {
            space,
            author,
            seq: alice.store.next_seq(space, author).expect("номер события"),
            lamport: alice.ctx.clock.lock().tick(),
            ts: now_ms(),
            kind: EventKind::Message {
                channel,
                body: "второе".into(),
                reply_to: None,
                thread: None,
                attachments: Vec::new(),
            },
        };
        let sig = alice.ctx.identity.sign(&event.canonical_bytes());
        SignedEvent { event, sig }
    };
    alice.store.apply(&lost).expect("событие легло в лог Алисы");
    tokio::time::sleep(Duration::from_secs(3)).await;

    assert!(
        !boris
            .messages(channel, None)
            .expect("лента Бориса")
            .iter()
            .any(|m| m.body == "второе"),
        "подготовка: сообщение должно быть именно пропущено"
    );

    // Вот проверка. Никто больше ничего не пишет, никто не перезапускается,
    // новых соседей не появляется — пропущенное обязано доехать само.
    until(
        "пропущенное сообщение доедет само",
        PATIENCE,
        || {
            boris
                .messages(channel, None)
                .map(|m| m.iter().any(|r| r.body == "второе"))
                .unwrap_or(false)
        },
    )
    .await;

    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}
