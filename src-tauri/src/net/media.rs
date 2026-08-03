//! Голосовые и видеозвонки.
//!
//! Комната — это голосовой канал: кто выставил `presence.voice`, тот в звонке.
//! Отдельного сигнального сервера нет и не нужно — присутствие и так разлетается
//! по gossip, а адреса умеет находить iroh.
//!
//! Топология — полная сетка (mesh): каждый шлёт свой поток каждому. Для звонка
//! на 4–6 человек это оптимально: нет сервера-микшера, минимальная задержка,
//! никто не видит чужие потоки в открытую. Дальше сетка упирается в исходящий
//! канал (`N-1` копий), поэтому на больших звонках понадобится SFU — но SFU
//! это сервер, а мы принципиально без него.
//!
//! Транспорт — QUIC-датаграммы, а не потоки: для реального времени потерять
//! кадр лучше, чем ждать его ретрансмита и уронить весь звук.

use anyhow::{anyhow, Result};
use bytes::Bytes;
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr, EndpointId,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::mpsc::Sender;

use super::{
    ctx::Ctx,
    wire::{open, seal, Presence},
};
use crate::domain::{ChannelId, Id, SpaceId};

pub const MEDIA_ALPN: &[u8] = b"bred/media/1";

/// Как часто сверяем состав комнаты с присутствием.
const RECONCILE: Duration = Duration::from_secs(2);
/// Сколько кадров держим в сборке, прежде чем признать их потерянными.
const REASSEMBLY_WINDOW: usize = 24;
/// Запас на служебные поля и шифрование внутри датаграммы.
const OVERHEAD: usize = 192;
/// Консервативный потолок датаграммы, если транспорт не сообщил свой.
const SAFE_DATAGRAM: usize = 1200;
/// Сколько кадров картинки ждут отправки каждому собеседнику. Очередь короткая:
/// если не успеваем, честнее выбросить кадр, чем копить задержку.
const VIDEO_QUEUE: usize = 6;
/// Потолок на кадр, пришедший потоком: защита от пира, который пришлёт гигабайт.
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
/// Сколько кадров ждут интерфейс. Очередь намеренно короткая: в реальном
/// времени опоздавший кадр не нужен никому, а неограниченная очередь при
/// медленном получателе съедает память гигабайтами.
pub const SINK_QUEUE: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Track {
    Audio,
    Video,
    /// Демонстрация экрана. Отдельно от камеры, чтобы показывать обе разом.
    Screen,
    /// Звук вместе с демонстрацией экрана.
    ScreenAudio,
}

impl Track {
    /// Картинку нельзя терять: пропавший кусок кадра рассыпает весь кадр и
    /// тянется артефактами до следующего ключевого. Поэтому видео едет
    /// надёжными потоками, а звук — датаграммами, где потеря дешевле задержки.
    fn reliable(self) -> bool {
        matches!(self, Track::Video | Track::Screen)
    }
}

impl Track {
    fn code(self) -> u8 {
        match self {
            Track::Audio => 0,
            Track::Video => 1,
            Track::Screen => 2,
            Track::ScreenAudio => 3,
        }
    }

    fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Track::Audio),
            1 => Some(Track::Video),
            2 => Some(Track::Screen),
            3 => Some(Track::ScreenAudio),
            _ => None,
        }
    }
}

/// Фрагмент кадра, как он едет по проводу.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaPacket {
    pub space: SpaceId,
    pub channel: ChannelId,
    pub author: Id,
    pub track: Track,
    pub seq: u32,
    pub part: u16,
    pub parts: u16,
    pub keyframe: bool,
    /// Метка времени в микросекундах — её ждёт декодер в вебвью.
    pub ts: i64,
    pub data: Vec<u8>,
}

/// Заголовок обмена с интерфейсом. Компактный бинарный формат: гонять кадры
/// видео через JSON было бы вчетверо дороже.
///
/// `[0]` версия, `[1]` дорожка, `[2]` флаги, `[3..35]` автор, `[35..43]` время.
pub const UI_HEADER: usize = 43;
const UI_VERSION: u8 = 1;

pub fn encode_for_ui(author: Id, track: Track, keyframe: bool, ts: i64, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(UI_HEADER + data.len());
    out.push(UI_VERSION);
    out.push(track.code());
    out.push(u8::from(keyframe));
    out.extend_from_slice(&author.0);
    out.extend_from_slice(&ts.to_le_bytes());
    out.extend_from_slice(data);
    out
}

