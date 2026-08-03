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

use super::wire::Presence;

/// Сколько присутствие считается свежим. Удар сердца идёт раз в 10 секунд,
/// так что три пропуска подряд — надёжный признак, что узла больше нет.
const PRESENCE_TTL_MS: i64 = 35_000;

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
}

pub struct Ctx {
    pub store: Arc<Store>,
    pub identity: Identity,
    /// Пространства, в которых мы состоим, вместе с их ключами.
    pub spaces: RwLock<HashMap<SpaceId, Space>>,
    /// Логические часы Лампорта — общие на все пространства.
    pub clock: Mutex<Clock>,
    /// Присутствие живёт только в памяти и умирает вместе с процессом.
    pub presence: RwLock<HashMap<SpaceId, HashMap<Id, Presence>>>,
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
        {
            let mut all = self.presence.write();
            let per_space = all.entry(space).or_default();
            let previous = per_space.insert(author, presence);
            // Перерисовку дёргаем, только если состав или голосовой канал
            // изменились: удары сердца идут каждые десять секунд и сами по себе
            // ничего нового не сообщают.
            let same = previous
                .map(|old| {
                    old.voice == per_space[&author].voice && old.nick == per_space[&author].nick
                })
                .unwrap_or(false);
            if same {
                return;
            }
        }
        let _ = self.notices.send(Notice::Presence { space });
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
            .filter(|p| p.ts >= deadline)
            .map(|p| p.nick.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| author.short())
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
            .map(|m| m.values().filter(|p| p.ts >= deadline).cloned().collect())
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
            people.retain(|_, p| p.ts >= deadline);
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
        ctx.note_presence(space, presence(1, now_ms() - 60_000));
        assert!(
            ctx.presence_of(space).is_empty(),
            "пропавший узел не должен вечно висеть в сети и в звонке"
        );
    }

    #[test]
    fn sweep_reports_only_spaces_it_changed() {
        let ctx = ctx();
        let space = Id([3u8; 32]);
        ctx.note_presence(space, presence(1, now_ms() - 60_000));
        ctx.note_presence(space, presence(2, now_ms()));

        assert_eq!(ctx.sweep_presence(), vec![space], "протухшее было убрано");
        assert!(
            ctx.sweep_presence().is_empty(),
            "второй проход менять нечего"
        );
        assert_eq!(ctx.presence_of(space).len(), 1, "живой участник остался");
    }
}
