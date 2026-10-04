//! Тесты правил власти: `store::governance`.

use super::*;
use crate::domain::{governance::founded_id, Space};

const OWNER: Id = Id([1u8; 32]);
const ADMIN: Id = Id([2u8; 32]);
const MEMBER: Id = Id([3u8; 32]);
const STRANGER: Id = Id([4u8; 32]);
const NONCE: [u8; 32] = [9u8; 32];

struct World {
    store: Store,
    space: SpaceId,
    seqs: std::collections::HashMap<Id, u64>,
    /// Настенные часы событий: по умолчанию — после выпуска 0.8.
    clock: i64,
}

impl World {
    /// Пространство с доказуемым владельцем и одним каналом.
    fn founded() -> (Self, Id) {
        let store = Store::in_memory().unwrap();
        store.set_me(OWNER);
        let space = founded_id(OWNER, &NONCE);
        store
            .save_space(&Space {
                id: space,
                name: "кухня".into(),
                key: [5u8; 32],
                direct: None,
            })
            .unwrap();
        let mut world = Self {
            store,
            space,
            seqs: Default::default(),
            clock: LEGACY_UNTIL_MS,
        };
        world.put(
            OWNER,
            1,
            EventKind::Founded {
                name: "кухня".into(),
                nonce: NONCE,
            },
        );
        let channel = world.put(
            OWNER,
            2,
            EventKind::ChannelCreate {
                name: "общий".into(),
                category: String::new(),
                voice: false,
            },
        );
        (world, channel)
    }

    fn put(&mut self, author: Id, lamport: u64, kind: EventKind) -> Id {
        let seq = self.seqs.entry(author).or_insert(0);
        *seq += 1;
        let seq = *seq;
        self.put_at(author, seq, lamport, kind)
    }

    /// Событие с заданным номером — для попыток «застолбить» номер наперёд.
    fn put_at(&mut self, author: Id, seq: u64, lamport: u64, kind: EventKind) -> Id {
        let signed = SignedEvent {
            event: Event {
                space: self.space,
                author,
                seq,
                lamport,
                ts: self.clock + lamport as i64,
                kind,
            },
            sig: [0u8; 64],
        };
        self.store.apply(&signed).unwrap();
        signed.id()
    }

    /// Последний номер события человека, который мы видели, — отметка решений.
    fn cut(&self, who: Id) -> u64 {
        self.seqs.get(&who).copied().unwrap_or(0)
    }

    fn grant(&mut self, lamport: u64, member: Id, admin: bool) -> Id {
        let cut = self.cut(member);
        self.put(OWNER, lamport, EventKind::RoleSet { member, admin, cut })
    }

    fn remove(&mut self, by: Id, lamport: u64, member: Id) -> Id {
        let cut = self.cut(member);
        self.put(by, lamport, EventKind::MemberRemove { member, cut })
    }

    fn rotate(&mut self, by: Id, lamport: u64, epoch: u64, prev: Option<Id>, to: &[Id]) -> Id {
        self.put(
            by,
            lamport,
            EventKind::KeyRotate {
                epoch,
                prev,
                ephemeral: Id([8u8; 32]),
                wraps: to
                    .iter()
                    .map(|member| crate::domain::governance::KeyWrap {
                        member: *member,
                        sealed: vec![0u8; 40],
                    })
                    .collect(),
                check: Id([epoch as u8; 32]),
            },
        )
    }

    fn head(&self) -> Option<Id> {
        self.store.rotation_head(self.space).unwrap().map(|h| h.id)
    }

    fn channels(&self) -> usize {
        self.store.channels(self.space).unwrap().len()
    }

    fn say(&mut self, author: Id, lamport: u64, channel: Id) -> Id {
        self.put(
            author,
            lamport,
            EventKind::Message {
                channel,
                body: "привет".into(),
                reply_to: None,
                thread: None,
                attachments: Vec::new(),
            },
        )
    }

    fn deleted(&self, message: Id) -> bool {
        self.store
            .message(message)
            .unwrap()
            .is_none_or(|m| m.deleted)
    }
}

