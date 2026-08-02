//! Формат того, что уходит в сеть.
//!
//! Всё шифруется ключом пространства: релей, соседи по локалке и любой, кто
//! слушает провод, видят только шум. Ключ знают ровно те, кому дали ссылку-
//! приглашение — на этом и держится модель доступа, серверных прав нет.

use anyhow::{anyhow, Result};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::domain::{ChannelId, Id, SignedEvent, SpaceId};

/// Максимальный размер кадра досинхронизации — защита от пира,
/// который пришлёт «историю» на гигабайт и съест память.
pub const MAX_FRAME: usize = 8 * 1024 * 1024;

/// Сколько событий отдаём за один заход досинхронизации.
pub const SYNC_BATCH: usize = 512;

/// Сообщение, разлетающееся по gossip-рою пространства.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Broadcast {
    /// Новое событие лога.
    Event(Box<SignedEvent>),
    /// Присутствие. Живёт только в памяти и в лог не попадает — иначе история
    /// распухнет на порядок от «зашёл/вышел», а пользы ноль.
    Presence(Presence),
    /// «Печатает…». Тоже эфемерное.
    Typing { channel: ChannelId, author: Id },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Presence {
    pub author: Id,
    pub nick: String,
    /// Голосовой канал, в котором находится участник.
    pub voice: Option<ChannelId>,
    pub ts: i64,
}

/// Протокол досинхронизации поверх отдельного QUIC-потока.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncFrame {
    /// «Вот что у меня есть, пришли недостающее».
    Request {
        space: SpaceId,
        have: Vec<(Id, u64)>,
    },
    /// Ответ с недостающими событиями и собственным вектором версий.
    Response {
        events: Vec<SignedEvent>,
        have: Vec<(Id, u64)>,
    },
    /// Досылка в обратную сторону: то, чего не хватало пиру.
    Push { events: Vec<SignedEvent> },
}

/// Маячок локальной сети. Уходит открытым текстом по мультикасту, поэтому
/// внутри нет ни имён, ни идентификаторов пространств — только метки-производные,
/// по которым свои узнают своих, а чужие видят случайные байты.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Beacon {
    /// Сериализованный `EndpointAddr` отправителя.
    pub addr: Vec<u8>,
    /// `Space::lan_tag()` для каждого пространства, в котором мы состоим.
    pub tags: Vec<[u8; 32]>,
}

// ── шифрование ──────────────────────────────────────────────────────────────

/// Зашифровать сообщение ключом пространства.
pub fn seal<T: Serialize>(key: &[u8; 32], value: &T) -> Result<Vec<u8>> {
    let plain = postcard::to_stdvec(value)?;
    let cipher = XChaCha20Poly1305::new(key.into());

    let mut nonce = [0u8; 24];
    rand::Rng::fill(&mut rand::rng(), &mut nonce[..]);

    let mut out = nonce.to_vec();
    let sealed = cipher
        .encrypt(XNonce::from_slice(&nonce), plain.as_slice())
        .map_err(|_| anyhow!("не удалось зашифровать сообщение"))?;
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// Расшифровать сообщение ключом пространства.
pub fn open<T: for<'de> Deserialize<'de>>(key: &[u8; 32], raw: &[u8]) -> Result<T> {
    if raw.len() < 24 {
        return Err(anyhow!("кадр короче nonce"));
    }
    let (nonce, body) = raw.split_at(24);
    let cipher = XChaCha20Poly1305::new(key.into());
    let plain = cipher
        .decrypt(XNonce::from_slice(nonce), body)
        .map_err(|_| anyhow!("не удалось расшифровать: чужой ключ или подмена"))?;
    Ok(postcard::from_bytes(&plain)?)
}

/// Вектор версий в формате провода и обратно.
pub fn vector_to_wire(have: &HashMap<Id, u64>) -> Vec<(Id, u64)> {
    have.iter().map(|(id, seq)| (*id, *seq)).collect()
}

pub fn vector_from_wire(have: Vec<(Id, u64)>) -> HashMap<Id, u64> {
    have.into_iter().collect()
}

// ── кадрирование ────────────────────────────────────────────────────────────

/// Кадр: длина u32 LE, затем тело.
pub async fn write_frame<W: tokio::io::AsyncWriteExt + Unpin>(
    w: &mut W,
    payload: &[u8],
) -> Result<()> {
    w.write_all(&(payload.len() as u32).to_le_bytes()).await?;
    w.write_all(payload).await?;
    Ok(())
}

pub async fn read_frame<R: tokio::io::AsyncReadExt + Unpin>(r: &mut R) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        // Иначе достаточно одного недоброжелательного пира, чтобы съесть память.
        return Err(anyhow!("кадр {len} байт превышает лимит {MAX_FRAME}"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_message_opens_with_same_key() {
        let key = [3u8; 32];
        let msg = Presence {
            author: Id([1u8; 32]),
            nick: "марина".into(),
            voice: None,
            ts: 42,
        };
        let sealed = seal(&key, &msg).unwrap();
        let opened: Presence = open(&key, &sealed).unwrap();
        assert_eq!(opened.nick, "марина");
    }

    #[test]
    fn wrong_key_cannot_read_message() {
        let sealed = seal(&[3u8; 32], &"секрет".to_string()).unwrap();
        let result: Result<String> = open(&[4u8; 32], &sealed);
        assert!(result.is_err(), "чужой ключ не должен расшифровывать");
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let key = [3u8; 32];
        let mut sealed = seal(&key, &"секрет".to_string()).unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0xff;
        let result: Result<String> = open(&key, &sealed);
        assert!(result.is_err(), "подмена шифротекста должна ловиться");
    }

    #[test]
    fn same_plaintext_produces_different_ciphertext() {
        let key = [3u8; 32];
        let a = seal(&key, &"одно и то же".to_string()).unwrap();
        let b = seal(&key, &"одно и то же".to_string()).unwrap();
        assert_ne!(a, b, "nonce должен быть случайным на каждое сообщение");
    }

    #[tokio::test]
    async fn frame_round_trips() {
        let mut buf = Vec::new();
        write_frame(&mut buf, b"payload").await.unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        assert_eq!(read_frame(&mut cursor).await.unwrap(), b"payload");
    }

    #[tokio::test]
    async fn oversized_frame_is_rejected() {
        let mut buf = ((MAX_FRAME + 1) as u32).to_le_bytes().to_vec();
        buf.extend_from_slice(b"...");
        let mut cursor = std::io::Cursor::new(buf);
        assert!(read_frame(&mut cursor).await.is_err());
    }

    #[test]
    fn short_frame_does_not_panic() {
        let result: Result<String> = open(&[1u8; 32], &[0u8; 5]);
        assert!(result.is_err());
    }
}
