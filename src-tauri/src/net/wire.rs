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

/// Версия формата провода.
///
/// Формат событий не самоописывающий: пропущенное или новое поле не
/// «подставляется по умолчанию», а сдвигает весь поток. Поэтому узлы разных
/// версий не могут читать события друг друга в принципе — и об этом надо
/// говорить вслух, а не молча отбрасывать чужие сообщения.
///
/// Четвёртая — подписанные сообщения роя, роли, смена ключа пространства и
/// приглашения без ключа внутри (0.8).
pub const PROTOCOL: u16 = 4;

/// Метка подписи сообщения роя: подпись этого рода нельзя выдать за другую.
const BROADCAST_DOMAIN: &[u8] = b"bred broadcast v4";

/// Конверт: версия снаружи, чтобы её можно было прочитать всегда.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u16,
    pub body: Vec<u8>,
}

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
    /// Состояние общего плеера. Рассылает ведущий, пока идёт трансляция.
    ///
    /// `None` — трансляцию выключили. Ждать, пока состояние протухнет само,
    /// было бы шесть секунд панели с кнопками, которые уже ничего не делают.
    Player(Option<PlayerState>),
    /// Нажатие на пульте: любой участник комнаты — ведущему.
    ///
    /// Отправителя, как и в присутствии, никто не подписывает: ключ пространства
    /// общий, и внутри него все равны. Ведущий поэтому проверяет не подпись, а
    /// то, что человек и правда сидит с ним в одной комнате.
    PlayerCommand {
        to: Id,
        from: Id,
        channel: ChannelId,
        command: PlayerCommand,
    },
}

/// Что играет в комнате и у кого.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerState {
    /// Чей это звук: он же единственный, кто может нажимать на кнопки источника.
    pub host: Id,
    pub channel: ChannelId,
    /// Имя приложения-источника — «Яндекс Музыка», «весь звук системы».
    pub source: String,
    /// Играет ли прямо сейчас.
    ///
    /// Не то, что мы когда-то нажали, а то, что слышно: ведущий смотрит на
    /// собственный поток. Иначе плеер врал бы каждый раз, когда музыку
    /// остановили мимо него — из самой Яндекс Музыки, например.
    pub playing: bool,
    pub ts: i64,
}

/// Кнопка на пульте.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayerCommand {
    Toggle,
    Next,
    Previous,
}