#[test]
fn founder_is_owner_and_forged_claim_changes_nothing() {
    let (mut w, channel) = World::founded();
    assert_eq!(w.store.owner(w.space).unwrap(), Some(OWNER));
    // Подделка: «Создано» от чужого с ранними часами.
    w.put(
        STRANGER,
        0,
        EventKind::SpaceCreate {
            name: "моё".into()
        },
    );
    assert_eq!(w.store.owner(w.space).unwrap(), Some(OWNER));
    w.put(STRANGER, 5, EventKind::ChannelDelete { channel });
    assert_eq!(w.channels(), 1, "рядовой участник каналы не удаляет");
    w.put(OWNER, 6, EventKind::ChannelDelete { channel });
    assert_eq!(w.channels(), 0);
}

#[test]
fn wrong_nonce_does_not_found_anything() {
    let store = Store::in_memory().unwrap();
    let space = founded_id(OWNER, &NONCE);
    store
        .save_space(&Space {
            id: space,
            name: "x".into(),
            key: [5u8; 32],
            direct: None,
        })
        .unwrap();
    let mut w = World {
        store,
        space,
        seqs: Default::default(),
        clock: LEGACY_UNTIL_MS,
    };
    w.put(
        STRANGER,
        1,
        EventKind::Founded {
            name: "x".into(),
            nonce: NONCE,
        },
    );
    assert_eq!(
        w.store.owner(space).unwrap(),
        None,
        "соль чужая — владельцем не стать"
    );
}

#[test]
fn two_legacy_claims_leave_nobody_in_charge() {
    let store = Store::in_memory().unwrap();
    let space = Id([7u8; 32]);
    store
        .save_space(&Space {
            id: space,
            name: "x".into(),
            key: [5u8; 32],
            direct: None,
        })
        .unwrap();
    let mut w = World {
        store,
        space,
        seqs: Default::default(),
        clock: LEGACY_UNTIL_MS,
    };
    w.put(OWNER, 3, EventKind::SpaceCreate { name: "x".into() });
    assert_eq!(w.store.owner(space).unwrap(), Some(OWNER));
    w.put(STRANGER, 1, EventKind::SpaceCreate { name: "y".into() });
    assert_eq!(
        w.store.owner(space).unwrap(),
        None,
        "спор — власти нет ни у кого"
    );
}

#[test]
fn late_role_grant_validates_earlier_moderation() {
    let (mut w, channel) = World::founded();
    let message = w.say(MEMBER, 3, channel);
    // Удаление администратора приехало раньше, чем его назначение.
    w.put(ADMIN, 10, EventKind::Delete { target: message });
    assert!(
        !w.deleted(message),
        "пока не администратор — чужое не удаляет"
    );
    // Владелец назначил, не видя ни одного события администратора: отметка 0,
    // и всё подписанное им — уже «после назначения».
    w.put(
        OWNER,
        5,
        EventKind::RoleSet {
            member: ADMIN,
            admin: true,
            cut: 0,
        },
    );
    assert!(
        w.deleted(message),
        "назначение из прошлого пересчитало удаление"
    );
}

#[test]
fn removal_silences_later_events_and_respects_rank() {
    let (mut w, channel) = World::founded();
    w.grant(3, ADMIN, true);
    w.grant(4, STRANGER, true);
    // Администратор не может исключить владельца или другого администратора.
    w.remove(ADMIN, 5, OWNER);
    w.remove(ADMIN, 6, STRANGER);
    assert!(!w.store.is_removed(w.space, OWNER).unwrap());
    assert!(!w.store.is_removed(w.space, STRANGER).unwrap());

    let before = w.say(MEMBER, 7, channel);
    let cut = w.cut(MEMBER);
    // Сообщение после исключения доехало раньше самого исключения.
    let after = w.say(MEMBER, 20, channel);
    w.put(
        ADMIN,
        10,
        EventKind::MemberRemove {
            member: MEMBER,
            cut,
        },
    );
    assert!(w.store.is_removed(w.space, MEMBER).unwrap());
    assert!(
        w.store.message(before).unwrap().is_some(),
        "прошлое остаётся"
    );
    assert!(
        w.store.message(after).unwrap().is_none(),
        "сказанное после исключения не считается"
    );
    assert!(w
        .store
        .members(w.space)
        .unwrap()
        .iter()
        .all(|m| m.id != MEMBER));
}

