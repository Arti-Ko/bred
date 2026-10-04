//! Управление пространством: владелец, ключи эпох и приглашения без ключа.
//!
//! Сервера, который помнил бы, кто здесь главный, нет. Поэтому всё, что
//! касается власти, должно проверяться любым участником самостоятельно и
//! одинаково:
//!
//! - **владелец** доказуем: идентификатор пространства — производная от его
//!   ключа и случайной соли, и назваться владельцем чужого пространства нельзя;
//! - **новый ключ** раздаётся каждому оставшемуся участнику отдельно, закрытым
//!   его ключом согласования, — исключённый его не получает;
//! - **приглашение** несёт не ключ пространства, а одноразовый секрет. В логе
//!   лежит только его хеш: по нему любой участник проверит гостя и выдаст ему
//!   ключ, а сам секрет из лога не восстановить.

use chacha20poly1305::{aead::Aead, aead::Payload, KeyInit, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};

use super::{EventId, Id, SpaceId};

/// Идентификатор пространства с доказуемым владельцем.
pub fn founded_id(owner: Id, nonce: &[u8; 32]) -> SpaceId {
    let mut hasher = blake3::Hasher::new_derive_key("bred space id v1");
    hasher.update(&owner.0);
    hasher.update(nonce);
    Id(*hasher.finalize().as_bytes())
}

/// Отпечаток ключа пространства: по нему каждый проверит, что получил тот же
/// ключ, что и остальные. Сам ключ из отпечатка не восстановить.
pub fn key_check(key: &[u8; 32]) -> Id {
    Id(blake3::derive_key("bred key check v1", key))
}

/// Что из секрета приглашения попадает в лог.
pub fn invite_proof(secret: &[u8; 32]) -> Id {
    Id(blake3::derive_key("bred invite proof v1", secret))
}

/// Новый ключ пространства, закрытый для одного участника.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyWrap {
    pub member: Id,
    /// nonce XChaCha (24 байта) и шифротекст ключа.
    pub sealed: Vec<u8>,
}

/// Кому раздаём ключ: участник и публичная половина его ключа согласования.
pub struct Recipient {
    pub member: Id,
    pub dh: Id,
}

fn wrap_key(shared: &[u8; 32], ephemeral: Id, dh: Id, space: SpaceId, epoch: u64) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key("bred key wrap v1");
    hasher.update(shared);
    hasher.update(&ephemeral.0);
    hasher.update(&dh.0);
    hasher.update(&space.0);
    hasher.update(&epoch.to_le_bytes());
    *hasher.finalize().as_bytes()
}

fn aad(space: SpaceId, epoch: u64, member: Id) -> Vec<u8> {
    let mut out = Vec::with_capacity(72);
    out.extend_from_slice(&space.0);
    out.extend_from_slice(&epoch.to_le_bytes());
    out.extend_from_slice(&member.0);
    out
}

/// Разложить новый ключ по участникам.
///
/// Одноразовая пара X25519 на всю раздачу: общий секрет с каждым участником
/// свой, а постоянный ключ раздающего в нём не участвует — его кража потом не
/// раскроет, что именно раздавали.
pub fn wrap(
    space: SpaceId,
    epoch: u64,
    key: &[u8; 32],
    recipients: &[Recipient],
) -> (Id, Vec<KeyWrap>) {
    let secret = fresh_secret();
    let ephemeral = Id(x25519_dalek::PublicKey::from(&secret).to_bytes());
    let wraps = recipients
        .iter()
        .filter_map(|to| {
            let shared = secret
                .diffie_hellman(&x25519_dalek::PublicKey::from(to.dh.0))
                .to_bytes();
            let cipher =
                XChaCha20Poly1305::new(&wrap_key(&shared, ephemeral, to.dh, space, epoch).into());
            let nonce: [u8; 24] = rand::random();
            let body = cipher
                .encrypt(
                    XNonce::from_slice(&nonce),
                    Payload {
                        msg: key,
                        aad: &aad(space, epoch, to.member),
                    },
                )
                .ok()?;
            let mut sealed = nonce.to_vec();
            sealed.extend_from_slice(&body);
            Some(KeyWrap {
                member: to.member,
                sealed,
            })
        })
        .collect();
    (ephemeral, wraps)
}

/// Достать свой экземпляр ключа из раздачи. `None` — нас в ней нет.
pub fn unwrap(
    space: SpaceId,
    epoch: u64,
    ephemeral: Id,
    wraps: &[KeyWrap],
    me: Id,
    dh: &x25519_dalek::StaticSecret,
) -> Option<[u8; 32]> {
    let mine = wraps.iter().find(|w| w.member == me)?;
    if mine.sealed.len() < 24 {
        return None;
    }
    let (nonce, body) = mine.sealed.split_at(24);
    let shared = dh
        .diffie_hellman(&x25519_dalek::PublicKey::from(ephemeral.0))
        .to_bytes();
    let public = Id(x25519_dalek::PublicKey::from(dh).to_bytes());
    let cipher = XChaCha20Poly1305::new(&wrap_key(&shared, ephemeral, public, space, epoch).into());
    let plain = cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: body,
                aad: &aad(space, epoch, me),
            },
        )
        .ok()?;
    plain.as_slice().try_into().ok()
}

/// Свежий одноразовый секрет X25519.
fn fresh_secret() -> x25519_dalek::StaticSecret {
    x25519_dalek::StaticSecret::from(rand::random::<[u8; 32]>())
}

