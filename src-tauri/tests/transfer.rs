//! Передача файла между двумя узлами целиком, включая докачку.
//!
//! Протокол гоняется через обычную трубу вместо QUIC: сеть здесь ничего не
//! добавляет к проверке, зато сделала бы тест медленным и капризным.

use std::sync::Arc;

use bred_lib::{
    domain::{Clock, Id, Space},
    identity::Identity,
    net::{blobs, Ctx},
    store::Store,
};

struct Node {
    ctx: Arc<Ctx>,
    dir: std::path::PathBuf,
}

fn node(space: &Space, tag: &str) -> Node {
    let dir = std::env::temp_dir().join(format!(
        "bred-transfer-{}-{tag}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).expect("каталог");

    let store = Arc::new(Store::in_memory().expect("хранилище"));
    let identity = Identity::load_or_create(&store).expect("личность");
    store.set_me(identity.id());
    store.save_space(space).expect("пространство");

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    std::mem::forget(rx);

    let ctx = Arc::new(Ctx::new(
        store,
        identity,
        vec![space.clone()],
        Clock::default(),
        tx,
        &dir,
    ));
    Node { ctx, dir }
}

fn test_space() -> Space {
    Space {
        id: Id([5u8; 32]),
        name: "Орбита".into(),
        key: [11u8; 32],
        direct: None,
    }
}

/// Один заход скачивания: соединяем стороны трубой и ждём обе половины.
async fn transfer(
    server: &Ctx,
    client: &Ctx,
    space: &Space,
    hash: Id,
    target: &std::path::Path,
    partial: &std::path::Path,
    offset: u64,
) -> anyhow::Result<std::path::PathBuf> {
    let (client_side, server_side) = tokio::io::duplex(8 * 1024);
    let (server_read, server_write) = tokio::io::split(server_side);
    let (client_read, client_write) = tokio::io::split(client_side);

    let serving = blobs::serve_stream(server, server_write, server_read);
    let fetching = blobs::fetch_stream(
        client,
        client_write,
        client_read,
        space.id,
        hash,
        &space.key,
        target,
        partial,
        offset,
    );

    let (served, fetched) = tokio::join!(serving, fetching);
    served?;
    fetched
}

#[tokio::test]
async fn file_arrives_byte_for_byte() {
    let space = test_space();
    let alice = node(&space, "a");
    let boris = node(&space, "b");

    // Заведомо больше одного куска чтения, чтобы проверить и склейку.
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let source = alice.dir.join("отчёт.bin");
    std::fs::write(&source, &payload).expect("исходный файл");

    let attachment = blobs::import(&alice.ctx, &source).await.expect("импорт");
    assert_eq!(attachment.size, payload.len() as u64);
    assert_eq!(attachment.name, "отчёт.bin");

    let target = boris.ctx.blob_path(attachment.hash);
    let partial = target.with_extension("part");

    let got = transfer(
        &alice.ctx,
        &boris.ctx,
        &space,
        attachment.hash,
        &target,
        &partial,
        0,
    )
    .await
    .expect("файл должен доехать");

    let received = std::fs::read(&got).expect("полученный файл");
    assert_eq!(received, payload, "байты обязаны совпасть один в один");
    assert!(
        boris
            .ctx
            .store
            .blob_path(attachment.hash)
            .unwrap()
            .is_some(),
        "получатель должен запомнить, что файл теперь у него"
    );

    std::fs::remove_dir_all(&alice.dir).ok();
    std::fs::remove_dir_all(&boris.dir).ok();
}

#[tokio::test]
async fn download_resumes_from_where_it_stopped() {
    let space = test_space();
    let alice = node(&space, "ra");
    let boris = node(&space, "rb");

    let payload: Vec<u8> = (0..50_000u32).map(|i| (i % 97) as u8).collect();
    let source = alice.dir.join("большой.bin");
    std::fs::write(&source, &payload).expect("исходный файл");
    let attachment = blobs::import(&alice.ctx, &source).await.expect("импорт");

    let target = boris.ctx.blob_path(attachment.hash);
    let partial = target.with_extension("part");

    // Изображаем оборванную загрузку: часть файла уже лежит в .part.
    std::fs::create_dir_all(partial.parent().unwrap()).unwrap();
    std::fs::write(&partial, &payload[..20_000]).expect("недокачанный кусок");

    let got = transfer(
        &alice.ctx,
        &boris.ctx,
        &space,
        attachment.hash,
        &target,
        &partial,
        20_000,
    )
    .await
    .expect("докачка должна завершиться");

    let received = std::fs::read(&got).expect("полученный файл");
    assert_eq!(
        received, payload,
        "докачка обязана дать тот же файл, что и полная загрузка"
    );

    std::fs::remove_dir_all(&alice.dir).ok();
    std::fs::remove_dir_all(&boris.dir).ok();
}

#[tokio::test]
async fn stranger_without_space_key_gets_nothing() {
    let space = test_space();
    let alice = node(&space, "sa");

    let other = Space {
        id: Id([6u8; 32]),
        name: "чужое".into(),
        key: [77u8; 32],
        direct: None,
    };
    let stranger = node(&other, "sb");

    let source = alice.dir.join("секрет.txt");
    std::fs::write(&source, "содержимое не для чужих").expect("исходный файл");
    let attachment = blobs::import(&alice.ctx, &source).await.expect("импорт");

    let target = stranger.ctx.blob_path(attachment.hash);
    let partial = target.with_extension("part");

    // Чужак шлёт запрос своим ключом — расшифровать его Алиса не сможет.
    let result = transfer(
        &alice.ctx,
        &stranger.ctx,
        &other,
        attachment.hash,
        &target,
        &partial,
        0,
    )
    .await;

    assert!(
        result.is_err(),
        "без ключа пространства файл не должен отдаваться"
    );
    assert!(!target.exists(), "у чужака не должно остаться файла");

    std::fs::remove_dir_all(&alice.dir).ok();
    std::fs::remove_dir_all(&stranger.dir).ok();
}

#[tokio::test]
async fn missing_file_is_reported_not_hung() {
    let space = test_space();
    let alice = node(&space, "ma");
    let boris = node(&space, "mb");

    let unknown = Id([200u8; 32]);
    let target = boris.ctx.blob_path(unknown);
    let partial = target.with_extension("part");

    let result = transfer(
        &alice.ctx, &boris.ctx, &space, unknown, &target, &partial, 0,
    )
    .await;

    assert!(result.is_err(), "об отсутствии файла надо сообщать явно");

    std::fs::remove_dir_all(&alice.dir).ok();
    std::fs::remove_dir_all(&boris.dir).ok();
}