#[test]
fn removed_member_cannot_backdate_clocks_to_slip_through() {
    let (mut w, channel) = World::founded();
    w.say(MEMBER, 3, channel);
    w.remove(OWNER, 4, MEMBER);
    // Часы «из прошлого» — но номер в его логе уже после отметки.
    let sneaky = w.say(MEMBER, 1, channel);
    assert!(w.store.message(sneaky).unwrap().is_none());
    w.put(MEMBER, 1, EventKind::ChannelDelete { channel });
    assert_eq!(w.channels(), 1);
}

#[test]
fn demoted_admin_loses_powers_whatever_the_clocks_say() {
    let (mut w, channel) = World::founded();
    let first = w.say(MEMBER, 3, channel);
    let second = w.say(MEMBER, 4, channel);
    w.grant(5, ADMIN, true);
    w.put(ADMIN, 6, EventKind::Delete { target: first });
    assert!(w.deleted(first), "пока админ — может");
    w.grant(7, ADMIN, false);
    // Разжалованный подписывает удаление с часами раньше разжалования.
    w.put(ADMIN, 2, EventKind::Delete { target: second });
    assert!(!w.deleted(second), "после разжалования — нет");
    assert!(w.deleted(first), "сделанное до разжалования остаётся");
    assert_eq!(w.store.role(w.space, ADMIN).unwrap(), Role::Member);
}

#[test]
fn invalid_decisions_do_not_trigger_rebuilds() {
    let (mut w, channel) = World::founded();
    w.say(MEMBER, 3, channel);
    // Рядовой участник «назначает» и «исключает» — ничего не меняется,
    // и пересобирать нечего.
    let conn = w.store.conn.lock();
    let before: i64 = conn
        .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))
        .unwrap();
    drop(conn);
    for lamport in 0..5 {
        w.put(
            STRANGER,
            lamport,
            EventKind::RoleSet {
                member: STRANGER,
                admin: true,
                cut: 0,
            },
        );
        w.put(
            STRANGER,
            lamport,
            EventKind::MemberRemove {
                member: MEMBER,
                cut: 0,
            },
        );
    }
    assert!(!w.store.is_removed(w.space, MEMBER).unwrap());
    assert_eq!(w.store.role(w.space, STRANGER).unwrap(), Role::Member);
    let conn = w.store.conn.lock();
    let after: i64 = conn
        .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, after);
}

#[test]
fn key_chain_follows_the_owner_and_refuses_shortcuts() {
    let (mut w, _) = World::founded();
    w.rotate(MEMBER, 3, 1, None, &[OWNER, MEMBER]);
    assert_eq!(w.head(), None, "рядовой ключ не меняет");

    let first = w.rotate(OWNER, 4, 1, None, &[OWNER, ADMIN, MEMBER]);
    assert_eq!(w.head(), Some(first));

    w.grant(5, ADMIN, true);
    // Прыжок через номера — мимо цепочки.
    w.rotate(ADMIN, 6, 1 << 40, Some(first), &[OWNER, ADMIN]);
    assert_eq!(w.head(), Some(first));
    // Смена без владельца в раздаче — попытка отрезать его — недействительна.
    w.rotate(ADMIN, 7, 2, Some(first), &[ADMIN, MEMBER]);
    assert_eq!(w.head(), Some(first));

    let admins = w.rotate(ADMIN, 8, 2, Some(first), &[OWNER, ADMIN, MEMBER]);
    assert_eq!(w.head(), Some(admins));
    // Владелец сменил ключ с той же точки — его смена главнее.
    let owners = w.rotate(OWNER, 9, 2, Some(first), &[OWNER, MEMBER]);
    assert_eq!(w.head(), Some(owners));
    let next = w.rotate(OWNER, 10, 3, Some(owners), &[OWNER, MEMBER]);
    assert_eq!(w.head(), Some(next));
}

#[test]
fn key_known_to_removed_member_is_flagged() {
    let (mut w, channel) = World::founded();
    w.say(MEMBER, 3, channel);
    let first = w.rotate(OWNER, 4, 1, None, &[OWNER, MEMBER]);
    assert!(!w.store.key_compromised(w.space).unwrap());
    w.remove(OWNER, 5, MEMBER);
    assert!(
        w.store.key_compromised(w.space).unwrap(),
        "действующий ключ знает исключённый"
    );
    w.rotate(OWNER, 6, 2, Some(first), &[OWNER]);
    assert!(!w.store.key_compromised(w.space).unwrap());
}

