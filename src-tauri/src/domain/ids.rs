//! Идентификаторы БРЕД.
//!
//! Всё в системе адресуется одним 32-байтным идентификатором: пространство,
//! канал, событие, автор. В JSON он выглядит как base32-строка (удобно для UI),
//! в бинарном формате едет сырыми байтами (компактно для сети и диска).

use std::fmt;

use data_encoding::BASE32_NOPAD;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};

/// 32-байтный идентификатор.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Id(pub [u8; 32]);

/// Идентификатор пространства («сервера»).
pub type SpaceId = Id;
/// Идентификатор канала — это идентификатор события, создавшего канал.
pub type ChannelId = Id;
/// Идентификатор события.
pub type EventId = Id;
/// Публичный ключ автора.
pub type AuthorId = Id;

impl Id {
    pub const ZERO: Id = Id([0u8; 32]);

    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        let arr: [u8; 32] = bytes.try_into().ok()?;
        Some(Id(arr))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Короткая форма для логов и UI: первые 8 символов base32.
    pub fn short(&self) -> String {
        self.to_string().chars().take(8).collect()
    }

    pub fn parse(s: &str) -> Result<Self, IdParseError> {
        let raw = BASE32_NOPAD
            .decode(s.to_ascii_uppercase().as_bytes())
            .map_err(|_| IdParseError)?;
        Self::from_slice(&raw).ok_or(IdParseError)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("некорректный идентификатор")]
pub struct IdParseError;

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", BASE32_NOPAD.encode(&self.0).to_ascii_lowercase())
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({})", self.short())
    }
}

impl From<[u8; 32]> for Id {
    fn from(v: [u8; 32]) -> Self {
        Id(v)
    }
}

/// В человекочитаемых форматах (JSON для UI) — строка,
/// в бинарных (postcard для сети и БД) — сырые байты.
impl Serialize for Id {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if s.is_human_readable() {
            s.serialize_str(&self.to_string())
        } else {
            s.serialize_bytes(&self.0)
        }
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if d.is_human_readable() {
            let s = String::deserialize(d)?;
            return Id::parse(&s).map_err(D::Error::custom);
        }
        // Читаем ровно тем же способом, каким писали: `serialize_bytes` кладёт
        // длину и тело, а `[u8; 32]` ждёт голые байты без длины. Рассинхрон
        // здесь ломает не только сам идентификатор, но и все поля после него.
        d.deserialize_bytes(IdVisitor)
    }
}

struct IdVisitor;

impl<'de> serde::de::Visitor<'de> for IdVisitor {
    type Value = Id;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("32 байта идентификатора")
    }

    fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Id, E> {
        Id::from_slice(v).ok_or_else(|| E::invalid_length(v.len(), &self))
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Id, A::Error> {
        let mut out = [0u8; 32];
        for (index, slot) in out.iter_mut().enumerate() {
            *slot = seq
                .next_element()?
                .ok_or_else(|| serde::de::Error::invalid_length(index, &self))?;
        }
        Ok(Id(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_round_trips_through_base32() {
        let id = Id([7u8; 32]);
        let text = id.to_string();
        assert_eq!(Id::parse(&text).unwrap(), id);
    }

    #[test]
    fn short_form_is_eight_chars() {
        assert_eq!(Id([1u8; 32]).short().len(), 8);
    }

    #[test]
    fn binary_encoding_stays_compact() {
        // 32 байта + служебная длина — заметно меньше, чем 52-символьная строка.
        let bytes = postcard::to_stdvec(&Id([3u8; 32])).unwrap();
        assert!(bytes.len() <= 34, "получилось {} байт", bytes.len());
    }

    #[test]
    fn binary_round_trip_keeps_following_fields_intact() {
        // Регрессия: несимметричные serialize/deserialize сдвигали поток,
        // и всё, что шло за идентификатором, читалось как мусор.
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct Frame {
            id: Id,
            flag: Option<u8>,
            tail: String,
        }

        let frame = Frame {
            id: Id([5u8; 32]),
            flag: Some(9),
            tail: "хвост".into(),
        };
        let raw = postcard::to_stdvec(&frame).unwrap();
        assert_eq!(postcard::from_bytes::<Frame>(&raw).unwrap(), frame);
    }

    #[test]
    fn json_round_trip_uses_readable_string() {
        let raw = serde_json::to_string(&Id([2u8; 32])).unwrap();
        assert!(raw.starts_with('"'), "в JSON ожидалась строка, а не массив");
        assert_eq!(serde_json::from_str::<Id>(&raw).unwrap(), Id([2u8; 32]));
    }
}
