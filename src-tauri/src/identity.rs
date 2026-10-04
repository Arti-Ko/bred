//! Личность пользователя.
//!
//! В БРЕД нет регистрации: личность — это пара ключей ed25519, которая живёт
//! на устройстве. Публичный ключ одновременно служит адресом в сети iroh и
//! идентификатором автора в логе событий, поэтому подделать авторство нельзя.

use anyhow::{Context, Result};
use iroh::SecretKey;

use crate::{
    domain::{
        account::{AccountId, DeviceCert, DeviceRevoke},
        Id,
    },
    store::Store,
};

const SECRET_KEY_SETTING: &str = "identity.secret_key";
const ACCOUNT_SETTING: &str = "account.secret_key";
const CERT_SETTING: &str = "account.device_cert";
const NICK_SETTING: &str = "identity.nick";
const AVATAR_SETTING: &str = "identity.avatar";
const DH_SETTING: &str = "identity.dh";

#[derive(Clone)]
pub struct Identity {
    secret: SecretKey,
    /// Ключ согласования: личные переписки и раздачи ключей пространств.
    dh: x25519_dalek::StaticSecret,
}

impl Identity {
    /// Загружает ключ из базы, а при первом запуске создаёт новый.
    pub fn load_or_create(store: &Store) -> Result<Self> {
        let dh = load_or_create_dh(store)?;
        if let Some(raw) = store.get_setting(SECRET_KEY_SETTING)? {
            let bytes: [u8; 32] = raw
                .as_slice()
                .try_into()
                .context("сохранённый ключ повреждён")?;
            return Ok(Self {
                secret: SecretKey::from_bytes(&bytes),
                dh,
            });
        }

        let secret = SecretKey::generate();
        store.set_setting(SECRET_KEY_SETTING, &secret.to_bytes())?;
        Ok(Self { secret, dh })
    }

    pub fn secret(&self) -> &SecretKey {
        &self.secret
    }

    pub fn dh(&self) -> &x25519_dalek::StaticSecret {
        &self.dh
    }

    /// Публичный ключ как доменный идентификатор.
    pub fn id(&self) -> Id {
        Id(*self.secret.public().as_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.secret.sign(message).to_bytes()
    }
}

/// Ключ аккаунта — общий для всех устройств человека.
///
/// Им подписывается ровно одно: «это устройство — моё». События по-прежнему
/// подписывает ключ устройства: так потерянный телефон отзывается одной
/// подписью, без смены ключей у всех остальных. См. `domain::account`.
#[derive(Clone)]
pub struct Account {
    secret: SecretKey,
}

impl Account {
    /// Загружает ключ аккаунта, а при первом запуске заводит новый.
    ///
    /// У тех, кто ставил БРЕД до аккаунтов, ключ появится при первом запуске
    /// новой версии: их единственное устройство становится первым устройством
    /// нового аккаунта, и ничего из прежнего не меняется.
    pub fn load_or_create(store: &Store) -> Result<Self> {
        if let Some(raw) = store.get_setting(ACCOUNT_SETTING)? {
            let bytes: [u8; 32] = raw
                .as_slice()
                .try_into()
                .context("сохранённый ключ аккаунта повреждён")?;
            return Ok(Self {
                secret: SecretKey::from_bytes(&bytes),
            });
        }
        let secret = SecretKey::generate();
        store.set_setting(ACCOUNT_SETTING, &secret.to_bytes())?;
        Ok(Self { secret })
    }

    /// Ключ аккаунта, полученный при привязке нового устройства.
    pub fn adopt(store: &Store, secret: [u8; 32]) -> Result<Self> {
        store.set_setting(ACCOUNT_SETTING, &secret)?;
        store.delete_setting(CERT_SETTING)?;
        Ok(Self {
            secret: SecretKey::from_bytes(&secret),
        })
    }

    pub fn id(&self) -> AccountId {
        Id(*self.secret.public().as_bytes())
    }

    /// Сырой ключ — только для передачи новому устройству при привязке.
    pub fn secret_bytes(&self) -> [u8; 32] {
        self.secret.to_bytes()
    }

    /// Удостоверить устройство: «оно моё».
    pub fn certify(&self, device: Id, name: &str, issued: i64) -> DeviceCert {
        let bytes = DeviceCert::signing_bytes(self.id(), device, name, issued);
        DeviceCert {
            account: self.id(),
            device,
            name: name.to_string(),
            issued,
            sig: self.secret.sign(&bytes).to_bytes(),
        }
    }