#[test]
fn legacy_space_has_no_removals_or_key_changes() {
    let store = Store::in_memory().unwrap();
    let space = Id([7u8; 32]);
    store
        .save_space(&Space {
            id: space,
            name: "x".into(),
            key: [5u8; 32],
            direct: None,
        })
        .unwrap();
    let mut w = World {
        store,
        space,
        seqs: Default::default(),
        clock: LEGACY_UNTIL_MS,
    };
    w.put(OWNER, 1, EventKind::SpaceCreate { name: "x".into() });
    assert_eq!(w.store.owner(space).unwrap(), Some(OWNER));
    w.remove(OWNER, 2, MEMBER);
    assert!(
        !w.store.is_removed(space, MEMBER).unwrap(),
        "владелец на слове не исключает"
    );
    w.rotate(OWNER, 3, 1, None, &[OWNER]);
    assert_eq!(w.head(), None);
}

#[test]
fn invite_is_revoked_only_by_author_or_moderator() {
    let (mut w, _) = World::founded();
    let invite = w.put(
        MEMBER,
        3,
        EventKind::InviteCreate {
            proof: Id([6u8; 32]),
            expires: 0,
            uses: 2,
        },
    );
    w.put(STRANGER, 4, EventKind::InviteRevoke { invite });
    assert!(
        !w.store.invite(invite).unwrap().unwrap().revoked,
        "чужой отзыв — шум"
    );
    // Рядовой участник записывает входы выдуманных гостей — лимит цел.
    for fake in 20..25u8 {
        w.put(
            MEMBER,
            5,
            EventKind::InviteUse {
                invite,
                member: Id([fake; 32]),
            },
        );
    }
    assert_eq!(w.store.invite(invite).unwrap().unwrap().used, 0);
    // Вход, записанный нами самими, считается сразу.
    w.put(
        OWNER,
        5,
        EventKind::InviteUse {
            invite,
            member: STRANGER,
        },
    );
    let row = w.store.invite(invite).unwrap().unwrap();
    assert_eq!(row.used, 1);
    assert!(row.live(0));
    w.put(MEMBER, 6, EventKind::InviteRevoke { invite });
    assert!(!w.store.invite(invite).unwrap().unwrap().live(0));
}

#[test]
fn invites_of_a_removed_member_die_with_them() {
    let (mut w, _) = World::founded();
    let invite = w.put(
        MEMBER,
        3,
        EventKind::InviteCreate {
            proof: Id([6u8; 32]),
            expires: 0,
            uses: 0,
        },
    );
    assert!(w.store.invite(invite).unwrap().unwrap().live(0));
    w.remove(OWNER, 4, MEMBER);
    assert!(!w.store.invite(invite).unwrap().unwrap().live(0));
}

#[test]
fn exhausted_or_expired_invite_is_not_live() {
    let row = InviteRow {
        id: Id::ZERO,
        space: Id::ZERO,
        author: Id::ZERO,
        proof: Id::ZERO,
        expires: 100,
        uses: 1,
        used: 0,
        revoked: false,
        created: 0,
    };
    assert!(row.live(50));
    assert!(!row.live(100), "срок вышел");
    assert!(
        !InviteRow {
            used: 1,
            ..row.clone()
        }
        .live(50),
        "входы кончились"
    );
    assert!(InviteRow {
        expires: 0,
        uses: 0,
        used: 9,
        ..row
    }
    .live(i64::MAX));
}

#[test]
fn moderator_deletes_others_but_not_owners_messages() {
    let (mut w, channel) = World::founded();
    w.grant(3, ADMIN, true);
    let owners = w.say(OWNER, 4, channel);
    let members = w.say(MEMBER, 5, channel);
    w.put(ADMIN, 6, EventKind::Delete { target: owners });
    w.put(ADMIN, 7, EventKind::Delete { target: members });
    w.put(MEMBER, 8, EventKind::Delete { target: owners });
    assert!(!w.deleted(owners), "сообщения владельца не трогает никто");
    assert!(w.deleted(members));
}

