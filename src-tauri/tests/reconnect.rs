//! Встреча после перезапуска — без локальной сети и без новой ссылки.
//!
//! Ссылка-приглашение срабатывает один раз: она несёт адрес пригласившего и
//! нужна ровно для первого знакомства. Дальше узлы обязаны находить друг друга
//! сами. Именно этого не происходило: рой gossip с пустым списком точек входа
//! никого не набирает, и после закрытия приложения каждый оказывался один — в
//! пространстве и в звонке.
//!
//! Локальный маячок здесь выключен намеренно: в одной квартире он подменяет
//! собой интернет-путь, и дыра в этом пути остаётся невидимой.

use std::time::Duration;

use bred_lib::app::App;

fn workspace(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bred-reconnect-{}-{tag}", std::process::id()));
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
async fn peers_find_each_other_after_restart() {
    unsafe { std::env::set_var("BRED_NO_LAN", "1") };

    let dir_a = workspace("a");
    let dir_b = workspace("b");
    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
    std::fs::create_dir_all(&dir_a).ok();
    std::fs::create_dir_all(&dir_b).ok();

    let db_a = dir_a.join("bred.sqlite");
    let db_b = dir_b.join("bred.sqlite");

    // Первое знакомство — по ссылке, как и задумано.
    let space = {
        let (alice, _na) = App::start(&db_a).await.expect("узел А");
        let (boris, _nb) = App::start(&db_b).await.expect("узел Б");

        let space = alice
            .create_space("Три города")
            .await
            .expect("пространство");
        let invite = alice.invite(space).await.expect("приглашение");
        boris.join_space(&invite).await.expect("вход по ссылке");

        let channel = alice.channels(space).expect("каналы")[0].id;
        alice
            .send_message(space, channel, "первое", None, None, vec![])
            .await
            .expect("отправка");

        until(
            "знакомство состоится",
            Duration::from_secs(60),
            || {
                boris
                    .messages(channel, None)
                    .map(|m| !m.is_empty())
                    .unwrap_or(false)
            },
        )
        .await;

        space
    };

    // Оба закрыли приложение. Ссылки больше нет, справочник в памяти пуст.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let (alice, _na) = App::start(&db_a).await.expect("узел А снова");
    let (boris, _nb) = App::start(&db_b).await.expect("узел Б снова");

    let channel = alice.channels(space).expect("каналы")[0].id;
    alice
        .send_message(space, channel, "после перезапуска", None, None, vec![])
        .await
        .expect("отправка");

    until(
        "встретятся сами",
        Duration::from_secs(90),
        || {
            boris
                .messages(channel, None)
                .map(|m| m.iter().any(|row| row.body == "после перезапуска"))
                .unwrap_or(false)
        },
    )
    .await;

    std::fs::remove_dir_all(&dir_a).ok();
    std::fs::remove_dir_all(&dir_b).ok();
}