    /// Отозвать устройство: «оно больше не моё».
    pub fn revoke(&self, device: Id, at: i64) -> DeviceRevoke {
        let bytes = DeviceRevoke::signing_bytes(self.id(), device, at);
        DeviceRevoke {
            account: self.id(),
            device,
            at,
            sig: self.secret.sign(&bytes).to_bytes(),
        }
    }

    /// Удостоверение этого устройства. Выдаётся один раз и хранится: имя
    /// устройства в нём подписано, и переподписывать его на каждом запуске
    /// незачем.
    pub fn own_cert(&self, store: &Store, device: Id, name: &str) -> Result<DeviceCert> {
        if let Some(raw) = store.get_setting(CERT_SETTING)? {
            if let Ok(cert) = postcard::from_bytes::<DeviceCert>(&raw) {
                if cert.account == self.id() && cert.device == device && verify_cert(&cert) {
                    return Ok(cert);
                }
            }
        }
        let cert = self.certify(device, name, crate::domain::now_ms());
        store.set_setting(CERT_SETTING, &postcard::to_stdvec(&cert)?)?;
        Ok(cert)
    }
}

/// Подлинно ли удостоверение: подписано ли оно тем аккаунтом, что в нём указан.
pub fn verify_cert(cert: &DeviceCert) -> bool {
    verify(cert.account, &cert.bytes(), &cert.sig)
}

/// Подлинен ли отзыв.
pub fn verify_revoke(revoke: &DeviceRevoke) -> bool {
    verify(revoke.account, &revoke.bytes(), &revoke.sig)
}

/// Как называть это устройство, пока человек не дал ему имя сам.
pub fn device_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Mac"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "ios") {
        "iPhone"
    } else if cfg!(target_os = "android") {
        "Android"
    } else {
        "Linux"
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
    fn device_cert_verifies_and_resists_forgery() {
        let store = Store::in_memory().unwrap();
        let account = Account::load_or_create(&store).unwrap();
        let device = Identity::load_or_create(&store).unwrap().id();
        let cert = account.certify(device, "MacBook", 42);
        assert!(verify_cert(&cert), "удостоверение своего аккаунта подлинно");

        let mut renamed = cert.clone();
        renamed.name = "чужой".into();
        assert!(
            !verify_cert(&renamed),
            "имя подписано — переименовать нельзя"
        );

        let stranger = Account::load_or_create(&Store::in_memory().unwrap()).unwrap();
        let mut stolen = cert.clone();
        stolen.account = stranger.id();
        assert!(
            !verify_cert(&stolen),
            "чужому аккаунту чужое устройство не присвоить"
        );
    }

    #[test]
    fn revoke_verifies() {
        let store = Store::in_memory().unwrap();
        let account = Account::load_or_create(&store).unwrap();
        let revoke = account.revoke(Id([5u8; 32]), 7);
        assert!(verify_revoke(&revoke));
    }

    #[test]
    fn account_and_own_cert_persist() {
        let store = Store::in_memory().unwrap();
        let device = Identity::load_or_create(&store).unwrap().id();
        let first = Account::load_or_create(&store).unwrap();
        let cert = first.own_cert(&store, device, "Mac").unwrap();
        let again = Account::load_or_create(&store).unwrap();
        assert_eq!(first.id(), again.id(), "аккаунт переживает перезапуск");
        assert_eq!(
            again.own_cert(&store, device, "Mac").unwrap(),
            cert,
            "удостоверение не переподписывается"
        );
    }

    #[test]
    fn adopted_account_replaces_the_old_one() {
        let store = Store::in_memory().unwrap();
        let device = Identity::load_or_create(&store).unwrap().id();
        let mine = Account::load_or_create(&store).unwrap();
        mine.own_cert(&store, device, "Mac").unwrap();

        let other = Account::load_or_create(&Store::in_memory().unwrap()).unwrap();
        let adopted = Account::adopt(&store, other.secret_bytes()).unwrap();
        assert_eq!(adopted.id(), other.id());
        let cert = adopted.own_cert(&store, device, "Mac").unwrap();
        assert_eq!(
            cert.account,
            other.id(),
            "старое удостоверение не годится новому аккаунту"
        );
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
