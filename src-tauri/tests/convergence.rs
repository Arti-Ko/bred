//! Проверка главного обещания БРЕД: два узла, обменявшись событиями в любом
//! порядке, приходят к одинаковой истории — без сервера, который бы этот
//! порядок назначал.
//!
//! Сеть здесь намеренно не поднимается: тест воспроизводит ровно ту логику
//! досинхронизации, что и `net::sync` (вектор версий → недостающие события),
//! но остаётся детерминированным и не зависит от релеев и NAT.

use std::sync::Arc;

use bred_lib::{
    domain::{now_ms, Clock, Event, EventKind, Id, SignedEvent, Space, SpaceId},
    identity::Identity,
    net::Ctx,
    store::Store,
};

/// Узел без сети: хранилище, личность и общий контекст применения событий.
struct Node {
    ctx: Arc<Ctx>,
    identity: Identity,
    store: Arc<Store>,
}

impl Node {
    fn new(space: &Space) -> Self {
        let store = Arc::new(Store::in_memory().expect("хранилище"));
        let identity = Identity::load_or_create(&store).expect("личность");
        store.set_me(identity.id());
        store.save_space(space).expect("пространство");

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        // Уведомления в тесте не нужны, но канал должен жить, иначе отправка
        // начнёт возвращать ошибку и замаскирует настоящие сбои.
        std::mem::forget(rx);

        let ctx = Arc::new(Ctx::new(
            store.clone(),
            identity.clone(),
            vec![space.clone()],
            Clock::default(),
            tx,
            std::env::temp_dir().join("bred-test-blobs"),
        ));
        Self {
            ctx,
            identity,
            store,
        }
    }

    /// Создание собственного события — то же, что делает `App::commit_and_publish`.
    fn commit(&self, space: SpaceId, kind: EventKind) -> SignedEvent {
        let author = self.identity.id();
        let seq = self.store.next_seq(space, author).expect("seq");
        let lamport = self.ctx.clock.lock().tick();

        let event = Event {
            space,
            author,
            seq,
            lamport,
            ts: now_ms(),
            kind,
        };
        let sig = self.identity.sign(&event.canonical_bytes());
        let signed = SignedEvent { event, sig };

        self.ctx.apply(&signed).expect("применение своего события");
        signed
    }

    /// Одна сторона досинхронизации: отдать то, чего нет у пира.
    fn events_for(&self, space: SpaceId, peer: &Node) -> Vec<SignedEvent> {
        let peer_has = peer.store.version_vector(space).expect("вектор версий");
        self.store
            .events_missing_for(space, &peer_has, 1000)
            .expect("недостающие события")
    }

    fn absorb(&self, events: &[SignedEvent]) -> usize {
        self.ctx.apply_batch(events).expect("применение пачки")
    }

    fn feed(&self, channel: Id) -> Vec<String> {
        self.store
            .messages(channel, 200, None)
            .expect("лента")
            .into_iter()
            .map(|m| format!("{}:{}", m.nick, m.body))
            .collect()
    }
}

fn test_space() -> Space {
    Space {
        id: Id([42u8; 32]),
        name: "Орбита".into(),
        key: [7u8; 32],
        direct: None,
    }
}

/// Полный обмен в обе стороны, как при встрече двух узлов.
fn sync(a: &Node, b: &Node, space: SpaceId) {
    let to_b = a.events_for(space, b);
    b.absorb(&to_b);
    let to_a = b.events_for(space, a);
    a.absorb(&to_a);
}

