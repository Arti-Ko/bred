//! Личность пользователя.
//!
//! В БРЕД нет регистрации: личность — это пара ключей ed25519, которая живёт
//! на устройстве. Публичный ключ одновременно служит адресом в сети iroh и
//! идентификатором автора в логе событий, поэтому подделать авторство нельзя.

use anyhow::{Context, Result};
use iroh::SecretKey;

use crate::{domain::Id, store::Store};

const SECRET_KEY_SETTING: &str = "identity.secret_key";
const NICK_SETTING: &str = "identity.nick";
const AVATAR_SETTING: &str = "identity.avatar";
const DH_SETTING: &str = "identity.dh";

#[derive(Clone)]
pub struct Identity {
    secret: SecretKey,
}

impl Identity {
    /// Загружает ключ из базы, а при первом запуске создаёт новый.
    pub fn load_or_create(store: &Store) -> Result<Self> {
        if let Some(raw) = store.get_setting(SECRET_KEY_SETTING)? {
            let bytes: [u8; 32] = raw
                .as_slice()
                .try_into()
                .context("сохранённый ключ повреждён")?;
            return Ok(Self {
                secret: SecretKey::from_bytes(&bytes),
            });
        }

        let secret = SecretKey::generate();
        store.set_setting(SECRET_KEY_SETTING, &secret.to_bytes())?;
        Ok(Self { secret })
    }

    pub fn secret(&self) -> &SecretKey {
        &self.secret
    }

    /// Публичный ключ как доменный идентификатор.
    pub fn id(&self) -> Id {
        Id(*self.secret.public().as_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.secret.sign(message).to_bytes()
    }
}

/// Ключ согласования (X25519) для личных переписок.
///
/// Отдельная пара от подписной: смешивать подпись и обмен ключами — известный
/// способ прострелить себе ногу. Публичная половина уезжает в профиле, и этого
/// достаточно, чтобы обе стороны вычислили общий секрет **не обмениваясь
/// ничем**: ни сервера, ни приглашения для личной переписки не нужно.
pub fn load_or_create_dh(store: &Store) -> Result<x25519_dalek::StaticSecret> {
    if let Some(raw) = store.get_setting(DH_SETTING)? {
        let bytes: [u8; 32] = raw.as_slice().try_into().context("ключ обмена повреждён")?;
        return Ok(x25519_dalek::StaticSecret::from(bytes));
    }
    let mut bytes = [0u8; 32];
    rand::Rng::fill(&mut rand::rng(), &mut bytes[..]);
    let secret = x25519_dalek::StaticSecret::from(bytes);
    store.set_setting(DH_SETTING, &secret.to_bytes())?;
    Ok(secret)
}

pub fn dh_public(secret: &x25519_dalek::StaticSecret) -> Id {
    Id(x25519_dalek::PublicKey::from(secret).to_bytes())
}

/// Общий секрет с собеседником. Обе стороны получают одно и то же значение.
pub fn shared_secret(mine: &x25519_dalek::StaticSecret, theirs: Id) -> [u8; 32] {
    mine.diffie_hellman(&x25519_dalek::PublicKey::from(theirs.0))
        .to_bytes()
}

/// Проверка подписи по идентификатору автора.
pub fn verify(author: Id, message: &[u8], sig: &[u8; 64]) -> bool {
    let Ok(key) = iroh::PublicKey::from_bytes(&author.0) else {
        return false;
    };
    let signature = iroh::Signature::from_bytes(sig);
    key.verify(message, &signature).is_ok()
}

/// Отображаемое имя. По умолчанию — короткая форма ключа, чтобы в списке
/// участников никогда не было пустых строк.
pub fn load_nick(store: &Store, id: Id) -> Result<String> {
    match store.get_setting(NICK_SETTING)? {
        Some(raw) => Ok(String::from_utf8_lossy(&raw).to_string()),
        None => Ok(id.short()),
    }
}

pub fn save_nick(store: &Store, nick: &str) -> Result<()> {
    store.set_setting(NICK_SETTING, nick.as_bytes())
}

/// Хеш картинки профиля. Хранится локально, а в пространства уезжает
/// событием `Profile` — по одному на каждое, где мы состоим.
pub fn load_avatar(store: &Store) -> Result<Option<Id>> {
    Ok(store
        .get_setting(AVATAR_SETTING)?
        .and_then(|raw| Id::from_slice(&raw)))
}

pub fn save_avatar(store: &Store, avatar: Id) -> Result<()> {
    store.set_setting(AVATAR_SETTING, &avatar.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_persists_between_loads() {
        let store = Store::in_memory().unwrap();
        let first = Identity::load_or_create(&store).unwrap();
        let second = Identity::load_or_create(&store).unwrap();
        assert_eq!(first.id(), second.id(), "ключ должен переживать перезапуск");
    }

    #[test]
    fn signature_verifies_for_its_author() {
        let store = Store::in_memory().unwrap();
        let me = Identity::load_or_create(&store).unwrap();
        let sig = me.sign("привет".as_bytes());
        assert!(verify(me.id(), "привет".as_bytes(), &sig));
    }

    #[test]
    fn signature_fails_on_tampered_message() {
        let store = Store::in_memory().unwrap();
        let me = Identity::load_or_create(&store).unwrap();
        let sig = me.sign("привет".as_bytes());
        assert!(!verify(me.id(), "пока".as_bytes(), &sig));
    }

    #[test]
    fn signature_fails_for_other_author() {
        let store = Store::in_memory().unwrap();
        let me = Identity::load_or_create(&store).unwrap();
        let other = Store::in_memory().unwrap();
        let other = Identity::load_or_create(&other).unwrap();
        let sig = me.sign("привет".as_bytes());
        assert!(!verify(other.id(), "привет".as_bytes(), &sig));
    }

    #[test]
    fn nick_defaults_to_short_key() {
        let store = Store::in_memory().unwrap();
        let me = Identity::load_or_create(&store).unwrap();
        assert_eq!(load_nick(&store, me.id()).unwrap(), me.id().short());
        save_nick(&store, "тимур").unwrap();
        assert_eq!(load_nick(&store, me.id()).unwrap(), "тимур");
    }
}
