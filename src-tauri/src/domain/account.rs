//! Аккаунт: несколько устройств одного человека под одним именем.
//!
//! Сейчас человек в БРЕД — это устройство: ключ ed25519 на ноутбуке и есть
//! автор событий, адрес в сети и запись в списке участников. Открыл БРЕД на
//! телефоне — появился второй человек с тем же именем, без истории личных
//! переписок и без пространств.
//!
//! Аккаунт меняет это, не трогая устройство. Устройство по-прежнему подписывает
//! свои события своим ключом и по-прежнему само по себе адрес в сети: так
//! устроен iroh, и так проще отозвать потерянный телефон, не меняя ключи всех
//! остальных. Сверху появляется второй ключ — ключ аккаунта, общий для всех
//! устройств человека. Им подписывается одно: «это устройство — моё»
//! ([`DeviceCert`]). Удостоверение уезжает в каждое пространство, и соседи
//! начинают показывать все устройства как одного человека.
//!
//! Новое устройство получает ключ аккаунта при привязке ([`LinkTicket`]): на
//! старом показывается одноразовая ссылка, новое по ней дозванивается напрямую
//! и забирает [`LinkBundle`] — ключ аккаунта, ключ личных переписок, профиль и
//! ключи всех пространств. Сервера в этой цепочке нет, как и везде в БРЕД.
//!
//! Слой доменный: здесь форматы и правила. Подпись и проверка — в `identity`,
//! хранение — в `store`, передача по сети — в `net` (протокол `bred/link/1`,
//! описан в ARCHITECTURE.md).

use serde::{Deserialize, Serialize};

use super::{event::serde_sig, Id, Space};

/// Идентификатор аккаунта — публичный ключ ed25519, общий для всех устройств.
pub type AccountId = Id;

/// Сколько живёт ссылка привязки. Она даёт полный доступ к аккаунту, поэтому
/// коротко: успеть отсканировать, но не успеть потерять в переписке.
pub const LINK_TTL_MS: i64 = 10 * 60 * 1000;

/// Метки подписей: подпись одного рода нельзя выдать за подпись другого.
const CERT_DOMAIN: &[u8] = b"bred device cert v1";
const REVOKE_DOMAIN: &[u8] = b"bred device revoke v1";

/// Удостоверение: аккаунт подписывает «это устройство — моё».
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCert {
    pub account: AccountId,
    pub device: Id,
    /// Имя устройства для людей: «MacBook», «телефон».
    pub name: String,
    /// Когда выдано, миллисекунды. Только для показа и для порядка с отзывом.
    pub issued: i64,
    #[serde(with = "serde_sig")]
    pub sig: [u8; 64],
}

impl DeviceCert {
    /// Что именно подписывается. Имя входит в подпись: иначе любой мог бы
    /// переименовать чужое устройство, не трогая удостоверения.
    pub fn signing_bytes(account: AccountId, device: Id, name: &str, issued: i64) -> Vec<u8> {
        let mut out = Vec::with_capacity(CERT_DOMAIN.len() + 72 + name.len());
        out.extend_from_slice(CERT_DOMAIN);
        out.extend_from_slice(&account.0);
        out.extend_from_slice(&device.0);
        out.extend_from_slice(&issued.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out
    }

    pub fn bytes(&self) -> Vec<u8> {
        Self::signing_bytes(self.account, self.device, &self.name, self.issued)
    }
}

/// Отзыв: аккаунт подписывает «это устройство больше не моё».
///
/// Потерянный телефон отзывается с любого оставшегося устройства. События,
/// которые он подписал до отзыва, остаются в истории — лог не переписывается,
/// — а после отзыва соседи перестают считать его частью аккаунта.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRevoke {
    pub account: AccountId,
    pub device: Id,
    pub at: i64,
    #[serde(with = "serde_sig")]
    pub sig: [u8; 64],
}

impl DeviceRevoke {
    pub fn signing_bytes(account: AccountId, device: Id, at: i64) -> Vec<u8> {
        let mut out = Vec::with_capacity(REVOKE_DOMAIN.len() + 72);
        out.extend_from_slice(REVOKE_DOMAIN);
        out.extend_from_slice(&account.0);
        out.extend_from_slice(&device.0);
        out.extend_from_slice(&at.to_le_bytes());
        out
    }

    pub fn bytes(&self) -> Vec<u8> {
        Self::signing_bytes(self.account, self.device, self.at)
    }
}