#[test]
fn two_nodes_converge_after_writing_concurrently() {
    let space = test_space();
    let alice = Node::new(&space);
    let boris = Node::new(&space);

    // Канал заводит Алиса, и он должен доехать до Бориса вместе с сообщениями.
    let channel = alice
        .commit(
            space.id,
            EventKind::ChannelCreate {
                name: "общий-канал".into(),
                category: "общее".into(),
                voice: false,
            },
        )
        .id();

    alice.commit(
        space.id,
        EventKind::Profile {
            nick: "алиса".into(),
            avatar: None,
            dh: None,
        },
    );
    boris.commit(
        space.id,
        EventKind::Profile {
            nick: "борис".into(),
            avatar: None,
            dh: None,
        },
    );

    // Пишут одновременно, ничего друг о друге не зная.
    for text in ["первое", "второе"] {
        alice.commit(
            space.id,
            EventKind::Message {
                channel,
                body: text.into(),
                reply_to: None,
                thread: None,
                attachments: Vec::new(),
            },
        );
    }
    boris.commit(
        space.id,
        EventKind::Message {
            channel,
            body: "борисово".into(),
            reply_to: None,
            thread: None,
            attachments: Vec::new(),
        },
    );

    sync(&alice, &boris, space.id);
    // Второй заход нужен, потому что после первого у Бориса появились события,
    // которые Алиса ещё не видела в момент своего запроса.
    sync(&alice, &boris, space.id);

    let from_alice = alice.feed(channel);
    let from_boris = boris.feed(channel);

    assert_eq!(from_alice, from_boris, "ленты должны совпасть дословно");
    assert_eq!(from_alice.len(), 3, "должны доехать все три сообщения");
    assert!(
        from_alice.iter().any(|line| line.contains("борисово")),
        "сообщение соседа обязано появиться у Алисы: {from_alice:?}"
    );
}

#[test]
fn repeated_delivery_changes_nothing() {
    let space = test_space();
    let alice = Node::new(&space);
    let boris = Node::new(&space);

    let channel = alice
        .commit(
            space.id,
            EventKind::ChannelCreate {
                name: "общий".into(),
                category: "общее".into(),
                voice: false,
            },
        )
        .id();
    alice.commit(
        space.id,
        EventKind::Message {
            channel,
            body: "привет".into(),
            reply_to: None,
            thread: None,
            attachments: Vec::new(),
        },
    );

    let batch = alice.events_for(space.id, &boris);
    let first = boris.absorb(&batch);
    let second = boris.absorb(&batch);
    let third = boris.absorb(&batch);

    assert_eq!(first, 2, "в первый раз события новые");
    assert_eq!((second, third), (0, 0), "повторы обязаны быть безвредны");
    assert_eq!(
        boris.feed(channel).len(),
        1,
        "дубликатов в ленте быть не должно"
    );
}

#[test]
fn forged_event_is_rejected() {
    let space = test_space();
    let alice = Node::new(&space);
    let boris = Node::new(&space);

    let channel = alice
        .commit(
            space.id,
            EventKind::ChannelCreate {
                name: "общий".into(),
                category: "общее".into(),
                voice: false,
            },
        )
        .id();
    let mut forged = alice.commit(
        space.id,
        EventKind::Message {
            channel,
            body: "исходный текст".into(),
            reply_to: None,
            thread: None,
            attachments: Vec::new(),
        },
    );

    // Злоумышленник переписал тело, оставив чужую подпись.
    forged.event.kind = EventKind::Message {
        channel,
        body: "подменённый текст".into(),
        reply_to: None,
        thread: None,
        attachments: Vec::new(),
    };

    let accepted = boris.absorb(&[forged]);
    assert_eq!(
        accepted, 0,
        "событие с несошедшейся подписью должно быть отброшено"
    );
    assert!(
        boris.feed(channel).is_empty(),
        "подделка не должна попасть в ленту"
    );
}