/// Разбор того, что прислал интерфейс. Автор в заголовке игнорируется:
/// подставить чужой идентификатор из UI не должно быть возможно.
pub fn decode_from_ui(raw: &[u8]) -> Result<(Track, bool, i64, &[u8])> {
    if raw.len() < UI_HEADER {
        return Err(anyhow!("кадр короче заголовка"));
    }
    if raw[0] != UI_VERSION {
        return Err(anyhow!("незнакомая версия кадра: {}", raw[0]));
    }
    let track = Track::from_code(raw[1]).ok_or_else(|| anyhow!("неизвестная дорожка"))?;
    let keyframe = raw[2] != 0;
    let ts = i64::from_le_bytes(raw[35..43].try_into().expect("восемь байт на месте"));
    Ok((track, keyframe, ts, &raw[UI_HEADER..]))
}

/// Связь с одним собеседником.
struct Peer {
    connection: Connection,
    /// Очередь кадров картинки. Отправкой занимается отдельная задача:
    /// открыть поток — операция с ожиданием, а путь кадра должен быть коротким.
    video: tokio::sync::mpsc::Sender<Bytes>,
}

/// Активный звонок.
#[derive(Debug, Clone, Copy)]
struct Call {
    space: SpaceId,
    channel: ChannelId,
}

pub struct Media {
    ctx: Arc<Ctx>,
    endpoint: Endpoint,
    call: RwLock<Option<Call>>,
    peers: RwLock<HashMap<EndpointId, Peer>>,
    outgoing: Mutex<HashMap<Track, u32>>,
    reassembly: Mutex<Reassembly>,
    /// Куда отдавать собранные кадры — интерфейсу.
    sink: RwLock<Option<Sender<Vec<u8>>>>,
    /// Сколько кадров выброшено из-за переполнения — видно в логах.
    dropped: Mutex<u64>,
}

impl Media {
    pub fn new(ctx: Arc<Ctx>, endpoint: Endpoint) -> Arc<Self> {
        let media = Arc::new(Self {
            ctx,
            endpoint,
            call: RwLock::new(None),
            peers: RwLock::new(HashMap::new()),
            outgoing: Mutex::new(HashMap::new()),
            reassembly: Mutex::new(Reassembly::default()),
            sink: RwLock::new(None),
            dropped: Mutex::new(0),
        });
        media.clone().reconcile_loop();
        media
    }

    pub fn set_sink(&self, sink: Sender<Vec<u8>>) {
        *self.sink.write() = Some(sink);
    }

    pub fn active(&self) -> Option<(SpaceId, ChannelId)> {
        self.call.read().map(|c| (c.space, c.channel))
    }

    pub fn join(&self, space: SpaceId, channel: ChannelId) {
        *self.call.write() = Some(Call { space, channel });
        self.outgoing.lock().clear();
        tracing::info!(channel = %channel.short(), "вошли в звонок");
    }

    pub fn leave(&self) {
        *self.call.write() = None;
        // Соединения рвём явно: иначе камера у собеседника ещё секунды будет
        // считать, что мы на связи.
        for (_, peer) in self.peers.write().drain() {
            peer.connection.close(0u32.into(), "вышел".as_bytes());
        }
        self.reassembly.lock().clear();
        tracing::info!("вышли из звонка");
    }

    /// Кто сейчас в той же комнате — по присутствию.
    pub fn participants(&self) -> Vec<Id> {
        let Some(call) = *self.call.read() else {
            return Vec::new();
        };
        self.ctx
            .presence_of(call.space)
            .into_iter()
            .filter(|p: &Presence| p.voice == Some(call.channel))
            .map(|p| p.author)
            .collect()
    }

