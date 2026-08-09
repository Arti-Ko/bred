//! Голосовые и видеозвонки.
//!
//! Комната — это голосовой канал: кто выставил `presence.voice`, тот в звонке.
//! Отдельного сигнального сервера нет и не нужно — присутствие и так разлетается
//! по gossip, а адреса умеет находить iroh.
//!
//! Топология — полная сетка (mesh): каждый шлёт свой поток каждому. Кадр при
//! этом кодируется и шифруется **один раз**, а рассылается готовыми байтами,
//! поэтому процессор от числа собеседников не зависит — растёт только исходящий
//! канал. Именно он и есть предел сетки, и держит его governor битрейта ниже.
//!
//! Транспорт звука — QUIC-датаграммы: для реального времени потерять кадр лучше,
//! чем ждать его ретрансмита и уронить весь звук. Картинка идёт надёжными
//! потоками, но у камеры и у экрана они **разные**: иначе ключевой кадр
//! демонстрации на двести килобайт вставал поперёк дороги всему видео.

use anyhow::{anyhow, Result};
use bytes::Bytes;
use chacha20poly1305::{
    aead::{AeadInPlace, KeyInit},
    ChaCha20Poly1305, Nonce, Tag,
};
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr, EndpointId,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc::Sender;

use super::{
    ctx::{Ctx, Notice},
    wire::Presence,
};
use crate::domain::{ChannelId, Id, SpaceId};

/// Версия поднята вместе с форматом кадра.
///
/// Старый и новый БРЕД не должны договориться о медиа: разойдись они по формату
/// заголовка — и звонок выглядел бы установленным, но состоял бы из тишины и
/// чёрных прямоугольников. Разные ALPN честнее: связь просто не поднимется.
pub const MEDIA_ALPN: &[u8] = b"bred/media/2";

/// Как часто сверяем состав комнаты с присутствием.
const RECONCILE: Duration = Duration::from_secs(2);
/// Через сколько дозванивается и вторая сторона.
///
/// Обычно соединение поднимает тот, чей идентификатор меньше — иначе получили бы
/// две встречные. Но если у него дозвон не проходит (бывает при недружелюбном
/// NAT ровно с одной стороны), ждать его вечно нельзя: связи не будет вообще.
/// Поэтому через несколько секунд право набирать получает и вторая сторона.
const DIAL_FALLBACK: Duration = Duration::from_secs(5);
/// Сколько кадров держим в сборке, прежде чем признать их потерянными.
const REASSEMBLY_WINDOW: usize = 24;
/// Запас на служебные поля и шифрование внутри датаграммы.
const OVERHEAD: usize = 64;
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

// ── формат провода ──────────────────────────────────────────────────────────

/// Заголовок кадра на проводе.
///
/// Раньше здесь ехал `postcard` со структурой, в которой лежали три полных
/// 32-байтных идентификатора: пространство, канал и автор. Сто двенадцать байт
/// служебного плюс двадцать четыре байта случайного nonce — на восемьдесят
/// байт голоса. Опус на 32 кбит/с занимал в эфире девяносто три.
///
/// Ни одно из трёх полей не нужно. Соединение уже привязано к паре пиров и к
/// своему ALPN, комната известна из `call`, а автор — из `remote_id()`, где его
/// подтвердил QUIC. Раньше автор брался из тела пакета, и любой участник
/// пространства мог представиться кем угодно; теперь подделать его нельзя.
///
/// `[0]` дорожка и признак ключевого кадра, `[1..5]` метка канала,
/// `[5..9]` номер кадра, `[9..11]` номер части, `[11..13]` всего частей,
/// `[13..21]` время в микросекундах.
const HEADER: usize = 21;
/// Счётчик nonce на проводе.
const NONCE_WIRE: usize = 8;
/// Тег Poly1305.
const TAG: usize = 16;
/// Столько служебного приходится на каждую датаграмму: было 152, стало 45.
pub const WIRE_OVERHEAD: usize = NONCE_WIRE + HEADER + TAG;

/// Разобранный заголовок.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Head {
    track: Track,
    keyframe: bool,
    channel_tag: [u8; 4],
    seq: u32,
    part: u16,
    parts: u16,
    ts: i64,
}