/// Ссылка привязки нового устройства. Показывается на уже привязанном —
/// QR-кодом или текстом.
///
/// Это ключ от аккаунта, а не приглашение в пространство: с ним новое
/// устройство получает всё. Поэтому она одноразовая, живёт десять минут, а
/// привязку старое устройство ещё и подтверждает вручную.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkTicket {
    /// Сериализованный адрес устройства, которое привязывает.
    pub addr: Vec<u8>,
    /// Одноразовый секрет: им шифруется вся передача, и им же новое
    /// устройство доказывает, что ссылку видело.
    pub secret: [u8; 32],
    pub expires: i64,
}

impl LinkTicket {
    const PREFIX: &'static str = "bred://link/";

    pub fn encode(&self) -> String {
        let raw = postcard::to_stdvec(self).expect("ссылка привязки сериализуема");
        format!(
            "{}{}",
            Self::PREFIX,
            data_encoding::BASE32_NOPAD
                .encode(&raw)
                .to_ascii_lowercase()
        )
    }

    pub fn decode(text: &str) -> anyhow::Result<Self> {
        let body = text
            .trim()
            .strip_prefix(Self::PREFIX)
            .ok_or_else(|| anyhow::anyhow!("ссылка привязки начинается с {}", Self::PREFIX))?;
        let raw = data_encoding::BASE32_NOPAD
            .decode(body.to_ascii_uppercase().as_bytes())
            .map_err(|_| anyhow::anyhow!("повреждённая ссылка привязки"))?;
        Ok(postcard::from_bytes(&raw)?)
    }

    pub fn expired(&self, now: i64) -> bool {
        now > self.expires
    }

    /// Ключ, которым шифруется передача по этой ссылке.
    pub fn channel_key(&self) -> [u8; 32] {
        blake3::derive_key("bred link channel v1", &self.secret)
    }

    /// Доказательство, что новое устройство видело ссылку: привязано к его
    /// собственному ключу, поэтому подслушанное доказательство чужому не
    /// поможет.
    pub fn proof(&self, device: Id) -> [u8; 32] {
        let key = blake3::derive_key("bred link proof v1", &self.secret);
        *blake3::keyed_hash(&key, &device.0).as_bytes()
    }
}

/// Что получает новое устройство при привязке. Едет зашифрованным ключом
/// ссылки и только тому, кто доказал, что её видел.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkBundle {
    /// Ключ аккаунта: с ним новое устройство может само привязывать следующие.
    pub account_secret: [u8; 32],
    /// Ключ личных переписок. Общий на аккаунт, иначе у каждого устройства
    /// выводились бы свои личные пространства, и переписка с другом
    /// раздваивалась бы по числу телефонов.
    pub dh_secret: [u8; 32],
    pub nick: String,
    pub avatar: Option<Id>,
    /// Все пространства с ключами — новое устройство входит в них сразу, а
    /// история догоняется обычной досинхронизацией.
    pub spaces: Vec<Space>,
    /// Удостоверение нового устройства, уже подписанное аккаунтом.
    pub cert: DeviceCert,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket() -> LinkTicket {
        LinkTicket {
            addr: vec![1, 2, 3],
            secret: [7u8; 32],
            expires: 1_000,
        }
    }

    #[test]
    fn link_ticket_round_trips() {
        let original = ticket();
        let text = original.encode();
        assert!(text.starts_with("bred://link/"));
        assert_eq!(LinkTicket::decode(&text).unwrap(), original);
    }

    #[test]
    fn foreign_links_are_not_link_tickets() {
        assert!(LinkTicket::decode("bred://join/abc").is_err());
        assert!(LinkTicket::decode("bred://link/%%%").is_err());
    }

    #[test]
    fn link_ticket_expires() {
        assert!(!ticket().expired(999));
        assert!(ticket().expired(1_001));
    }

    #[test]
    fn proof_is_bound_to_the_device() {
        let t = ticket();
        assert_ne!(
            t.proof(Id([1u8; 32])),
            t.proof(Id([2u8; 32])),
            "подслушанное доказательство не должно подходить другому устройству"
        );
        assert_ne!(t.proof(Id([1u8; 32])), t.channel_key());
    }

    #[test]
    fn cert_and_revoke_never_sign_the_same_bytes() {
        let a = Id([1u8; 32]);
        let d = Id([2u8; 32]);
        assert_ne!(
            DeviceCert::signing_bytes(a, d, "", 5),
            DeviceRevoke::signing_bytes(a, d, 5),
            "подпись удостоверения нельзя выдать за отзыв"
        );
    }

    #[test]
    fn device_name_is_part_of_the_signature() {
        let a = Id([1u8; 32]);
        let d = Id([2u8; 32]);
        assert_ne!(
            DeviceCert::signing_bytes(a, d, "MacBook", 5),
            DeviceCert::signing_bytes(a, d, "телефон", 5)
        );
    }
}