    /// Разослать свой кадр всем в комнате.
    ///
    /// Синхронно и намеренно: внутри нет ни одной точки ожидания, а обёртка в
    /// задачу порождала бы под сотню задач в секунду, каждую с копией кадра.
    pub fn broadcast(&self, track: Track, keyframe: bool, ts: i64, data: &[u8]) -> Result<()> {
        let Some(call) = *self.call.read() else {
            return Ok(()); // не в звонке — кадр просто выбрасываем
        };
        let Some(key) = self.ctx.space(call.space).map(|s| s.key) else {
            return Ok(());
        };

        let seq = {
            let mut counters = self.outgoing.lock();
            let counter = counters.entry(track).or_insert(0);
            *counter = counter.wrapping_add(1);
            *counter
        };

        let packet = |part: u16, parts: u16, chunk: &[u8]| MediaPacket {
            space: call.space,
            channel: call.channel,
            author: self.ctx.identity.id(),
            track,
            seq,
            part,
            parts,
            keyframe,
            ts,
            data: chunk.to_vec(),
        };

        // Картинка целиком в один надёжный поток: резать её незачем, а терять
        // куски нельзя. Звук — датаграммами, кусками по размеру пути.
        if track.reliable() {
            let sealed = Bytes::from(seal(&key, &packet(0, 1, data))?);
            for peer in self.peers.read().values() {
                // Очередь переполнена — кадр выбрасываем: показать его с
                // опозданием всё равно нельзя, а копить — значит тонуть.
                let _ = peer.video.try_send(sealed.clone());
            }
            return Ok(());
        }

        let connections: Vec<Connection> = self
            .peers
            .read()
            .values()
            .map(|p| p.connection.clone())
            .collect();
        if connections.is_empty() {
            return Ok(());
        }

        let budget = connections
            .iter()
            .filter_map(|c| c.max_datagram_size())
            .min()
            .unwrap_or(SAFE_DATAGRAM)
            .saturating_sub(OVERHEAD)
            .max(256);

        let chunks: Vec<&[u8]> = if data.is_empty() {
            vec![data]
        } else {
            data.chunks(budget).collect()
        };
        let parts =
            u16::try_from(chunks.len()).map_err(|_| anyhow!("кадр слишком фрагментирован"))?;

        for (index, chunk) in chunks.iter().enumerate() {
            let sealed = Bytes::from(seal(&key, &packet(index as u16, parts, chunk))?);
            for connection in &connections {
                // Потеря кадра здесь — норма и лучше, чем ожидание ретрансмита.
                if let Err(err) = connection.send_datagram(sealed.clone()) {
                    tracing::trace!(%err, "датаграмма не ушла");
                }
            }
        }
        Ok(())
    }

    /// Приём от одного собеседника: датаграммы со звуком и потоки с картинкой.
    async fn pump(self: Arc<Self>, connection: Connection) {
        let remote = connection.remote_id();
        let (video, frames) = tokio::sync::mpsc::channel(VIDEO_QUEUE);
        self.peers.write().insert(
            remote,
            Peer {
                connection: connection.clone(),
                video,
            },
        );

        let writer = tokio::spawn(write_frames(connection.clone(), frames));
        let reader = tokio::spawn(read_frames(self.clone(), connection.clone()));

        loop {
            match connection.read_datagram().await {
                Ok(raw) => self.on_packet(&raw),
                Err(err) => {
                    tracing::debug!(peer = %remote.fmt_short(), %err, "медиа-соединение закрыто");
                    break;
                }
            }
        }

        writer.abort();
        reader.abort();
        self.peers.write().remove(&remote);
    }

    fn on_packet(&self, raw: &[u8]) {
        let Some(call) = *self.call.read() else {
            return; // мы не в звонке — принимать нечего
        };
        // Кадры звонка всегда из пространства звонка, поэтому сразу берём его
        // ключ, а не перебираем все: на видео это тысячи пакетов в секунду.
        let Some(key) = self.ctx.space(call.space).map(|s| s.key) else {
            return;
        };
        let Ok(packet) = open::<MediaPacket>(&key, raw) else {
            return; // чужой ключ или мусор
        };
        if packet.channel != call.channel || packet.space != call.space {
            return; // кадр из другой комнаты
        }

        if let Some(frame) = self.reassembly.lock().push(packet) {
            if let Some(sink) = self.sink.read().as_ref() {
                // Очередь переполнена — значит интерфейс не успевает. Кадр
                // выбрасываем: показать его с опозданием всё равно нельзя.
                if sink.try_send(frame).is_err() {
                    let mut dropped = self.dropped.lock();
                    *dropped += 1;
                    if *dropped % 300 == 1 {
                        tracing::debug!(
                            dropped = *dropped,
                            "интерфейс не успевает, кадры теряются"
                        );
                    }
                }
            }
        }
    }

    /// Раз в пару секунд сверяем, кто в комнате, и держим сетку соединений.
    fn reconcile_loop(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(RECONCILE);
            loop {
                ticker.tick().await;
                let Some(_) = *self.call.read() else {
                    continue;
                };
                let me = self.ctx.identity.id();

                for participant in self.participants() {
                    if participant == me {
                        continue;
                    }
                    // Соединение поднимает только одна сторона — та, чей
                    // идентификатор меньше. Иначе получили бы две встречные.
                    if me.0 > participant.0 {
                        continue;
                    }
                    let Ok(peer) = iroh::PublicKey::from_bytes(&participant.0) else {
                        continue;
                    };
                    if self.peers.read().contains_key(&peer) {
                        continue;
                    }

                    let media = self.clone();
                    tokio::spawn(async move {
                        match media
                            .endpoint
                            .connect(EndpointAddr::from(peer), MEDIA_ALPN)
                            .await
                        {
                            Ok(connection) => media.pump(connection).await,
                            Err(err) => {
                                tracing::debug!(peer = %peer.fmt_short(), %err, "звонок не дозвонился")
                            }
                        }
                    });
                }
            }
        });
    }
}