/// Ссылка-приглашение нового образца: ключа пространства в ней нет.
///
/// Утёкшая ссылка теперь не равна утёкшей переписке: она живёт до срока,
/// исчерпывается по числу входов и гасится одной кнопкой, а ключ гость
/// получает только у живого участника, который сверит секрет с логом.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ticket {
    pub space: SpaceId,
    pub name: String,
    pub invite: EventId,
    pub secret: [u8; 32],
    /// Адреса тех, кто может впустить: пригласивший и несколько недавних.
    pub bootstrap: Vec<Vec<u8>>,
}

const TICKET_PREFIX: &str = "bred://invite/";
const LEGACY_PREFIX: &str = "bred://join/";

impl Ticket {
    pub fn encode(&self) -> String {
        let raw = postcard::to_stdvec(self).expect("приглашение сериализуемо");
        format!(
            "{TICKET_PREFIX}{}",
            data_encoding::BASE32_NOPAD
                .encode(&raw)
                .to_ascii_lowercase()
        )
    }

    pub fn decode(text: &str) -> anyhow::Result<Self> {
        let text = text.trim();
        if text.starts_with(LEGACY_PREFIX) {
            anyhow::bail!(
                "это ссылка старого образца: в ней лежит сам ключ пространства, и такие больше не принимаются. Попросите новую"
            );
        }
        let body = text
            .strip_prefix(TICKET_PREFIX)
            .ok_or_else(|| anyhow::anyhow!("ссылка должна начинаться с {TICKET_PREFIX}"))?;
        let raw = data_encoding::BASE32_NOPAD
            .decode(body.to_ascii_uppercase().as_bytes())
            .map_err(|_| anyhow::anyhow!("повреждённая ссылка-приглашение"))?;
        Ok(postcard::from_bytes(&raw)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dh() -> (x25519_dalek::StaticSecret, Id) {
        let secret = fresh_secret();
        let public = Id(x25519_dalek::PublicKey::from(&secret).to_bytes());
        (secret, public)
    }

    #[test]
    fn owner_is_bound_to_space_id() {
        let nonce = [7u8; 32];
        let owner = Id([1u8; 32]);
        let space = founded_id(owner, &nonce);
        assert_eq!(space, founded_id(owner, &nonce));
        assert_ne!(
            space,
            founded_id(Id([2u8; 32]), &nonce),
            "чужой ключ — чужое пространство"
        );
    }

    #[test]
    fn each_member_unwraps_only_own_copy() {
        let space = Id([3u8; 32]);
        let key = [42u8; 32];
        let (alice_dh, alice_pub) = dh();
        let (boris_dh, boris_pub) = dh();
        let (_, vera_pub) = dh();
        let alice = Id([10u8; 32]);
        let boris = Id([11u8; 32]);
        let vera = Id([12u8; 32]);

        let (ephemeral, wraps) = wrap(
            space,
            1,
            &key,
            &[
                Recipient {
                    member: alice,
                    dh: alice_pub,
                },
                Recipient {
                    member: boris,
                    dh: boris_pub,
                },
            ],
        );
        assert_eq!(
            unwrap(space, 1, ephemeral, &wraps, alice, &alice_dh),
            Some(key)
        );
        assert_eq!(
            unwrap(space, 1, ephemeral, &wraps, boris, &boris_dh),
            Some(key)
        );
        // Веру не позвали — её копии нет.
        assert!(wraps.iter().all(|w| w.member != vera));
        let _ = vera_pub;
        // Чужой экземпляр своим ключом не открыть.
        assert_eq!(unwrap(space, 1, ephemeral, &wraps, alice, &boris_dh), None);
        // Раздачу нельзя переложить в другую эпоху или пространство.
        assert_eq!(unwrap(space, 2, ephemeral, &wraps, alice, &alice_dh), None);
        assert_eq!(
            unwrap(Id([4u8; 32]), 1, ephemeral, &wraps, alice, &alice_dh),
            None
        );
    }

    #[test]
    fn copy_moved_to_another_member_does_not_open() {
        let space = Id([3u8; 32]);
        let (alice_dh, alice_pub) = dh();
        let alice = Id([10u8; 32]);
        let mallory = Id([13u8; 32]);
        let (ephemeral, mut wraps) = wrap(
            space,
            1,
            &[1u8; 32],
            &[Recipient {
                member: alice,
                dh: alice_pub,
            }],
        );
        wraps[0].member = mallory;
        assert_eq!(
            unwrap(space, 1, ephemeral, &wraps, mallory, &alice_dh),
            None
        );
    }

    #[test]
    fn ticket_round_trips_and_carries_no_key() {
        let ticket = Ticket {
            space: Id([4u8; 32]),
            name: "кухня".into(),
            invite: Id([5u8; 32]),
            secret: [6u8; 32],
            bootstrap: vec![vec![1, 2, 3]],
        };
        let text = ticket.encode();
        assert!(text.starts_with("bred://invite/"));
        assert_eq!(Ticket::decode(&text).unwrap(), ticket);
        assert_ne!(invite_proof(&ticket.secret).0, ticket.secret);
    }

    #[test]
    fn legacy_link_is_refused_with_explanation() {
        let err = Ticket::decode("bred://join/abc").unwrap_err().to_string();
        assert!(err.contains("старого образца"), "{err}");
    }
}
