//! Общий контекст сетевого слоя: то, к чему обращаются и gossip, и
//! досинхронизация, и локальный маячок.

use anyhow::Result;
use parking_lot::{Mutex, RwLock};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    domain::{Clock, Id, SignedEvent, Space, SpaceId},
    identity::{verify, Identity},
    store::{Applied, Store},
};

use super::wire::{PlayerState, Presence};

/// Сколько живёт состояние общего плеера без подтверждения.
///
/// Втрое короче присутствия: ведущий подтверждает его каждые две секунды, и
/// панель плеера, висящая после того, как трансляцию выключили, — это кнопки,
/// которые ничего не делают.
const PLAYER_TTL_MS: i64 = 6_000;

/// Присутствие соседа вместе с моментом, когда мы его услышали.
///
/// Момент нужен свой: срок годности отсчитывается по нашим часам, потому что
/// чужие с нашими не сверены и сверить их нечем.
#[derive(Debug, Clone)]
pub struct Seen {
    pub presence: Presence,
    /// Когда мы приняли этот удар сердца, по нашим часам.
    pub at: i64,
    /// Когда в последний раз записали этого соседа в базу.
    pub persisted: i64,
}

/// Сколько присутствие считается свежим. Удар сердца идёт раз в три секунды,
/// так что четыре пропуска подряд — надёжный признак, что узла больше нет.
///
/// Считается по **нашим** часам, а не по отметке отправителя. Отметка ехала из
/// чужой системы, где время может отличаться на минуты: с коротким сроком
/// годности сосед с отстающими часами не появился бы в сети никогда, а с
/// забегающими — не исчез бы вовсе.
const PRESENCE_TTL_MS: i64 = 12_000;

/// Как часто удар сердца доходит до диска.
///
/// Сам он приходит раз в три секунды, но писать его в базу каждый раз незачем:
/// на диске от этого меняется только «был в сети», и полминуты точности там
/// более чем достаточно. Всё, что меняется по существу — имя или адрес, —
/// записывается сразу, не дожидаясь срока.
const PERSIST_INTERVAL_MS: i64 = 30_000;

/// Уведомление наверх — в UI.
#[derive(Debug, Clone)]
pub enum Notice {
    /// Событие применено к логу; в UI надо обновить канал.
    Applied { space: SpaceId, event: Id },
    /// Изменился состав присутствующих.
    Presence { space: SpaceId },
    /// Кто-то печатает в канале. Эфемерное, в лог не попадает.
    Typing {
        space: SpaceId,
        channel: Id,
        author: Id,
        nick: String,
    },
    /// Автор выдал два разных события с одним номером.
    Fork { space: SpaceId, author: Id },
    /// Собеседник на другой версии формата: его события мы прочитать не можем.
    Version {
        space: SpaceId,
        theirs: u16,
        ours: u16,
    },
    /// Изменилось состояние сети (число соседей, адрес).
    Net,
    /// Изменилось состояние общего плеера в пространстве.
    Player { space: SpaceId },
    /// Нам, как ведущему, нажали кнопку на пульте.
    PlayerCommand { command: super::wire::PlayerCommand },
    /// Кодировщику пора выдать ключевой кадр.
    ///
    /// В звонке появился новый собеседник, а декодер не начинает работу, пока
    /// не увидит ключевой. По расписанию тот приходит раз в несколько секунд —
    /// ровно столько новичок и смотрел на чёрный прямоугольник.
    Keyframe,
    /// Подобранный битрейт дорожки картинки.
    ///
    /// Считается по тому, сколько кадров пришлось выбросить из очередей
    /// отправки, и делится на число собеседников: в сетке каждый из них стоит
    /// отдельной копии кадра.
    Bitrate { track: &'static str, bps: u32 },
}

pub struct Ctx {
    pub store: Arc<Store>,
    pub identity: Identity,
    /// Пространства, в которых мы состоим, вместе с их ключами.
    pub spaces: RwLock<HashMap<SpaceId, Space>>,
    /// Логические часы Лампорта — общие на все пространства.
    pub clock: Mutex<Clock>,
    /// Присутствие живёт только в памяти и умирает вместе с процессом.
    pub presence: RwLock<HashMap<SpaceId, HashMap<Id, Seen>>>,
    /// Общий плеер — по одному на пространство: комнат много, но ведущий,
    /// который транслирует звук, обычно один, и путать их незачем.
    pub players: RwLock<HashMap<SpaceId, PlayerState>>,
    pub notices: UnboundedSender<Notice>,
    /// Куда складываются байты вложений.
    blob_dir: PathBuf,
}

impl Ctx {
    pub fn new(
        store: Arc<Store>,
        identity: Identity,
        spaces: Vec<Space>,
        clock: Clock,
        notices: UnboundedSender<Notice>,
        blob_dir: impl AsRef<Path>,
    ) -> Self {
        let map = spaces.into_iter().map(|s| (s.id, s)).collect();
        Self {
            store,
            identity,
            spaces: RwLock::new(map),
            clock: Mutex::new(clock),
            presence: RwLock::new(HashMap::new()),
            players: RwLock::new(HashMap::new()),
            notices,
            blob_dir: blob_dir.as_ref().to_path_buf(),
        }
    }