/// Отправка кадров картинки: каждый — своим однонаправленным потоком.
///
/// Поток на кадр, а не один общий: внутри потока QUIC гарантирует порядок и
/// доставку, а между потоками нет блокировки — застрявший кадр не задерживает
/// следующие.
async fn write_frames(connection: Connection, mut frames: tokio::sync::mpsc::Receiver<Bytes>) {
    while let Some(frame) = frames.recv().await {
        let Ok(mut stream) = connection.open_uni().await else {
            break;
        };
        if stream.write_all(&frame).await.is_err() || stream.finish().is_err() {
            break;
        }
    }
}

/// Приём кадров картинки.
async fn read_frames(media: Arc<Media>, connection: Connection) {
    loop {
        let Ok(mut stream) = connection.accept_uni().await else {
            break;
        };
        let media = media.clone();
        tokio::spawn(async move {
            if let Ok(raw) = stream.read_to_end(MAX_FRAME_BYTES).await {
                media.on_packet(&raw);
            }
        });
    }
}

/// Сборка кадров из фрагментов.
///
/// Датаграммы приходят без гарантий порядка и доставки, поэтому кадр
/// собирается по частям, а недособранные через окно выбрасываются: ждать
/// потерянный фрагмент в реальном времени бессмысленно.
#[derive(Default)]
struct Reassembly {
    pending: HashMap<(Id, Track, u32), Partial>,
    /// Самый свежий номер кадра, который видели от каждой дорожки.
    /// Окно чистки считается от него, а не от номера текущего пакета: иначе
    /// один опоздавший фрагмент выбрасывал бы всё, что новее его.
    newest: HashMap<(Id, Track), u32>,
}

struct Partial {
    parts: Vec<Option<Vec<u8>>>,
    filled: usize,
    keyframe: bool,
    ts: i64,
}

impl Reassembly {
    fn push(&mut self, packet: MediaPacket) -> Option<Vec<u8>> {
        if packet.parts == 0 || packet.part >= packet.parts {
            return None;
        }

        // Однофрагментный кадр — самый частый случай для звука, не заводим запись.
        if packet.parts == 1 {
            return Some(encode_for_ui(
                packet.author,
                packet.track,
                packet.keyframe,
                packet.ts,
                &packet.data,
            ));
        }

        let key = (packet.author, packet.track, packet.seq);
        let entry = self.pending.entry(key).or_insert_with(|| Partial {
            parts: vec![None; packet.parts as usize],
            filled: 0,
            keyframe: packet.keyframe,
            ts: packet.ts,
        });

        if entry.parts.len() != packet.parts as usize {
            return None; // пир противоречит сам себе
        }
        if entry.parts[packet.part as usize].is_none() {
            entry.filled += 1;
            entry.parts[packet.part as usize] = Some(packet.data);
        }

        if entry.filled == entry.parts.len() {
            let done = self.pending.remove(&key).expect("запись только что была");
            let mut data = Vec::new();
            for part in done.parts.into_iter().flatten() {
                data.extend_from_slice(&part);
            }
            self.forget_older_than(packet.author, packet.track, packet.seq);
            return Some(encode_for_ui(
                packet.author,
                packet.track,
                done.keyframe,
                done.ts,
                &data,
            ));
        }

        self.forget_older_than(packet.author, packet.track, packet.seq);
        None
    }

    /// Выбрасываем то, что уже не догонит: иначе потерянный фрагмент держал бы
    /// память до конца звонка.
    fn forget_older_than(&mut self, author: Id, track: Track, seq: u32) {
        let newest = self.newest.entry((author, track)).or_insert(seq);
        // Пакеты приходят вперемешку, и опоздавший не должен сдвигать границу
        // назад: иначе он выбросит из сборки всё, что новее его самого.
        if seq > *newest {
            *newest = seq;
        }
        let newest = *newest;

        let window = REASSEMBLY_WINDOW as u32;
        self.pending.retain(|(a, t, s), _| {
            *a != author || *t != track || newest.saturating_sub(*s) < window
        });
    }

    fn clear(&mut self) {
        self.pending.clear();
    }
}

/// Приём входящих медиа-соединений.
#[derive(Clone)]
pub struct MediaProtocol {
    media: Arc<Media>,
}