#[test]
fn reactions_and_edits_converge() {
    let space = test_space();
    let alice = Node::new(&space);
    let boris = Node::new(&space);

    let channel = alice
        .commit(
            space.id,
            EventKind::ChannelCreate {
                name: "общий".into(),
                category: "общее".into(),
                voice: false,
            },
        )
        .id();
    let message = alice
        .commit(
            space.id,
            EventKind::Message {
                channel,
                body: "черновик".into(),
                reply_to: None,
                thread: None,
                attachments: Vec::new(),
            },
        )
        .id();

    sync(&alice, &boris, space.id);

    // Алиса правит своё сообщение, Борис ставит реакцию — одновременно.
    alice.commit(
        space.id,
        EventKind::Edit {
            target: message,
            body: "итоговый текст".into(),
        },
    );
    boris.commit(
        space.id,
        EventKind::Reaction {
            target: message,
            emoji: "★".into(),
            remove: false,
        },
    );

    sync(&alice, &boris, space.id);
    sync(&alice, &boris, space.id);

    for (who, node) in [("алиса", &alice), ("борис", &boris)] {
        let rows = node.store.messages(channel, 10, None).expect("лента");
        let row = rows.first().expect("сообщение на месте");
        assert_eq!(row.body, "итоговый текст", "правка не доехала до {who}");
        assert!(row.edited, "пометка о правке потерялась у {who}");
        assert_eq!(row.reactions.len(), 1, "реакция не доехала до {who}");
        assert_eq!(row.reactions[0].count, 1);
    }
}

#[test]
fn events_from_unknown_space_are_ignored() {
    let space = test_space();
    let alice = Node::new(&space);

    let other = Space {
        id: Id([99u8; 32]),
        name: "чужое".into(),
        key: [1u8; 32],
        direct: None,
    };
    let stranger = Node::new(&other);
    let channel = stranger
        .commit(
            other.id,
            EventKind::ChannelCreate {
                name: "секрет".into(),
                category: "общее".into(),
                voice: false,
            },
        )
        .id();
    let intruder = stranger.commit(
        other.id,
        EventKind::Message {
            channel,
            body: "не для вас".into(),
            reply_to: None,
            thread: None,
            attachments: Vec::new(),
        },
    );

    assert_eq!(
        alice.absorb(&[intruder]),
        0,
        "события чужого пространства не должны приниматься"
    );
    assert!(alice.feed(channel).is_empty());
}

#[test]
fn gap_in_the_log_is_healed_by_sync() {
    let space = test_space();
    let alice = Node::new(&space);
    let boris = Node::new(&space);

    let channel = alice
        .commit(
            space.id,
            EventKind::ChannelCreate {
                name: "общий".into(),
                category: "общее".into(),
                voice: false,
            },
        )
        .id();
    for text in ["первое", "второе", "третье"] {
        alice.commit(
            space.id,
            EventKind::Message {
                channel,
                body: text.into(),
                reply_to: None,
                thread: None,
                attachments: Vec::new(),
            },
        );
    }

    // Изображаем потерю в рое: одно событие из середины не доехало.
    let all = alice.events_for(space.id, &boris);
    let lossy: Vec<_> = all.iter().filter(|e| e.event.seq != 3).cloned().collect();
    boris.absorb(&lossy);
    assert_eq!(
        boris.feed(channel).len(),
        2,
        "дыра действительно образовалась"
    );

    // Досинхронизация обязана её закрыть, а не считать, что всё на месте.
    sync(&alice, &boris, space.id);
    sync(&alice, &boris, space.id);

    assert_eq!(
        boris.feed(channel),
        alice.feed(channel),
        "пропущенное событие должно доехать при следующей встрече"
    );
}

/// Досинхронизация через настоящий протокол (трубой вместо QUIC), с раундами.
async fn sync_over_wire(a: &Node, b: &Node, space: &Space) -> usize {
    use bred_lib::net::sync;

    let mut total = 0;
    for _ in 0..64 {
        let (client, server) = tokio::io::duplex(1024 * 1024);
        let (server_read, server_write) = tokio::io::split(server);
        let (client_read, client_write) = tokio::io::split(client);

        let serving = sync::serve_round(&a.ctx, server_write, server_read);
        let fetching = sync::sync_round(&b.ctx, client_write, client_read, space.id, &space.key);

        let (served, round) = tokio::join!(serving, fetching);
        served.expect("сторона-раздатчик");
        let round = round.expect("сторона-получатель");
        total += round.received;
        if round.received == 0 && round.sent == 0 {
            break;
        }
    }
    total
}