    /// Путь, по которому лежит (или будет лежать) вложение с таким хешем.
    pub fn blob_path(&self, hash: Id) -> PathBuf {
        self.blob_dir.join(hash.to_string())
    }

    /// Расшифровать кадр, перебрав ключи известных пространств.
    ///
    /// Идентификатор пространства не едет по проводу открытым текстом, поэтому
    /// принимающая сторона подбирает ключ сама. Заодно это отсекает чужаков:
    /// без ключа кадр просто не расшифруется.
    pub fn open_with_known_key<T: for<'de> serde::Deserialize<'de>>(
        &self,
        raw: &[u8],
    ) -> Option<(SpaceId, T)> {
        self.space_list().into_iter().find_map(|space| {
            super::wire::open::<T>(&space.key, raw)
                .ok()
                .map(|v| (space.id, v))
        })
    }

    pub fn space(&self, id: SpaceId) -> Option<Space> {
        self.spaces.read().get(&id).cloned()
    }

    pub fn space_list(&self) -> Vec<Space> {
        self.spaces.read().values().cloned().collect()
    }

    pub fn add_space(&self, space: Space) {
        self.spaces.write().insert(space.id, space);
    }

    /// Единственная точка входа для чужих событий.
    ///
    /// Проверяет подпись, двигает часы и пишет в лог. Возвращает `true`, если
    /// событие оказалось новым — дубликаты приходят постоянно, потому что одно
    /// и то же событие прилетает и через gossip, и через досинхронизацию.
    pub fn apply(&self, signed: &SignedEvent) -> Result<bool> {
        let bytes = signed.event.canonical_bytes();
        if !verify(signed.event.author, &bytes, &signed.sig) {
            tracing::warn!(author = %signed.event.author.short(), "подпись не сошлась, событие отброшено");
            return Ok(false);
        }
        if self.space(signed.event.space).is_none() {
            // Событие из пространства, в котором мы не состоим.
            return Ok(false);
        }

        self.clock.lock().observe(signed.event.lamport);
        match self.store.apply(signed)? {
            Applied::Fresh => {
                let _ = self.notices.send(Notice::Applied {
                    space: signed.event.space,
                    event: signed.id(),
                });
                Ok(true)
            }
            Applied::Duplicate => Ok(false),
            Applied::Fork => {
                // Молчать здесь нельзя: узел либо сломан, либо переписывает
                // собственную историю. Факт зафиксирован, людям это видно.
                tracing::warn!(
                    author = %signed.event.author.short(),
                    seq = signed.event.seq,
                    "раздвоение лога: событие отвергнуто"
                );
                let _ = self.notices.send(Notice::Fork {
                    space: signed.event.space,
                    author: signed.event.author,
                });
                Ok(false)
            }
        }
    }

    /// Пакетное применение — при досинхронизации событий приезжает много.
    pub fn apply_batch(&self, events: &[SignedEvent]) -> Result<usize> {
        let mut fresh = 0;
        for signed in events {
            if self.apply(signed)? {
                fresh += 1;
            }
        }
        Ok(fresh)
    }

    pub fn note_presence(&self, space: SpaceId, presence: Presence) {
        let author = presence.author;
        let now = crate::domain::now_ms();

        let (changed, persist) = {
            let mut all = self.presence.write();
            let per_space = all.entry(space).or_default();
            let previous = per_space.get(&author);

            // Протухшую запись считаем за отсутствие: человек уже пропал из
            // списка, и его возвращение — новость, даже если имя и голосовой
            // канал у него те же. Перерисовку в остальных случаях не дёргаем:
            // удары сердца идут часто и сами по себе ничего не сообщают.
            let changed = previous.is_none_or(|old| {
                old.at < now - PRESENCE_TTL_MS
                    || old.presence.voice != presence.voice
                    || old.presence.nick != presence.nick
            });
            let persist = previous.is_none_or(|old| {
                old.presence.addr != presence.addr
                    || old.presence.nick != presence.nick
                    || now - old.persisted >= PERSIST_INTERVAL_MS
            });
            let persisted = if persist {
                now
            } else {
                previous.map_or(now, |old| old.persisted)
            };

            let nick = presence.nick.clone();
            let addr = presence.addr.clone();
            per_space.insert(
                author,
                Seen {
                    presence,
                    at: now,
                    persisted,
                },
            );
            (changed, persist.then_some((nick, addr)))
        };

        // На диск — вне блокировки: держать её через запись в базу незачем.
        //
        // Человек, приехавший по ссылке, мог ещё ничего не написать: в логе его
        // нет, а значит нет и в списке участников. Присутствие — первый, а до
        // первого сообщения единственный признак, что он вообще существует.
        if let Some((nick, addr)) = persist {
            if author != self.identity.id() {
                let _ = self.store.remember_peer_seen(
                    space,
                    author,
                    &nick,
                    (!addr.is_empty()).then_some(addr.as_slice()),
                );
            }
        }

        if changed {
            let _ = self.notices.send(Notice::Presence { space });
        }
    }