impl MediaProtocol {
    pub fn new(media: Arc<Media>) -> Self {
        Self { media }
    }
}

impl std::fmt::Debug for MediaProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MediaProtocol")
    }
}

impl ProtocolHandler for MediaProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        self.media.clone().pump(connection).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(author: u8, seq: u32, part: u16, parts: u16, data: &[u8]) -> MediaPacket {
        MediaPacket {
            space: Id([1u8; 32]),
            channel: Id([2u8; 32]),
            author: Id([author; 32]),
            track: Track::Video,
            seq,
            part,
            parts,
            keyframe: true,
            ts: 1234,
            data: data.to_vec(),
        }
    }

    #[test]
    fn single_part_frame_passes_straight_through() {
        let mut r = Reassembly::default();
        let out = r.push(packet(1, 1, 0, 1, b"audio")).expect("кадр целиком");
        assert_eq!(&out[UI_HEADER..], b"audio");
        assert!(
            r.pending.is_empty(),
            "однофрагментный кадр не должен копиться"
        );
    }

    #[test]
    fn fragments_reassemble_in_order() {
        let mut r = Reassembly::default();
        assert!(r.push(packet(1, 7, 0, 3, b"aaa")).is_none());
        assert!(r.push(packet(1, 7, 2, 3, b"ccc")).is_none());
        let out = r.push(packet(1, 7, 1, 3, b"bbb")).expect("кадр собрался");
        assert_eq!(
            &out[UI_HEADER..],
            b"aaabbbccc",
            "порядок должен быть по индексу"
        );
    }

    #[test]
    fn duplicate_fragment_does_not_break_assembly() {
        let mut r = Reassembly::default();
        assert!(r.push(packet(1, 3, 0, 2, b"aa")).is_none());
        assert!(
            r.push(packet(1, 3, 0, 2, b"aa")).is_none(),
            "дубль не считается"
        );
        let out = r.push(packet(1, 3, 1, 2, b"bb")).expect("кадр собрался");
        assert_eq!(&out[UI_HEADER..], b"aabb");
    }

    #[test]
    fn lost_fragment_is_eventually_forgotten() {
        let mut r = Reassembly::default();
        r.push(packet(1, 1, 0, 2, b"aa"));
        assert_eq!(r.pending.len(), 1);

        // Кадры продолжают идти, потерянный уезжает за окно сборки.
        for seq in 2..(REASSEMBLY_WINDOW as u32 + 4) {
            r.push(packet(1, seq, 0, 2, b"xx"));
        }
        assert!(
            r.pending.len() < REASSEMBLY_WINDOW + 2,
            "недособранные кадры обязаны выбрасываться, накопилось {}",
            r.pending.len()
        );
    }

    #[test]
    fn late_packet_does_not_purge_newer_frames() {
        let mut r = Reassembly::default();
        // Ждём вторую половину свежего кадра.
        assert!(r.push(packet(1, 40, 0, 2, b"aa")).is_none());
        // Прилетает половинка давно устаревшего кадра.
        assert!(r.push(packet(1, 1, 0, 2, b"zz")).is_none());
        // Свежий кадр обязан дособраться, а не пропасть из-за опоздавшего.
        let out = r.push(packet(1, 40, 1, 2, b"bb"));
        assert!(
            out.is_some(),
            "опоздавший пакет не должен ронять свежие кадры"
        );
        assert_eq!(&out.unwrap()[UI_HEADER..], b"aabb");
    }

    #[test]
    fn nonsense_fragment_indices_are_rejected() {
        let mut r = Reassembly::default();
        assert!(
            r.push(packet(1, 1, 5, 3, b"x")).is_none(),
            "индекс вне диапазона"
        );
        assert!(
            r.push(packet(1, 1, 0, 0, b"x")).is_none(),
            "нулевое число частей"
        );
        assert!(r.pending.is_empty());
    }

    #[test]
    fn ui_frame_round_trips() {
        let raw = encode_for_ui(Id([9u8; 32]), Track::Audio, true, -42, b"payload");
        let (track, keyframe, ts, data) = decode_from_ui(&raw).unwrap();
        assert_eq!(track, Track::Audio);
        assert!(keyframe);
        assert_eq!(ts, -42);
        assert_eq!(data, b"payload");
    }

    #[test]
    fn short_or_unknown_ui_frame_is_rejected() {
        assert!(decode_from_ui(&[1, 0, 0]).is_err());
        let mut raw = encode_for_ui(Id::ZERO, Track::Video, false, 0, b"x");
        raw[0] = 99;
        assert!(decode_from_ui(&raw).is_err(), "чужая версия формата");
    }
}