#[tokio::test]
async fn history_larger_than_one_batch_arrives_completely() {
    let space = test_space();
    let alice = Node::new(&space);
    let boris = Node::new(&space);

    let channel = alice
        .commit(
            space.id,
            EventKind::ChannelCreate {
                name: "общий".into(),
                category: "общее".into(),
                voice: false,
            },
        )
        .id();

    // Заведомо больше одной пачки досинхронизации (SYNC_BATCH = 512).
    let count = 700;
    for i in 0..count {
        alice.commit(
            space.id,
            EventKind::Message {
                channel,
                body: format!("сообщение {i}"),
                reply_to: None,
                thread: None,
                attachments: Vec::new(),
            },
        );
    }

    sync_over_wire(&alice, &boris, &space).await;

    let mine = boris.store.messages(channel, 2000, None).expect("лента");
    assert_eq!(
        mine.len(),
        count,
        "длинная история обязана доехать целиком, а не одной пачкой"
    );
}

#[test]
fn feed_pages_backwards_without_gaps_or_repeats() {
    let space = test_space();
    let alice = Node::new(&space);

    let channel = alice
        .commit(
            space.id,
            EventKind::ChannelCreate {
                name: "общий".into(),
                category: "общее".into(),
                voice: false,
            },
        )
        .id();
    for i in 0..500 {
        alice.commit(
            space.id,
            EventKind::Message {
                channel,
                body: format!("строка {i}"),
                reply_to: None,
                thread: None,
                attachments: Vec::new(),
            },
        );
    }

    let newest = alice
        .store
        .messages(channel, 200, None)
        .expect("страница 1");
    assert_eq!(newest.len(), 200);
    assert_eq!(
        newest.last().unwrap().body,
        "строка 499",
        "лента идёт снизу вверх"
    );

    let cursor = newest.first().unwrap().lamport;
    let older = alice
        .store
        .messages(channel, 200, Some(cursor))
        .expect("страница 2");
    assert_eq!(older.len(), 200);

    // Стык страниц: ни дыр, ни повторов.
    let seen: std::collections::HashSet<_> = newest.iter().map(|m| m.id).collect();
    assert!(
        older.iter().all(|m| !seen.contains(&m.id)),
        "страницы не должны пересекаться"
    );
    assert!(
        older.last().unwrap().lamport < newest.first().unwrap().lamport,
        "вторая страница обязана быть строго старше первой"
    );
}

#[test]
fn thread_replies_stay_out_of_the_channel_feed() {
    let space = test_space();
    let alice = Node::new(&space);

    let channel = alice
        .commit(
            space.id,
            EventKind::ChannelCreate {
                name: "общий".into(),
                category: "общее".into(),
                voice: false,
            },
        )
        .id();
    let root = alice
        .commit(
            space.id,
            EventKind::Message {
                channel,
                body: "корень обсуждения".into(),
                reply_to: None,
                thread: None,
                attachments: Vec::new(),
            },
        )
        .id();
    for i in 0..3 {
        alice.commit(
            space.id,
            EventKind::Message {
                channel,
                body: format!("ответ {i}"),
                reply_to: None,
                thread: Some(root),
                attachments: Vec::new(),
            },
        );
    }

    let feed = alice.store.messages(channel, 50, None).expect("лента");
    assert_eq!(feed.len(), 1, "ответы из ветки не должны засорять канал");
    assert_eq!(
        feed[0].thread_replies, 3,
        "счётчик ветки должен считать ответы"
    );

    let thread = alice.store.thread(root).expect("ветка");
    assert_eq!(thread.len(), 4, "в ветке корень и все ответы");
    assert_eq!(thread[0].body, "корень обсуждения");
}