    /// То же, но с заданным моментом приёма. Только для тестов: настоящий
    /// момент всегда «сейчас», и подделать его иначе нечем.
    #[cfg(test)]
    pub fn note_presence_at(&self, space: SpaceId, presence: Presence, at: i64) {
        self.presence
            .write()
            .entry(space)
            .or_default()
            .insert(
                presence.author,
                Seen {
                    presence,
                    at,
                    persisted: at,
                },
            );
    }

    pub fn drop_presence(&self, space: SpaceId, author: Id) {
        let removed = self
            .presence
            .write()
            .get_mut(&space)
            .and_then(|m| m.remove(&author))
            .is_some();
        if removed {
            let _ = self.notices.send(Notice::Presence { space });
        }
    }

    /// Отображаемое имя участника: сперва из присутствия, затем короткий ключ.
    pub fn nick_of(&self, space: SpaceId, author: Id) -> String {
        let deadline = crate::domain::now_ms() - PRESENCE_TTL_MS;
        self.presence
            .read()
            .get(&space)
            .and_then(|m| m.get(&author))
            .filter(|seen| seen.at >= deadline)
            .map(|seen| seen.presence.nick.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| author.short())
    }

    /// Запомнить состояние плеера, пришедшее от ведущего. `None` — выключили.
    pub fn note_player(&self, space: SpaceId, state: Option<PlayerState>) {
        let changed = {
            let mut all = self.players.write();
            match state {
                None => all.remove(&space).is_some(),
                Some(state) => {
                    // Подтверждения идут каждые две секунды и сами по себе
                    // ничего не сообщают. Будим интерфейс, только если что-то
                    // и правда поменялось.
                    let same = all.get(&space).is_some_and(|old| {
                        old.host == state.host
                            && old.playing == state.playing
                            && old.source == state.source
                    });
                    all.insert(space, state);
                    !same
                }
            }
        };
        if changed {
            let _ = self.notices.send(Notice::Player { space });
        }
    }

    /// Что играет в пространстве прямо сейчас.
    pub fn player_of(&self, space: SpaceId) -> Option<PlayerState> {
        let deadline = crate::domain::now_ms() - PLAYER_TTL_MS;
        self.players
            .read()
            .get(&space)
            .filter(|state| state.ts >= deadline)
            .cloned()
    }

    /// Убрать плеер, о котором давно ничего не слышно.
    pub fn sweep_players(&self) -> Vec<SpaceId> {
        let deadline = crate::domain::now_ms() - PLAYER_TTL_MS;
        let mut gone = Vec::new();
        self.players.write().retain(|space, state| {
            let alive = state.ts >= deadline;
            if !alive {
                gone.push(*space);
            }
            alive
        });
        gone
    }

