//! Исключение участника на живых узлах.
//!
//! Мало записать в лог «Веру исключили»: пока ключ прежний, она продолжала бы
//! читать всё новое. Проверяем всю цепочку целиком — исключение, раздачу
//! нового ключа, переход оставшихся в новый рой — и что после этого
//! оставшиеся по-прежнему слышат друг друга, а исключённая отрезана.

use std::time::Duration;

use bred_lib::{app::App, domain::SpaceId, store::Role};

fn workspace(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bred-gov-{}-{tag}-{}",
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

fn heard(node: &App, space: SpaceId, text: &str) -> bool {
    node.channels(space)
        .map(|channels| {
            channels.iter().any(|c| {
                node.messages(c.id, None)
                    .map(|m| m.iter().any(|m| m.body == text))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[tokio::test(flavor = "multi_thread")]
async fn removed_member_is_cut_off_and_the_rest_carry_on() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let (alice, _na) = App::start(&workspace("a").join("bred.sqlite"))
        .await
        .expect("А");
    let (boris, _nb) = App::start(&workspace("b").join("bred.sqlite"))
        .await
        .expect("Б");
    let (vera, _nv) = App::start(&workspace("v").join("bred.sqlite"))
        .await
        .expect("В");

    let space = alice.create_space("Кухня").await.expect("пространство");
    assert_eq!(alice.governance(space).unwrap().role, Role::Owner);
    until("адрес узла", Duration::from_secs(20), || {
        !alice.net.addr_now().is_empty()
    })
    .await;

    let invite = alice.invite(space).await.expect("приглашение");
    assert!(invite.starts_with("bred://invite/"));
    boris.join_space(&invite).await.expect("вход Бориса");
    vera.join_space(&invite).await.expect("вход Веры");

    // Все трое знают друг друга с ключами согласования — иначе раздавать некому.
    until(
        "Алиса знает обоих",
        Duration::from_secs(60),
        || {
            alice
                .members(space)
                .map(|m| m.iter().filter(|m| m.dh.is_some()).count() >= 3)
                .unwrap_or(false)
        },
    )
    .await;
    // Владельца видят и гости: он доказуем, а не назначен на слово.
    until(
        "Борис видит владельца",
        Duration::from_secs(30),
        || {
            boris
                .governance(space)
                .map(|g| g.owner == Some(alice.me()))
                .unwrap_or(false)
        },
    )
    .await;
    assert!(!boris.governance(space).unwrap().can_moderate);

    let channel = alice.channels(space).expect("каналы")[0].id;
    alice
        .send_message(space, channel, "до", None, None, Vec::new())
        .await
        .expect("сообщение");
    until(
        "Вера слышит «до»",
        Duration::from_secs(60),
        || heard(&vera, space, "до"),
    )
    .await;

    // Рядовой участник исключать не может.
    assert!(boris.remove_member(space, vera.me()).await.is_err());

    alice
        .remove_member(space, vera.me())
        .await
        .expect("исключение");
    assert_eq!(alice.governance(space).unwrap().epoch, 1);

    until(
        "Борис перешёл на новый ключ",
        Duration::from_secs(60),
        || {
            boris
                .governance(space)
                .map(|g| g.epoch == 1)
                .unwrap_or(false)
        },
    )
    .await;
    until(
        "у Веры пространство стёрто",
        Duration::from_secs(60),
        || {
            vera.spaces()
                .map(|s| s.iter().all(|s| s.id != space))
                .unwrap_or(false)
        },
    )
    .await;

    // Новые ключи совпали: Алиса и Борис по-прежнему слышат друг друга.
    alice
        .send_message(space, channel, "после", None, None, Vec::new())
        .await
        .expect("сообщение после");
    until(
        "Борис слышит «после»",
        Duration::from_secs(60),
        || heard(&boris, space, "после"),
    )
    .await;
    boris
        .send_message(space, channel, "ответ", None, None, Vec::new())
        .await
        .expect("ответ Бориса");
    until(
        "Алиса слышит ответ",
        Duration::from_secs(60),
        || heard(&alice, space, "ответ"),
    )
    .await;

    // Старая ссылка больше никого не впускает к исключённой: её не пустят и
    // по действующему приглашению.
    let err = vera
        .join_space(&invite)
        .await
        .expect_err("исключённую не впускают");
    assert!(err.to_string().contains("исключили"), "{err}");
}

/// Участник был офлайн, пока ключ меняли. Вернувшись, он стучится прежним
/// ключом — и получает в ответ раздачу нового, а не тишину.
#[tokio::test(flavor = "multi_thread")]
async fn member_who_slept_through_rotation_catches_up() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let db_a = workspace("sa").join("bred.sqlite");
    let db_b = workspace("sb").join("bred.sqlite");
    let (alice, _na) = App::start(&db_a).await.expect("А");

    let space = alice.create_space("Сон").await.expect("пространство");
    until("адрес узла", Duration::from_secs(20), || {
        !alice.net.addr_now().is_empty()
    })
    .await;
    let invite = alice.invite(space).await.expect("приглашение");
    {
        let (boris, _nb) = App::start(&db_b).await.expect("Б");
        boris.join_space(&invite).await.expect("вход Бориса");
        until(
            "Алиса знает ключ Бориса",
            Duration::from_secs(60),
            || {
                alice
                    .members(space)
                    .map(|m| m.iter().any(|m| m.id == boris.me() && m.dh.is_some()))
                    .unwrap_or(false)
            },
        )
        .await;
        boris.net.shutdown().await;
    }

    // Пока Бориса нет, ключ меняют дважды.
    assert_eq!(alice.rotate_key(space).await.expect("смена 1"), 2);
    alice.rotate_key(space).await.expect("смена 2");
    assert_eq!(alice.governance(space).unwrap().epoch, 2);

    let (boris, _nb) = App::start(&db_b).await.expect("Б снова");
    assert_eq!(
        boris.governance(space).unwrap().epoch,
        0,
        "проснулся со старым ключом"
    );
    until(
        "Борис догнал ключ",
        Duration::from_secs(90),
        || {
            boris
                .governance(space)
                .map(|g| g.epoch == 2)
                .unwrap_or(false)
        },
    )
    .await;

    let channel = alice.channels(space).expect("каналы")[0].id;
    alice
        .send_message(space, channel, "доброе утро", None, None, Vec::new())
        .await
        .expect("сообщение");
    until(
        "Борис слышит Алису",
        Duration::from_secs(60),
        || heard(&boris, space, "доброе утро"),
    )
    .await;
}