#[test]
fn orphan_files_are_collected_but_shared_ones_survive() {
    use bred_lib::domain::Attachment;

    let space = test_space();
    let alice = Node::new(&space);
    let dir = std::env::temp_dir().join(format!("bred-gc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let channel = alice
        .commit(
            space.id,
            EventKind::ChannelCreate {
                name: "общий".into(),
                category: "общее".into(),
                voice: false,
            },
        )
        .id();

    let shared = Attachment {
        hash: Id([70u8; 32]),
        name: "общий.png".into(),
        size: 10,
        mime: "image/png".into(),
    };
    let path = dir.join("blob");
    std::fs::write(&path, b"0123456789").unwrap();
    alice.store.record_blob(shared.hash, &path, 10).unwrap();

    // Один и тот же файл переслан двумя сообщениями.
    let first = alice.commit(
        space.id,
        EventKind::Message {
            channel,
            body: "раз".into(),
            reply_to: None,
            thread: None,
            attachments: vec![shared.clone()],
        },
    );
    alice.commit(
        space.id,
        EventKind::Message {
            channel,
            body: "два".into(),
            reply_to: None,
            thread: None,
            attachments: vec![shared.clone()],
        },
    );

    assert!(
        alice.store.orphan_blobs().unwrap().is_empty(),
        "на файл ссылаются — он не мусор"
    );

    // Удаляем только первое сообщение: второе всё ещё держит файл.
    alice.commit(space.id, EventKind::Delete { target: first.id() });
    assert!(
        alice.store.orphan_blobs().unwrap().is_empty(),
        "пока жива вторая ссылка, файл удалять нельзя"
    );

    // Теперь ссылок не осталось.
    let second_id = alice
        .store
        .messages(channel, 10, None)
        .unwrap()
        .last()
        .unwrap()
        .id;
    alice.commit(space.id, EventKind::Delete { target: second_id });

    let orphans = alice.store.orphan_blobs().unwrap();
    assert_eq!(
        orphans.len(),
        1,
        "последняя ссылка ушла — файл стал мусором"
    );
    assert_eq!(orphans[0].0, shared.hash);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn forked_author_is_recorded_not_silently_dropped() {
    let space = test_space();
    let alice = Node::new(&space);
    let boris = Node::new(&space);

    let channel = alice
        .commit(
            space.id,
            EventKind::ChannelCreate {
                name: "общий".into(),
                category: "общее".into(),
                voice: false,
            },
        )
        .id();
    let honest = alice.commit(
        space.id,
        EventKind::Message {
            channel,
            body: "как было на самом деле".into(),
            reply_to: None,
            thread: None,
            attachments: Vec::new(),
        },
    );

    // Автор переписывает собственную историю: другое содержимое под тем же
    // номером в его логе. Подпись при этом настоящая — подделкой не поймать.
    let mut forged = honest.clone();
    forged.event.kind = EventKind::Message {
        channel,
        body: "как он хочет это подать".into(),
        reply_to: None,
        thread: None,
        attachments: Vec::new(),
    };
    let sig = alice.identity.sign(&forged.event.canonical_bytes());
    let forged = bred_lib::domain::SignedEvent {
        event: forged.event,
        sig,
    };

    boris.absorb(std::slice::from_ref(&honest));
    boris.absorb(&[forged]);

    assert_eq!(
        boris.feed(channel).len(),
        1,
        "вторая версия не должна попасть в ленту"
    );
    assert_eq!(
        boris.store.forks(space.id).unwrap().len(),
        1,
        "раздвоение обязано быть зафиксировано, а не проглочено"
    );
    assert_eq!(
        boris.store.forks(space.id).unwrap()[0].0,
        alice.identity.id()
    );
}