impl Head {
    fn write(&self, out: &mut Vec<u8>) {
        out.push(self.track.code() | if self.keyframe { 0x80 } else { 0 });
        out.extend_from_slice(&self.channel_tag);
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.part.to_le_bytes());
        out.extend_from_slice(&self.parts.to_le_bytes());
        out.extend_from_slice(&self.ts.to_le_bytes());
    }

    fn read(raw: &[u8]) -> Option<Self> {
        if raw.len() < HEADER {
            return None;
        }
        Some(Self {
            track: Track::from_code(raw[0] & 0x7f)?,
            keyframe: raw[0] & 0x80 != 0,
            channel_tag: raw[1..5].try_into().ok()?,
            seq: u32::from_le_bytes(raw[5..9].try_into().ok()?),
            part: u16::from_le_bytes(raw[9..11].try_into().ok()?),
            parts: u16::from_le_bytes(raw[11..13].try_into().ok()?),
            ts: i64::from_le_bytes(raw[13..21].try_into().ok()?),
        })
    }
}

/// Метка канала: четырёх байт хватает, чтобы отбросить кадр из прошлой комнаты,
/// и не хватает, чтобы по ней что-то узнать снаружи — она и так под шифром.
fn channel_tag(channel: ChannelId) -> [u8; 4] {
    [channel.0[0], channel.0[1], channel.0[2], channel.0[3]]
}

/// Ключ дорожки одного отправителя.
///
/// Свой ключ на каждого отправителя нужен, чтобы nonce можно было считать
/// счётчиком, а не брать случайным. Общий ключ пространства плюс счётчик с нуля
/// у обеих сторон — это повтор пары (ключ, nonce), а для ChaCha20-Poly1305 повтор
/// nonce означает потерю всей защиты разом.
fn media_key(space_key: &[u8; 32], sender: Id) -> ChaCha20Poly1305 {
    let mut material = [0u8; 64];
    material[..32].copy_from_slice(space_key);
    material[32..].copy_from_slice(&sender.0);
    let derived = blake3::derive_key("bred media key v1", &material);
    ChaCha20Poly1305::new((&derived).into())
}

fn nonce_of(counter: u64) -> Nonce {
    let mut raw = [0u8; 12];
    raw[4..].copy_from_slice(&counter.to_le_bytes());
    *Nonce::from_slice(&raw)
}

/// Собрать датаграмму: счётчик открыто, заголовок и тело под шифром.
///
/// Одно выделение на кадр и ни одной лишней копии: раньше `seal()` строил
/// `postcard`-вектор, потом второй вектор под nonce, потом копировал в него
/// результат шифрования — три аллокации и два прохода по памяти на каждый из
/// полусотни пакетов в секунду.
fn seal_media(cipher: &ChaCha20Poly1305, counter: u64, head: &Head, data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(WIRE_OVERHEAD + data.len());
    out.extend_from_slice(&counter.to_le_bytes());
    head.write(&mut out);
    out.extend_from_slice(data);

    let tag = cipher
        .encrypt_in_place_detached(&nonce_of(counter), &[], &mut out[NONCE_WIRE..])
        .map_err(|_| anyhow!("не удалось зашифровать кадр"))?;
    out.extend_from_slice(&tag);
    Ok(out)
}

/// Разобрать датаграмму обратно. Возвращает заголовок и расшифрованное тело.
fn open_media(cipher: &ChaCha20Poly1305, raw: &[u8]) -> Option<(Head, Vec<u8>)> {
    if raw.len() < WIRE_OVERHEAD {
        return None;
    }
    let counter = u64::from_le_bytes(raw[..NONCE_WIRE].try_into().ok()?);
    let (body, tag) = raw[NONCE_WIRE..].split_at(raw.len() - NONCE_WIRE - TAG);

    let mut plain = body.to_vec();
    cipher
        .decrypt_in_place_detached(&nonce_of(counter), &[], &mut plain, Tag::from_slice(tag))
        .ok()?;

    let head = Head::read(&plain)?;
    plain.drain(..HEADER);
    Some((head, plain))
}

// ── дорожки ─────────────────────────────────────────────────────────────────

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

    /// Имя для интерфейса — им governor называет дорожку, которой сменил битрейт.
    pub fn name(self) -> &'static str {
        match self {
            Track::Audio => "audio",
            Track::Video => "video",
            Track::Screen => "screen",
            Track::ScreenAudio => "screen-audio",
        }
    }
}

/// Заголовок обмена с интерфейсом. Компактный бинарный формат: гонять кадры
/// видео через JSON было бы вчетверо дороже.
///
/// `[0]` версия, `[1]` дорожка, `[2]` флаги, `[3..35]` автор, `[35..43]` время,
/// `[43..47]` номер кадра — по нему вебвью видит потерю и растит буфер вместо
/// того, чтобы булькать.
pub const UI_HEADER: usize = 47;
const UI_VERSION: u8 = 2;