#[test]
fn history_before_release_keeps_old_rules() {
    let (mut w, _) = World::founded();
    // Канал и его удаление рядовым участником — до выпуска 0.8.
    w.clock = LEGACY_UNTIL_MS - 1_000_000;
    let old = w.put(
        OWNER,
        3,
        EventKind::ChannelCreate {
            name: "старый".into(),
            category: String::new(),
            voice: false,
        },
    );
    w.put(MEMBER, 4, EventKind::ChannelDelete { channel: old });
    assert!(w
        .store
        .channels(w.space)
        .unwrap()
        .iter()
        .all(|c| c.id != old));

    // «Состаренное» удаление нового канала не проходит.
    w.clock = LEGACY_UNTIL_MS;
    let fresh = w.put(
        OWNER,
        5,
        EventKind::ChannelCreate {
            name: "новый".into(),
            category: String::new(),
            voice: false,
        },
    );
    w.clock = LEGACY_UNTIL_MS - 1_000_000;
    w.put(MEMBER, 6, EventKind::ChannelDelete { channel: fresh });
    assert!(w
        .store
        .channels(w.space)
        .unwrap()
        .iter()
        .any(|c| c.id == fresh));
}

#[test]
fn someone_elses_emoji_name_is_protected() {
    let (mut w, _) = World::founded();
    let add = |hash: u8| EventKind::EmojiAdd {
        name: "кот".into(),
        hash: Id([hash; 32]),
        sticker: false,
    };
    w.put(MEMBER, 3, add(1));
    w.put(STRANGER, 4, add(2));
    w.put(
        STRANGER,
        5,
        EventKind::EmojiRemove {
            name: "кот".into()
        },
    );
    let emojis = w.store.emojis(w.space).unwrap();
    assert_eq!(emojis.len(), 1);
    assert_eq!(emojis[0].hash, Id([1u8; 32]));
    w.put(
        OWNER,
        6,
        EventKind::EmojiRemove {
            name: "кот".into()
        },
    );
    assert!(w.store.emojis(w.space).unwrap().is_empty());
}

#[test]
fn reserving_a_far_sequence_number_buys_nothing() {
    let (mut w, channel) = World::founded();
    w.say(MEMBER, 3, channel);
    // Заранее «застолбил» номер в миллиард, чтобы отметка уехала туда же.
    w.put_at(
        MEMBER,
        1_000_000_000,
        4,
        EventKind::Profile {
            nick: "м".into(),
            avatar: None,
            dh: None,
        },
    );
    let cut = w.store.decision_cut(w.space, MEMBER).unwrap();
    assert_eq!(cut, 1, "отметка — по непрерывному началу лога");
    w.put(
        OWNER,
        5,
        EventKind::MemberRemove {
            member: MEMBER,
            cut,
        },
    );
    let sneaky = w.put_at(
        MEMBER,
        2,
        6,
        EventKind::Message {
            channel,
            body: "я ещё здесь".into(),
            reply_to: None,
            thread: None,
            attachments: Vec::new(),
        },
    );
    assert!(w.store.message(sneaky).unwrap().is_none());
}

#[test]
fn removal_then_promotion_ends_the_same_everywhere() {
    // Порядок А: админ исключил участника, потом владелец сделал его админом.
    let (mut a, channel) = World::founded();
    a.grant(3, ADMIN, true);
    a.say(MEMBER, 4, channel);
    let removal = EventKind::MemberRemove {
        member: MEMBER,
        cut: 1,
    };
    let promotion = EventKind::RoleSet {
        member: MEMBER,
        admin: true,
        cut: 1,
    };
    a.put(ADMIN, 5, removal.clone());
    a.put(OWNER, 6, promotion.clone());

    // Порядок Б: те же события, назначение доехало раньше.
    let (mut b, channel) = World::founded();
    b.grant(3, ADMIN, true);
    b.say(MEMBER, 4, channel);
    b.seqs.insert(OWNER, 3);
    b.put(OWNER, 6, promotion);
    b.put(ADMIN, 5, removal);

    assert_eq!(
        a.store.is_removed(a.space, MEMBER).unwrap(),
        b.store.is_removed(b.space, MEMBER).unwrap(),
        "один и тот же набор событий — один итог"
    );
    assert!(
        !a.store.is_removed(a.space, MEMBER).unwrap(),
        "админа исключает только владелец"
    );
}
