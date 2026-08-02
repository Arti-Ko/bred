//! Доменное ядро: идентификаторы, лог событий, пространства и приглашения.
//! Слой не знает ни про сеть, ни про базу — только правила.

pub mod event;
pub mod ids;

pub use event::{now_ms, Attachment, Clock, Event, EventKind, SignedEvent};
pub use ids::{AuthorId, ChannelId, EventId, Id, SpaceId};

use serde::{Deserialize, Serialize};

/// Пространство — то, что в Дискорде называется сервером.
/// Здесь это просто общий секрет: кто знает ключ, тот участник.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Space {
    pub id: SpaceId,
    pub name: String,
    /// Личная переписка на двоих. Отличается от обычного пространства только
    /// тем, как показывается и как заводится: ключ выводится, а не раздаётся.
    #[serde(default)]
    pub direct: Option<Id>,
    /// Симметричный ключ пространства. Им шифруется всё, что уходит в сеть,
    /// поэтому релеи и случайные соседи по локалке видят только шум.
    pub key: [u8; 32],
}

impl Space {
    /// Топик gossip-роя. Это не сам `space.id`: идентификатор пространства
    /// не должен утекать в открытую сеть, поэтому в топик идёт производная.
    pub fn topic(&self) -> [u8; 32] {
        derive(&self.key, "bred/topic")
    }

    /// Метка для локального маячка: свои узнают, чужие видят случайные байты.
    pub fn lan_tag(&self) -> [u8; 32] {
        derive(&self.key, "bred/lan")
    }
}

/// Приглашение в пространство. Ровно этого достаточно, чтобы подключиться:
/// сервера нет, поэтому «ссылка-инвайт» переносит и ключ, и точки входа.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invite {
    pub space: SpaceId,
    pub name: String,
    pub key: [u8; 32],
    /// Кого пробовать первым. Не обязательно живы — сеть переоткроет пиров сама.
    pub bootstrap: Vec<Vec<u8>>,
}

impl Invite {
    pub fn encode(&self) -> String {
        let raw = postcard::to_stdvec(self).expect("приглашение сериализуемо");
        format!(
            "bred://join/{}",
            data_encoding::BASE32_NOPAD
                .encode(&raw)
                .to_ascii_lowercase()
        )
    }

    pub fn decode(text: &str) -> anyhow::Result<Self> {
        let body = text
            .trim()
            .strip_prefix("bred://join/")
            .ok_or_else(|| anyhow::anyhow!("ссылка должна начинаться с bred://join/"))?;
        let raw = data_encoding::BASE32_NOPAD
            .decode(body.to_ascii_uppercase().as_bytes())
            .map_err(|_| anyhow::anyhow!("повреждённая ссылка-приглашение"))?;
        Ok(postcard::from_bytes(&raw)?)
    }
}

/// Адрес и ключ личной переписки.
///
/// Идентификатор считается из двух публичных ключей в отсортированном порядке —
/// значит обе стороны получают один и тот же, не сговариваясь. Ключ выводится
/// из общего секрета X25519, поэтому знание публичных ключей (а они публичны)
/// ничего не даёт постороннему.
pub fn direct_space(me: Id, peer: Id, shared: [u8; 32]) -> Space {
    let (first, second) = if me.0 <= peer.0 {
        (me, peer)
    } else {
        (peer, me)
    };

    let mut hasher = blake3::Hasher::new_derive_key("bred/direct/id");
    hasher.update(&first.0);
    hasher.update(&second.0);
    let id = Id(*hasher.finalize().as_bytes());

    Space {
        id,
        name: String::new(),
        direct: Some(peer),
        key: derive(&shared, "bred/direct/key"),
    }
}

/// Производный ключ: blake3 в режиме key derivation.
pub fn derive(key: &[u8; 32], context: &str) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(context);
    hasher.update(key);
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space() -> Space {
        Space {
            id: Id([1u8; 32]),
            name: "тест".into(),
            key: [9u8; 32],
            direct: None,
        }
    }

    #[test]
    fn topic_does_not_leak_space_id_or_key() {
        let s = space();
        assert_ne!(s.topic(), s.id.0);
        assert_ne!(s.topic(), s.key);
        assert_ne!(s.topic(), s.lan_tag());
    }

    #[test]
    fn invite_round_trips() {
        let inv = Invite {
            space: Id([4u8; 32]),
            name: "Орбита".into(),
            key: [7u8; 32],
            bootstrap: vec![vec![1, 2, 3]],
        };
        let restored = Invite::decode(&inv.encode()).unwrap();
        assert_eq!(restored.space, inv.space);
        assert_eq!(restored.key, inv.key);
        assert_eq!(restored.name, "Орбита");
    }

    #[test]
    fn invite_rejects_garbage() {
        assert!(Invite::decode("https://example.com").is_err());
        assert!(Invite::decode("bred://join/%%%").is_err());
    }
}

#[cfg(test)]
mod direct_tests {
    use super::*;

    #[test]
    fn both_sides_derive_the_same_room() {
        let alice = Id([1u8; 32]);
        let boris = Id([2u8; 32]);
        let shared = [42u8; 32];

        // Ключ у обеих сторон один — его даёт X25519, а не обмен сообщениями.
        let mine = direct_space(alice, boris, shared);
        let theirs = direct_space(boris, alice, shared);

        assert_eq!(mine.id, theirs.id, "адрес переписки обязан совпасть");
        assert_eq!(mine.key, theirs.key, "и ключ тоже");
        assert_eq!(mine.direct, Some(boris));
        assert_eq!(theirs.direct, Some(alice));
    }

    #[test]
    fn different_pairs_never_collide() {
        let a = direct_space(Id([1u8; 32]), Id([2u8; 32]), [9u8; 32]);
        let b = direct_space(Id([1u8; 32]), Id([3u8; 32]), [9u8; 32]);
        assert_ne!(a.id, b.id, "у разных пар должны быть разные комнаты");
    }

    #[test]
    fn public_keys_alone_do_not_reveal_the_key() {
        // Посторонний знает оба публичных ключа, но не общий секрет.
        let honest = direct_space(Id([1u8; 32]), Id([2u8; 32]), [7u8; 32]);
        let guess = direct_space(Id([1u8; 32]), Id([2u8; 32]), [0u8; 32]);
        assert_eq!(honest.id, guess.id, "адрес и правда выводится из ключей");
        assert_ne!(
            honest.key, guess.key,
            "а вот ключ — только из общего секрета"
        );
    }
}