pub fn encode_for_ui(
    author: Id,
    track: Track,
    keyframe: bool,
    ts: i64,
    seq: u32,
    data: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(UI_HEADER + data.len());
    out.push(UI_VERSION);
    out.push(track.code());
    out.push(u8::from(keyframe));
    out.extend_from_slice(&author.0);
    out.extend_from_slice(&ts.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
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

// ── очередь картинки ────────────────────────────────────────────────────────

struct Frame {
    bytes: Bytes,
    keyframe: bool,
}

/// Очередь кадров одной дорожки к одному собеседнику.
///
/// Обычный `mpsc` с `try_send` ронял то, что прилетело последним, — в том числе
/// ключевые кадры. Потерянный ключевой означает секунды рассыпающейся картинки:
/// следующий приходит только по расписанию. Здесь наоборот: ключевой кадр
/// **вытесняет всю очередь**, потому что всё, что стояло перед ним, декодеру уже
/// не понадобится, а обычный кадр в переполненную очередь просто не встаёт.
struct FrameQueue {
    inner: Mutex<VecDeque<Frame>>,
    ready: tokio::sync::Notify,
    closed: AtomicBool,
    /// Сколько кадров пришлось выбросить — это и есть сигнал governor'у, что
    /// исходящий канал не тянет заданный битрейт.
    dropped: AtomicU64,
}

impl FrameQueue {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(VecDeque::with_capacity(VIDEO_QUEUE)),
            ready: tokio::sync::Notify::new(),
            closed: AtomicBool::new(false),
            dropped: AtomicU64::new(0),
        })
    }

    fn push(&self, bytes: Bytes, keyframe: bool) {
        {
            let mut queue = self.inner.lock();
            if keyframe {
                // Всё, что стояло до ключевого, декодер всё равно пропустит.
                self.dropped
                    .fetch_add(queue.len() as u64, Ordering::Relaxed);
                queue.clear();
            } else if queue.len() >= VIDEO_QUEUE {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
            queue.push_back(Frame { bytes, keyframe });
        }
        self.ready.notify_one();
    }

    fn pop(&self) -> Option<Frame> {
        self.inner.lock().pop_front()
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        // Именно `notify_one`, а не `notify_waiters`: второй будит только тех,
        // кто уже ждёт, и закрытие в зазоре между «очередь пуста» и «ждём»
        // осталось бы незамеченным. `notify_one` оставляет разрешение впрок.
        self.ready.notify_one();
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// Забрать и обнулить счётчик выброшенных.
    fn take_dropped(&self) -> u64 {
        self.dropped.swap(0, Ordering::Relaxed)
    }
}

// ── governor битрейта ───────────────────────────────────────────────────────

/// Как часто пересчитываем битрейт.
const GOVERNOR_TICK: Duration = Duration::from_secs(2);
/// Столько тиков без потерь — и можно осторожно прибавить.
const CALM_TICKS: u32 = 2;

/// Потолки: столько дорожка получит, когда собеседник один и канал свободен.
const CEILING_VIDEO: u32 = 900_000;
const CEILING_SCREEN: u32 = 2_500_000;
/// Полы: ниже картинка бессмысленна, лучше её выключить, чем показывать кашу.
const FLOOR_VIDEO: u32 = 120_000;
const FLOOR_SCREEN: u32 = 350_000;

/// Состояние подбора битрейта по одной дорожке.
struct Governor {
    current: u32,
    calm: u32,
}

// ── соединение с собеседником ───────────────────────────────────────────────

/// Связь с одним собеседником.
struct Peer {
    /// Номер попытки: по нему отличаем своё соединение от того, которым его
    /// успели заменить. Иначе завершение старой сессии выкидывало из таблицы
    /// живую новую, и связь пропадала на ровном месте.
    link: u64,
    connection: Connection,
    video: Arc<FrameQueue>,
    screen: Arc<FrameQueue>,
}

/// Всё, что нужно горячему пути приёма, — без единой блокировки.
struct Link {
    author: Id,
    cipher: ChaCha20Poly1305,
    channel_tag: [u8; 4],
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
    /// Шифр отправителя и метка комнаты — считаются один раз на вход в звонок,
    /// а не на каждый из полусотни пакетов в секунду.
    ///
    /// За `Arc` он спрятан не ради экономии: `broadcast` обязан отпустить эту
    /// блокировку до того, как возьмёт `peers`, иначе получается встречный
    /// порядок захвата с `join`, который берёт их наоборот. Дешёвый клон
    /// указателя — самый простой способ не держать две блокировки разом.
    sending: RwLock<Option<(Arc<ChaCha20Poly1305>, [u8; 4])>>,
    /// Счётчик nonce общий на все соединения: кадр шифруется один раз и уходит
    /// всем, поэтому и номер у него должен быть один.
    nonce: AtomicU64,
    /// Куда отдавать собранные кадры — интерфейсу.
    sink: RwLock<Option<Sender<Vec<u8>>>>,
    /// Сколько кадров выброшено из-за переполнения — видно в логах.
    dropped: Mutex<u64>,
    /// Кого уже пробуем набрать и с какого момента.
    dialing: Mutex<HashMap<Id, Instant>>,
    /// Подбор битрейта по дорожкам картинки.
    governors: Mutex<HashMap<Track, Governor>>,
    links: AtomicU64,
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
            sending: RwLock::new(None),
            nonce: AtomicU64::new(0),
            sink: RwLock::new(None),
            dropped: Mutex::new(0),
            dialing: Mutex::new(HashMap::new()),
            governors: Mutex::new(HashMap::new()),
            links: AtomicU64::new(0),
        });
        media.clone().reconcile_loop();
        media.clone().governor_loop();
        media
    }

    pub fn set_sink(&self, sink: Sender<Vec<u8>>) {
        *self.sink.write() = Some(sink);
    }

    pub fn active(&self) -> Option<(SpaceId, ChannelId)> {
        self.call.read().map(|c| (c.space, c.channel))
    }

    pub fn join(&self, space: SpaceId, channel: ChannelId) {
        // Смена комнаты — это новая сетка. Старые соединения рвём: иначе к нам
        // продолжали бы идти кадры из покинутого канала, а по ним же считался бы
        // битрейт.
        self.drop_peers("сменили комнату");

        *self.call.write() = Some(Call { space, channel });
        self.outgoing.lock().clear();
        self.dialing.lock().clear();
        self.governors.lock().clear();

        *self.sending.write() = self.ctx.space(space).map(|s| {
            (
                Arc::new(media_key(&s.key, self.ctx.identity.id())),
                channel_tag(channel),
            )
        });

        tracing::info!(channel = %channel.short(), "вошли в звонок");
    }

    pub fn leave(&self) {
        *self.call.write() = None;
        *self.sending.write() = None;
        self.drop_peers("вышел");
        self.reassembly.lock().clear();
        self.dialing.lock().clear();
        self.governors.lock().clear();
        tracing::info!("вышли из звонка");
    }

    fn drop_peers(&self, why: &str) {
        // Соединения рвём явно: иначе камера у собеседника ещё секунды будет
        // считать, что мы на связи.
        for (_, peer) in self.peers.write().drain() {
            peer.video.close();
            peer.screen.close();
            peer.connection.close(0u32.into(), why.as_bytes());
        }
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
        // Шифр и метку комнаты забираем сразу и отпускаем блокировку: держать её
        // до `peers` нельзя, иначе встречный порядок захвата с `join`.
        let Some((cipher, tag)) = ({
            let sending = self.sending.read();
            sending.as_ref().map(|(c, t)| (c.clone(), *t))
        }) else {
            return Ok(()); // не в звонке — кадр просто выбрасываем
        };

        let seq = {
            let mut counters = self.outgoing.lock();
            let counter = counters.entry(track).or_insert(0);
            *counter = counter.wrapping_add(1);
            *counter
        };

        let head = |part: u16, parts: u16| Head {
            track,
            keyframe,
            channel_tag: tag,
            seq,
            part,
            parts,
            ts,
        };

        // Картинка целиком в один надёжный поток: резать её незачем, а терять
        // куски нельзя. Звук — датаграммами, кусками по размеру пути.
        if track.reliable() {
            let counter = self.nonce.fetch_add(1, Ordering::Relaxed);
            let sealed = Bytes::from(seal_media(&cipher, counter, &head(0, 1), data)?);
            for peer in self.peers.read().values() {
                let queue = if track == Track::Screen {
                    &peer.screen
                } else {
                    &peer.video
                };
                queue.push(sealed.clone(), keyframe);
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
            let counter = self.nonce.fetch_add(1, Ordering::Relaxed);
            let sealed = Bytes::from(seal_media(
                &cipher,
                counter,
                &head(index as u16, parts),
                chunk,
            )?);
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
        let author = Id(*remote.as_bytes());

        // Ключ и метка комнаты — на всё время связи, а не на каждый пакет.
        let Some((call, key)) = ({
            let call = *self.call.read();
            call.and_then(|c| self.ctx.space(c.space).map(|s| (c, s.key)))
        }) else {
            connection.close(0u32.into(), "мы не в звонке".as_bytes());
            return;
        };
        let link = Arc::new(Link {
            author,
            cipher: media_key(&key, author),
            channel_tag: channel_tag(call.channel),
        });

        let id = self.links.fetch_add(1, Ordering::Relaxed);
        let video = FrameQueue::new();
        let screen = FrameQueue::new();

        if let Some(old) = self.peers.write().insert(
            remote,
            Peer {
                link: id,
                connection: connection.clone(),
                video: video.clone(),
                screen: screen.clone(),
            },
        ) {
            // Встречный дозвон: две стороны подняли связь одновременно. Лишнюю
            // закрываем, иначе кадры пошли бы в оба ствола сразу.
            old.video.close();
            old.screen.close();
            old.connection
                .close(0u32.into(), "есть другое соединение".as_bytes());
        }
        self.dialing.lock().remove(&author);

        // Новый собеседник не увидит картинку до ближайшего ключевого кадра, а
        // тот приходит по расписанию — до нескольких секунд чёрного экрана.
        // Поэтому просим кодировщик выдать ключевой прямо сейчас.
        let _ = self.ctx.notices.send(Notice::Keyframe);

        let writers = vec![
            tokio::spawn(write_track(connection.clone(), video.clone())),
            tokio::spawn(write_track(connection.clone(), screen.clone())),
        ];
        let reader = tokio::spawn(read_tracks(self.clone(), connection.clone(), link.clone()));

        loop {
            match connection.read_datagram().await {
                Ok(raw) => self.on_packet(&link, &raw),
                Err(err) => {
                    tracing::debug!(peer = %remote.fmt_short(), %err, "медиа-соединение закрыто");
                    break;
                }
            }
        }

        video.close();
        screen.close();
        for writer in writers {
            writer.abort();
        }
        reader.abort();

        // Убираем себя, только если нас ещё не подменили новым соединением.
        let mut peers = self.peers.write();
        if peers.get(&remote).map(|p| p.link) == Some(id) {
            peers.remove(&remote);
        }
    }

    fn on_packet(&self, link: &Link, raw: &[u8]) {
        let Some((head, data)) = open_media(&link.cipher, raw) else {
            return; // чужой ключ, подмена или мусор
        };
        if head.channel_tag != link.channel_tag {
            return; // кадр из другой комнаты
        }

        if let Some(frame) = self.reassembly.lock().push(link.author, head, data) {
            if let Some(sink) = self.sink.read().as_ref() {
                // Очередь переполнена — значит интерфейс не успевает. Кадр
                // выбрасываем: показать его с опозданием всё равно нельзя.
                if sink.try_send(frame).is_err() {
                    let mut dropped = self.dropped.lock();
                    *dropped += 1;
                    if *dropped % 300 == 1 {
                        tracing::debug!(dropped = *dropped, "интерфейс не успевает, кадры теряются");
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
                    let Ok(peer) = iroh::PublicKey::from_bytes(&participant.0) else {
                        continue;
                    };
                    if self.peers.read().contains_key(&peer) {
                        self.dialing.lock().remove(&participant);
                        continue;
                    }

                    // Обычно набирает тот, чей идентификатор меньше: иначе
                    // получили бы две встречные связи. Но если у него не
                    // выходит — а при одностороннем недружелюбном NAT так и
                    // бывает, — через несколько секунд пробуем и мы. Встречный
                    // дозвон разрулит `pump`: лишнее соединение он закроет.
                    let waiting = {
                        let mut dialing = self.dialing.lock();
                        let since = dialing.entry(participant).or_insert_with(Instant::now);
                        since.elapsed()
                    };
                    if me.0 > participant.0 && waiting < DIAL_FALLBACK {
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

    /// Подбор битрейта картинки под то, что реально утекает в провод.
    ///
    /// Раньше битрейт был прибит гвоздями: 500 кбит/с камера и 2.5 Мбит/с экран,
    /// независимо ни от числа собеседников, ни от канала. В сетке на шестерых с
    /// демонстрацией это пятнадцать мегабит исходящего — не деградация, а
    /// коллапс: очередь переполняется и дропает всё подряд, включая звук.
    ///
    /// Сигнал берём не из статистики транспорта, а из собственных очередей: если
    /// кадры пришлось выбрасывать, значит канал не принял заданный битрейт. Это
    /// прямее любого счётчика потерь и не зависит от версии iroh.
    fn governor_loop(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(GOVERNOR_TICK);
            loop {
                ticker.tick().await;
                if self.call.read().is_none() {
                    continue;
                }

                // Сетка платит за каждого собеседника отдельной копией кадра,
                // поэтому потолок на дорожку делится на число слушателей.
                let listeners = self.peers.read().len().max(1) as u32;

                for (track, ceiling, floor) in [
                    (Track::Video, CEILING_VIDEO, FLOOR_VIDEO),
                    (Track::Screen, CEILING_SCREEN, FLOOR_SCREEN),
                ] {
                    let dropped: u64 = self
                        .peers
                        .read()
                        .values()
                        .map(|p| {
                            if track == Track::Screen {
                                p.screen.take_dropped()
                            } else {
                                p.video.take_dropped()
                            }
                        })
                        .sum();

                    let share = (ceiling / listeners).max(floor);
                    let mut governors = self.governors.lock();
                    let state = governors.entry(track).or_insert(Governor {
                        current: share,
                        calm: 0,
                    });

                    let next = if dropped > 0 {
                        state.calm = 0;
                        // Отступаем заметно: мелкий шаг вниз на переполненном
                        // канале означает ещё несколько секунд рассыпающейся
                        // картинки, пока подбор доедет до реального потолка.
                        (state.current * 4 / 5).max(floor)
                    } else {
                        state.calm += 1;
                        if state.calm < CALM_TICKS {
                            state.current
                        } else {
                            state.calm = 0;
                            // Наверх — осторожно: канал мог освободиться на миг.
                            (state.current + state.current / 10).min(share)
                        }
                    };

                    if next != state.current {
                        state.current = next;
                        let _ = self.ctx.notices.send(Notice::Bitrate {
                            track: track.name(),
                            bps: next,
                        });
                    }
                }
            }
        });
    }
}

/// Отправка кадров одной дорожки — одним долгоживущим потоком, кадр за кадром.
///
/// Не поток на кадр: доставку это давало, а порядок — нет. Кодек не терпит
/// перестановки, дельта-кадр раньше своего предшественника превращает картинку
/// в кашу, которая не восстанавливается. Здесь порядок гарантирован самим QUIC.
///
/// И не один поток на всё: камера и демонстрация экрана раньше делили ствол, и
/// ключевой кадр экрана на двести килобайт вставал поперёк дороги камере.
async fn write_track(connection: Connection, queue: Arc<FrameQueue>) {
    let Ok(mut stream) = connection.open_uni().await else {
        return;
    };
    loop {
        let Some(frame) = queue.pop() else {
            if queue.is_closed() {
                break;
            }
            queue.ready.notified().await;
            continue;
        };
        let header = (frame.bytes.len() as u32).to_le_bytes();
        if stream.write_all(&header).await.is_err() || stream.write_all(&frame.bytes).await.is_err()
        {
            break;
        }
    }
    let _ = stream.finish();
}

/// Приём кадров картинки. Потоков теперь несколько — по одному на дорожку,
/// поэтому принимаем их в цикле, а каждый читаем строго последовательно:
/// порядок обработки так же важен, как порядок доставки.
async fn read_tracks(media: Arc<Media>, connection: Connection, link: Arc<Link>) {
    while let Ok(stream) = connection.accept_uni().await {
        tokio::spawn(read_frames(media.clone(), stream, link.clone()));
    }
}

async fn read_frames(media: Arc<Media>, mut stream: iroh::endpoint::RecvStream, link: Arc<Link>) {
    let mut header = [0u8; 4];
    loop {
        if stream.read_exact(&mut header).await.is_err() {
            break;
        }
        let len = u32::from_le_bytes(header) as usize;
        if len == 0 || len > MAX_FRAME_BYTES {
            break; // мусор или попытка съесть память
        }
        let mut frame = vec![0u8; len];
        if stream.read_exact(&mut frame).await.is_err() {
            break;
        }
        media.on_packet(&link, &frame);
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
    fn push(&mut self, author: Id, head: Head, data: Vec<u8>) -> Option<Vec<u8>> {
        if head.parts == 0 || head.part >= head.parts {
            return None;
        }

        // Однофрагментный кадр — самый частый случай для звука, не заводим запись.
        if head.parts == 1 {
            return Some(encode_for_ui(
                author,
                head.track,
                head.keyframe,
                head.ts,
                head.seq,
                &data,
            ));
        }

        let key = (author, head.track, head.seq);
        let entry = self.pending.entry(key).or_insert_with(|| Partial {
            parts: vec![None; head.parts as usize],
            filled: 0,
            keyframe: head.keyframe,
            ts: head.ts,
        });

        if entry.parts.len() != head.parts as usize {
            return None; // пир противоречит сам себе
        }
        if entry.parts[head.part as usize].is_none() {
            entry.filled += 1;
            entry.parts[head.part as usize] = Some(data);
        }

        if entry.filled == entry.parts.len() {
            let done = self.pending.remove(&key).expect("запись только что была");
            let mut whole = Vec::new();
            for part in done.parts.into_iter().flatten() {
                whole.extend_from_slice(&part);
            }
            self.forget_older_than(author, head.track, head.seq);
            return Some(encode_for_ui(
                author,
                head.track,
                done.keyframe,
                done.ts,
                head.seq,
                &whole,
            ));
        }

        self.forget_older_than(author, head.track, head.seq);
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
        self.newest.clear();
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

    fn head(seq: u32, part: u16, parts: u16) -> Head {
        Head {
            track: Track::Video,
            keyframe: true,
            channel_tag: [2, 2, 2, 2],
            seq,
            part,
            parts,
            ts: 1234,
        }
    }

    fn cipher() -> ChaCha20Poly1305 {
        media_key(&[7u8; 32], Id([9u8; 32]))
    }

    fn author(n: u8) -> Id {
        Id([n; 32])
    }

    // ── формат провода ──────────────────────────────────────────────────────

    #[test]
    fn header_round_trips() {
        let original = Head {
            track: Track::ScreenAudio,
            keyframe: false,
            channel_tag: [1, 2, 3, 4],
            seq: 4_000_000_000,
            part: 7,
            parts: 9,
            ts: -1_234_567_890,
        };
        let mut raw = Vec::new();
        original.write(&mut raw);
        assert_eq!(raw.len(), HEADER, "заголовок обязан быть ровно {HEADER}");
        assert_eq!(Head::read(&raw), Some(original));
    }

    #[test]
    fn sealed_frame_round_trips() {
        let cipher = cipher();
        let sealed = seal_media(&cipher, 42, &head(1, 0, 1), b"payload").unwrap();
        let (out, data) = open_media(&cipher, &sealed).expect("кадр открывается");
        assert_eq!(out, head(1, 0, 1));
        assert_eq!(data, b"payload");
    }

    #[test]
    fn overhead_is_what_we_promised() {
        let sealed = seal_media(&cipher(), 0, &head(1, 0, 1), &[0u8; 80]).unwrap();
        assert_eq!(
            sealed.len() - 80,
            WIRE_OVERHEAD,
            "служебного на кадр должно быть {WIRE_OVERHEAD} байт"
        );
        // Ради этого всё и затевалось: раньше на восемьдесят байт голоса
        // приходилось сто пятьдесят два байта служебного.
        assert!(
            sealed.len() < 130,
            "датаграмма со звуком раздулась: {}",
            sealed.len()
        );
    }

    #[test]
    fn frame_from_another_sender_does_not_open() {
        let sealed = seal_media(&cipher(), 1, &head(1, 0, 1), b"secret").unwrap();
        // Ключ выводится из идентификатора отправителя, а тот берётся из
        // соединения. Подставить чужое авторство больше нельзя.
        let someone_else = media_key(&[7u8; 32], Id([1u8; 32]));
        assert!(open_media(&someone_else, &sealed).is_none());
    }

    #[test]
    fn tampered_frame_is_rejected() {
        let cipher = cipher();
        let mut sealed = seal_media(&cipher, 1, &head(1, 0, 1), b"payload").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0xff;
        assert!(open_media(&cipher, &sealed).is_none());
    }

    #[test]
    fn short_frame_is_rejected() {
        assert!(open_media(&cipher(), &[0u8; 8]).is_none());
        assert!(open_media(&cipher(), &[]).is_none());
    }

    #[test]
    fn every_frame_gets_its_own_nonce() {
        let cipher = cipher();
        let first = seal_media(&cipher, 0, &head(1, 0, 1), b"same").unwrap();
        let second = seal_media(&cipher, 1, &head(1, 0, 1), b"same").unwrap();
        assert_ne!(
            first, second,
            "одинаковый шифртекст означал бы повтор nonce, а с ним потерю защиты"
        );
    }

    // ── очередь картинки ────────────────────────────────────────────────────

    #[test]
    fn keyframe_clears_the_backlog() {
        let queue = FrameQueue::new();
        for _ in 0..VIDEO_QUEUE {
            queue.push(Bytes::from_static(b"delta"), false);
        }
        queue.push(Bytes::from_static(b"key"), true);

        let frame = queue.pop().expect("что-то в очереди есть");
        assert!(
            frame.keyframe,
            "ключевой кадр обязан вытеснить накопившееся, а не встать за ним"
        );
        assert!(queue.pop().is_none(), "после вытеснения очередь пуста");
    }

    #[test]
    fn full_queue_drops_deltas_and_counts_them() {
        let queue = FrameQueue::new();
        for _ in 0..VIDEO_QUEUE * 2 {
            queue.push(Bytes::from_static(b"delta"), false);
        }
        let mut count = 0;
        while queue.pop().is_some() {
            count += 1;
        }
        assert_eq!(count, VIDEO_QUEUE, "очередь не должна расти без предела");
        assert_eq!(
            queue.take_dropped(),
            VIDEO_QUEUE as u64,
            "выброшенное обязано считаться: по нему governor снижает битрейт"
        );
    }

    // ── сборка кадров ───────────────────────────────────────────────────────

    #[test]
    fn single_part_frame_passes_straight_through() {
        let mut r = Reassembly::default();
        let out = r
            .push(author(1), head(1, 0, 1), b"audio".to_vec())
            .expect("кадр целиком");
        assert_eq!(&out[UI_HEADER..], b"audio");
        assert!(
            r.pending.is_empty(),
            "однофрагментный кадр не должен копиться"
        );
    }

    #[test]
    fn fragments_reassemble_in_order() {
        let mut r = Reassembly::default();
        assert!(r.push(author(1), head(7, 0, 3), b"aaa".to_vec()).is_none());
        assert!(r.push(author(1), head(7, 2, 3), b"ccc".to_vec()).is_none());
        let out = r
            .push(author(1), head(7, 1, 3), b"bbb".to_vec())
            .expect("кадр собрался");
        assert_eq!(
            &out[UI_HEADER..],
            b"aaabbbccc",
            "порядок должен быть по индексу"
        );
    }

    #[test]
    fn duplicate_fragment_does_not_break_assembly() {
        let mut r = Reassembly::default();
        assert!(r.push(author(1), head(3, 0, 2), b"aa".to_vec()).is_none());
        assert!(
            r.push(author(1), head(3, 0, 2), b"aa".to_vec()).is_none(),
            "дубль не считается"
        );
        let out = r
            .push(author(1), head(3, 1, 2), b"bb".to_vec())
            .expect("кадр собрался");
        assert_eq!(&out[UI_HEADER..], b"aabb");
    }

    #[test]
    fn lost_fragment_is_eventually_forgotten() {
        let mut r = Reassembly::default();
        r.push(author(1), head(1, 0, 2), b"aa".to_vec());
        assert_eq!(r.pending.len(), 1);

        // Кадры продолжают идти, потерянный уезжает за окно сборки.
        for seq in 2..(REASSEMBLY_WINDOW as u32 + 4) {
            r.push(author(1), head(seq, 0, 2), b"xx".to_vec());
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
        assert!(r.push(author(1), head(40, 0, 2), b"aa".to_vec()).is_none());
        // Прилетает половинка давно устаревшего кадра.
        assert!(r.push(author(1), head(1, 0, 2), b"zz".to_vec()).is_none());
        // Свежий кадр обязан дособраться, а не пропасть из-за опоздавшего.
        let out = r.push(author(1), head(40, 1, 2), b"bb".to_vec());
        assert!(
            out.is_some(),
            "опоздавший пакет не должен ронять свежие кадры"
        );
        assert_eq!(&out.unwrap()[UI_HEADER..], b"aabb");
    }

    #[test]
    fn frames_from_two_people_do_not_mix() {
        let mut r = Reassembly::default();
        assert!(r.push(author(1), head(5, 0, 2), b"aa".to_vec()).is_none());
        assert!(r.push(author(2), head(5, 0, 2), b"XX".to_vec()).is_none());

        let first = r
            .push(author(1), head(5, 1, 2), b"bb".to_vec())
            .expect("кадр первого собрался");
        assert_eq!(&first[UI_HEADER..], b"aabb", "куски не должны перепутаться");
        assert_eq!(
            &first[3..35],
            &author(1).0,
            "автор берётся из соединения, а не из пакета"
        );
    }

    #[test]
    fn nonsense_fragment_indices_are_rejected() {
        let mut r = Reassembly::default();
        assert!(
            r.push(author(1), head(1, 5, 3), b"x".to_vec()).is_none(),
            "индекс вне диапазона"
        );
        assert!(
            r.push(author(1), head(1, 0, 0), b"x".to_vec()).is_none(),
            "нулевое число частей"
        );
        assert!(r.pending.is_empty());
    }

    #[test]
    fn ui_frame_round_trips() {
        let raw = encode_for_ui(Id([9u8; 32]), Track::Audio, true, -42, 77, b"payload");
        let (track, keyframe, ts, data) = decode_from_ui(&raw).unwrap();
        assert_eq!(track, Track::Audio);
        assert!(keyframe);
        assert_eq!(ts, -42);
        assert_eq!(data, b"payload");
        assert_eq!(
            u32::from_le_bytes(raw[43..47].try_into().unwrap()),
            77,
            "номер кадра нужен вебвью, чтобы заметить потерю и вырастить буфер"
        );
    }

    #[test]
    fn short_or_unknown_ui_frame_is_rejected() {
        assert!(decode_from_ui(&[1, 0, 0]).is_err());
        let mut raw = encode_for_ui(Id::ZERO, Track::Video, false, 0, 0, b"x");
        raw[0] = 99;
        assert!(decode_from_ui(&raw).is_err(), "чужая версия формата");
    }
}