impl From<PlayerCommand> for crate::player::Command {
    fn from(command: PlayerCommand) -> Self {
        match command {
            PlayerCommand::Toggle => Self::Toggle,
            PlayerCommand::Next => Self::Next,
            PlayerCommand::Previous => Self::Previous,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Presence {
    pub author: Id,
    pub nick: String,
    /// Голосовой канал, в котором находится участник.
    pub voice: Option<ChannelId>,
    pub ts: i64,
    /// Собственный адрес отправителя.
    ///
    /// Без него до человека нельзя дозвониться напрямую: рой gossip прокладывает
    /// пути себе сам, а наши собственные соединения — за файлами и за медиа —
    /// знают только идентификатор и вынуждены надеяться на внешний DNS. Когда он
    /// недоступен, не работает ничего: ни картинки, ни аватары, ни стикеры,
    /// ни камера. Присутствие и так летит всем каждые несколько секунд, поэтому
    /// адрес едет вместе с ним.
    #[serde(default)]
    pub addr: Vec<u8>,
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

/// Маячок локальной сети. Уходит по мультикасту всем в той же сети, поэтому
/// открытого в нём нет ничего.
///
/// Раньше адрес узла — а в нём его постоянный идентификатор — ехал как есть, и
/// любой в том же Wi-Fi видел, что здесь работает БРЕД, и узнавал то же
/// устройство в другой сети. Теперь адрес закрыт одноразовым ключом, а ключ —
/// ключом каждого пространства; метки пространств меняются раз в десять минут.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Beacon {
    /// Сериализованный `EndpointAddr`, закрытый одноразовым ключом маячка.
    pub sealed: Vec<u8>,
    /// Соль маски: одинаковые ключи в разных маячках выглядят по-разному.
    pub salt: [u8; 16],
    /// По месту на каждое наше пространство: метка окна и одноразовый ключ,
    /// закрытый маской из ключа этого пространства.
    pub slots: Vec<BeaconSlot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeaconSlot {
    pub tag: [u8; 32],
    /// Одноразовый ключ XOR маска. Подлинность проверяет сам адрес: с
    /// неверным ключом AEAD его не откроет.
    pub key: [u8; 32],
}

// ── шифрование ──────────────────────────────────────────────────────────────

/// Сообщение роя с подписью отправителя.
///
/// Раньше подписаны были только события лога, а присутствие, «печатает» и
/// пульт плеера ехали как есть. Ключ пространства общий, и любой участник мог
/// разослать «присутствие» от чужого имени — с чужим ником, в чужом звонке.
/// Теперь каждое сообщение подписано ключом устройства, а получатель сверяет
/// подпись с тем, кем сообщение себя называет.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Signed {
    author: Id,
    /// Когда подписано, мс. Без метки перехваченное «нажал паузу» можно было
    /// бы повторять в рой бесконечно.
    at: i64,
    message: Broadcast,
    #[serde(with = "crate::domain::event::serde_sig")]
    sig: [u8; 64],
}

/// Сколько живёт подписанное сообщение роя. С запасом на часы, которые у
/// людей расходятся, но не настолько, чтобы старое можно было крутить по кругу.
const BROADCAST_TTL_MS: i64 = 10 * 60 * 1000;

/// Что подписывается: метка, пространство, автор, время и само сообщение.
/// Пространство в подписи — чтобы сообщение из одного пространства нельзя было
/// переслать в другое, где отправитель тоже состоит.
fn signing_bytes(space: SpaceId, author: Id, at: i64, message: &Broadcast) -> Result<Vec<u8>> {
    let mut out = BROADCAST_DOMAIN.to_vec();
    out.extend_from_slice(&space.0);
    out.extend_from_slice(&author.0);
    out.extend_from_slice(&at.to_le_bytes());
    out.extend_from_slice(&postcard::to_stdvec(message)?);
    Ok(out)
}

/// Упаковать сообщение роя: подпись, версия формата, шифр пространства.
pub fn wrap(
    key: &[u8; 32],
    space: SpaceId,
    identity: &crate::identity::Identity,
    message: &Broadcast,
) -> Result<Vec<u8>> {
    let author = identity.id();
    let at = crate::domain::now_ms();
    let sig = identity.sign(&signing_bytes(space, author, at, message)?);
    let body = postcard::to_stdvec(&Signed {
        author,
        at,
        message: message.clone(),
        sig,
    })?;
    seal(
        key,
        &Envelope {
            version: PROTOCOL,
            body,
        },
    )
}

/// Распаковать сообщение роя. `Err` с версией — собеседник на другой версии.
///
/// Тело той же версии, которое не разобралось, — это новый вид эфемерного
/// сообщения от более свежего БРЕД. Раньше это превращалось в «у собеседника
/// другая версия, формат 3 против 3» — неправда, которая пугала зря. Теперь
/// такое сообщение просто пропускается: события лога так не теряются, их
/// догонит досинхронизация.
///
/// Возвращает подтверждённого подписью отправителя и сообщение. Неподписанное
/// или подписанное не тем — молча отбрасывается, как чужой шум.
pub fn unwrap(key: &[u8; 32], space: SpaceId, raw: &[u8]) -> Result<(Id, Broadcast), Option<u16>> {
    let envelope: Envelope = open(key, raw).map_err(|_| None)?;
    if envelope.version != PROTOCOL {
        return Err(Some(envelope.version));
    }
    let signed: Signed = postcard::from_bytes(&envelope.body).map_err(|_| None)?;
    if (crate::domain::now_ms() - signed.at).abs() > BROADCAST_TTL_MS {
        return Err(None);
    }
    let bytes =
        signing_bytes(space, signed.author, signed.at, &signed.message).map_err(|_| None)?;
    if !crate::identity::verify(signed.author, &bytes, &signed.sig) {
        return Err(None);
    }
    Ok((signed.author, signed.message))
}

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

    const SPACE: SpaceId = Id([6u8; 32]);

    fn me() -> crate::identity::Identity {
        crate::identity::Identity::load_or_create(&crate::store::Store::in_memory().unwrap())
            .unwrap()
    }

    #[test]
    fn unknown_message_of_same_version_is_not_a_version_clash() {
        let key = [6u8; 32];
        // Так выглядит для старой версии новый вид сообщения: конверт тот же,
        // а тело не разбирается.
        let sealed = seal(
            &key,
            &Envelope {
                version: PROTOCOL,
                body: vec![250, 1, 2, 3],
            },
        )
        .unwrap();
        assert!(matches!(unwrap(&key, SPACE, &sealed), Err(None)));
    }

    #[test]
    fn sealed_message_opens_with_same_key() {
        let key = [3u8; 32];
        let msg = Presence {
            author: Id([1u8; 32]),
            addr: Vec::new(),
            nick: "марина".into(),
            voice: None,
            ts: 42,
        };
        let sealed = seal(&key, &msg).unwrap();
        let opened: Presence = open(&key, &sealed).unwrap();
        assert_eq!(opened.nick, "марина");
    }

    #[test]
    fn player_state_survives_the_swarm() {
        let key = [5u8; 32];
        let sealed = wrap(
            &key,
            SPACE,
            &me(),
            &Broadcast::Player(Some(PlayerState {
                host: Id([7u8; 32]),
                channel: Id([8u8; 32]),
                source: "Яндекс Музыка".into(),
                playing: true,
                ts: 99,
            })),
        )
        .unwrap();

        match unwrap(&key, SPACE, &sealed)
            .expect("своё сообщение читается")
            .1
        {
            Broadcast::Player(Some(state)) => {
                assert_eq!(state.source, "Яндекс Музыка");
                assert!(state.playing);
                assert_eq!(state.host, Id([7u8; 32]));
            }
            other => panic!("приехало не состояние плеера: {other:?}"),
        }
    }

    #[test]
    fn player_command_survives_the_swarm() {
        let key = [5u8; 32];
        let sealed = wrap(
            &key,
            SPACE,
            &me(),
            &Broadcast::PlayerCommand {
                to: Id([1u8; 32]),
                from: Id([2u8; 32]),
                channel: Id([3u8; 32]),
                command: PlayerCommand::Next,
            },
        )
        .unwrap();

        match unwrap(&key, SPACE, &sealed)
            .expect("своё сообщение читается")
            .1
        {
            Broadcast::PlayerCommand { to, command, .. } => {
                assert_eq!(to, Id([1u8; 32]));
                assert_eq!(command, PlayerCommand::Next);
            }
            other => panic!("приехала не команда пульта: {other:?}"),
        }
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
    fn envelope_round_trips_within_one_version() {
        let key = [5u8; 32];
        let message = Broadcast::Typing {
            channel: Id([2u8; 32]),
            author: Id([3u8; 32]),
        };
        let identity = me();
        let raw = wrap(&key, SPACE, &identity, &message).unwrap();
        let (author, back) = unwrap(&key, SPACE, &raw).unwrap();
        assert!(matches!(back, Broadcast::Typing { .. }));
        assert_eq!(author, identity.id(), "отправитель подтверждён подписью");
    }

    #[test]
    fn forged_or_replayed_elsewhere_is_rejected() {
        let key = [5u8; 32];
        let message = Broadcast::Typing {
            channel: Id([2u8; 32]),
            author: Id([3u8; 32]),
        };
        let raw = wrap(&key, SPACE, &me(), &message).unwrap();
        assert_eq!(
            unwrap(&key, Id([9u8; 32]), &raw).err(),
            Some(None),
            "сообщение одного пространства не годится в другом"
        );

        // Подпись чужого ключа под своим именем — не пройдёт.
        let envelope: Envelope = open(&key, &raw).unwrap();
        let mut signed: Signed = postcard::from_bytes(&envelope.body).unwrap();
        signed.author = me().id();
        let forged = seal(
            &key,
            &Envelope {
                version: PROTOCOL,
                body: postcard::to_stdvec(&signed).unwrap(),
            },
        )
        .unwrap();
        assert_eq!(unwrap(&key, SPACE, &forged).err(), Some(None));
    }

    #[test]
    fn other_version_is_reported_not_swallowed() {
        // Узел с другой версией формата обязан быть заметен: иначе со стороны
        // это выглядит как «человек в сети, но его сообщения не приходят».
        let key = [5u8; 32];
        let alien = seal(
            &key,
            &Envelope {
                version: PROTOCOL + 7,
                body: vec![1, 2, 3],
            },
        )
        .unwrap();
        assert_eq!(unwrap(&key, SPACE, &alien).err(), Some(Some(PROTOCOL + 7)));
    }

    #[test]
    fn foreign_key_stays_silent() {
        // А вот чужой ключ — обычное дело в открытом рое, шуметь не о чем.
        let raw = wrap(
            &[1u8; 32],
            SPACE,
            &me(),
            &Broadcast::Typing {
                channel: Id([2u8; 32]),
                author: Id([3u8; 32]),
            },
        )
        .unwrap();
        assert_eq!(unwrap(&[9u8; 32], SPACE, &raw).err(), Some(None));
    }

    #[test]
    fn short_frame_does_not_panic() {
        let result: Result<String> = open(&[1u8; 32], &[0u8; 5]);
        assert!(result.is_err());
    }
}