    /// Кто сейчас на связи в пространстве.
    ///
    /// Протухшие записи отсеиваются: узел может исчезнуть без прощания —
    /// упасть, потерять сеть, уснуть вместе с ноутбуком. Без срока годности
    /// он навсегда остался бы «в сети» и «в звонке».
    pub fn presence_of(&self, space: SpaceId) -> Vec<Presence> {
        let deadline = crate::domain::now_ms() - PRESENCE_TTL_MS;
        self.presence
            .read()
            .get(&space)
            .map(|m| {
                m.values()
                    .filter(|seen| seen.at >= deadline)
                    .map(|seen| seen.presence.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Убирает протухшие записи из памяти. Вызывается по таймеру, а не при
    /// каждом чтении: чтение не должно требовать блокировку на запись.
    pub fn sweep_presence(&self) -> Vec<SpaceId> {
        let deadline = crate::domain::now_ms() - PRESENCE_TTL_MS;
        let mut changed = Vec::new();
        let mut all = self.presence.write();
        for (space, people) in all.iter_mut() {
            let before = people.len();
            people.retain(|_, seen| seen.at >= deadline);
            if people.len() != before {
                changed.push(*space);
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{now_ms, Id};

    fn ctx() -> Ctx {
        let store = Arc::new(Store::in_memory().unwrap());
        let identity = Identity::load_or_create(&store).unwrap();
        let space = Space {
            id: Id([3u8; 32]),
            name: "тест".into(),
            key: [3u8; 32],
            direct: None,
        };
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        std::mem::forget(rx);
        Ctx::new(
            store,
            identity,
            vec![space],
            Clock::default(),
            tx,
            std::env::temp_dir().join("bred-ctx-test"),
        )
    }

    fn presence(author: u8, ts: i64) -> Presence {
        Presence {
            author: Id([author; 32]),
            addr: Vec::new(),
            nick: "кто-то".into(),
            voice: Some(Id([9u8; 32])),
            ts,
        }
    }

    #[test]
    fn fresh_presence_is_visible() {
        let ctx = ctx();
        let space = Id([3u8; 32]);
        ctx.note_presence(space, presence(1, now_ms()));
        assert_eq!(ctx.presence_of(space).len(), 1);
    }

    #[test]
    fn stale_presence_disappears() {
        let ctx = ctx();
        let space = Id([3u8; 32]);
        // Узел исчез без прощания: последний удар сердца был минуту назад.
        ctx.note_presence_at(space, presence(1, now_ms()), now_ms() - 60_000);
        assert!(
            ctx.presence_of(space).is_empty(),
            "пропавший узел не должен вечно висеть в сети и в звонке"
        );
    }

    fn player(ts: i64) -> PlayerState {
        PlayerState {
            host: Id([9u8; 32]),
            channel: Id([4u8; 32]),
            source: "Яндекс Музыка".into(),
            playing: true,
            ts,
        }
    }

    #[test]
    fn player_state_is_remembered_and_expires() {
        let ctx = ctx();
        let space = Id([3u8; 32]);
        ctx.note_player(space, Some(player(now_ms())));
        assert!(ctx.player_of(space).is_some(), "свежее состояние читается");

        // Ведущий пропал вместе с ноутбуком: подтверждений больше нет.
        ctx.note_player(space, Some(player(now_ms() - 30_000)));
        assert!(
            ctx.player_of(space).is_none(),
            "панель с кнопками не должна висеть после исчезновения ведущего"
        );
        assert_eq!(ctx.sweep_players(), vec![space]);
    }

    #[test]
    fn player_switched_off_disappears_at_once() {
        let ctx = ctx();
        let space = Id([3u8; 32]);
        ctx.note_player(space, Some(player(now_ms())));
        // Выключили вслух — ждать, пока протухнет, было бы шесть секунд
        // кнопок, которые уже ничего не делают.
        ctx.note_player(space, None);
        assert!(ctx.player_of(space).is_none());
    }

    #[test]
    fn clock_skew_does_not_hide_a_live_neighbour() {
        let ctx = ctx();
        let space = Id([3u8; 32]);
        // Часы собеседника отстали на час — сам он при этом на связи.
        ctx.note_presence(space, presence(1, now_ms() - 3_600_000));
        assert_eq!(
            ctx.presence_of(space).len(),
            1,
            "свежесть считается по нашим часам, чужие с нашими не сверены"
        );
    }

    #[test]
    fn return_after_expiry_is_reported() {
        let ctx = ctx();
        let space = Id([3u8; 32]);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let ctx = Ctx::new(
            ctx.store.clone(),
            Identity::load_or_create(&ctx.store).unwrap(),
            vec![Space {
                id: space,
                name: "тест".into(),
                key: [3u8; 32],
                direct: None,
            }],
            Clock::default(),
            tx,
            std::env::temp_dir().join("bred-ctx-test"),
        );

        ctx.note_presence_at(space, presence(1, now_ms()), now_ms() - 60_000);
        ctx.note_presence(space, presence(1, now_ms()));
        assert!(
            rx.try_recv().is_ok(),
            "вернувшийся участник обязан перерисовать список, даже если имя и канал те же"
        );
    }

    #[test]
    fn sweep_reports_only_spaces_it_changed() {
        let ctx = ctx();
        let space = Id([3u8; 32]);
        ctx.note_presence_at(space, presence(1, now_ms()), now_ms() - 60_000);
        ctx.note_presence(space, presence(2, now_ms()));

        assert_eq!(ctx.sweep_presence(), vec![space], "протухшее было убрано");
        assert!(
            ctx.sweep_presence().is_empty(),
            "второй проход менять нечего"
        );
        assert_eq!(ctx.presence_of(space).len(), 1, "живой участник остался");
    }
}
