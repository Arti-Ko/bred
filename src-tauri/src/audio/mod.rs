//! Звук звонка в ядре: кодирование, приём, восстановление потерь и сведение.
//!
//! Раньше всё это делал вебвью: WebCodecs кодировал Opus на 32 кбит/с, а на
//! приёме каждый кадр декодировался по приходу и ставился в очередь отдельным
//! узлом WebAudio — полсотни узлов в секунду на каждого собеседника, в главном
//! потоке, рядом с перерисовкой ленты. Пропавший кадр выпадал без следа: ни
//! восстановить его по избыточности, ни замаскировать WebCodecs не умеет.
//!
//! Теперь вебвью только снимает микрофон (системное эхоподавление остаётся его
//! заслугой) и играет один готовый поток. Всё между — здесь:
//!
//! * кодирование Opus 1.6: голос 64 кбит/с с глубокой избыточностью (DRED),
//!   музыка 128 кбит/с в стерео;
//! * приём: на каждый поток свой джиттер-буфер с подбором задержки под канал
//!   и восстановлением пропавших кадров ([`jitter`]);
//! * сведение всех голосов в один поток со своей громкостью у каждого
//!   собеседника — его и забирает вебвью.
//!
//! Часы у сведения свои: отдельный поток с шагом в двадцать миллисекунд. На
//! рабочем пуле tokio шаг плавал бы вместе с нагрузкой сети.

pub mod codec;
pub mod jitter;

use anyhow::{anyhow, Result};
use parking_lot::{Mutex, RwLock};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc::{Sender, UnboundedSender};

use crate::{
    domain::Id,
    net::{
        media::{AudioPacket, Track},
        Notice,
    },
};
use codec::{Profile, VoiceDecoder, VoiceEncoder, FRAME};
use jitter::{rms, Jitter};

/// Шаг сведения.
const TICK: Duration = Duration::from_millis(20);
/// Отставание, после которого часы сведения не догоняют, а начинают заново:
/// система спала, и играть накопленное за это время незачем.
const MAX_LAG: Duration = Duration::from_millis(200);
/// Через сколько молчания поток собеседника забывается вместе с декодером.
const STREAM_IDLE: Duration = Duration::from_secs(10);
/// Громче этого — человек говорит. Тот же порог, что был в вебвью.
const SPEAKING_RMS: f32 = 0.012;
/// Окно подсветки говорящего — пять тиков, сто миллисекунд.
const LEVEL_WINDOW: usize = 5;
/// Подсветку подтверждаем не реже этого, пока кто-то говорит: интерфейс гасит
/// её сам через четыреста миллисекунд.
const SPEAKING_REFRESH: Duration = Duration::from_millis(250);
/// Сколько готовых кадров ждут вебвью. Больше — уже задержка, а не запас.
pub const PLAYOUT_QUEUE: usize = 16;
/// Версия формата кадра для вебвью.
const OUT_VERSION: u8 = 1;
/// Заголовок кадра для вебвью: версия, число каналов, отсчётов на канал.
pub const OUT_HEADER: usize = 4;

/// Куда уходят закодированные кадры — в сеть: дорожка, кодек, время, байты.
pub type Outlet = Arc<dyn Fn(Track, u8, i64, &[u8]) + Send + Sync>;

/// Пришедший кадр, ещё не разложенный по буферам.
struct Arrival {
    author: Id,
    track: Track,
    seq: u32,
    data: Vec<u8>,
    at_ms: f64,
}

/// Исходящая дорожка: кодировщик и хвост, не набравший целого кадра.
struct Outgoing {
    encoder: VoiceEncoder,
    pending: Vec<f32>,
    /// Время кадра в микросекундах — едет в заголовке, как раньше у WebCodecs.
    ts: i64,
}

struct Shared {
    /// Кадры между сетью и часами сведения. Сеть только кладёт — разбор и
    /// декодирование идут уже в потоке сведения, без этого замка.
    inbox: Mutex<Vec<Arrival>>,
    /// Громкость каждого собеседника у себя: 1 — как есть.
    gains: RwLock<HashMap<Id, f32>>,
    /// Громкость общего плеера, `f32` в битах.
    music_gain: AtomicU32,
    sink: RwLock<Option<Sender<Vec<u8>>>>,
    send: RwLock<Option<Outlet>>,
    outgoing: Mutex<HashMap<Track, Outgoing>>,
    notices: UnboundedSender<Notice>,
    /// Попросили забыть все входящие потоки — вышли из звонка.
    reset: AtomicBool,
    epoch: Instant,
}

/// Движок звука. Дешёвый клон: внутри один `Arc`.
#[derive(Clone)]
pub struct AudioEngine {
    shared: Arc<Shared>,
}

impl AudioEngine {
    pub fn new(notices: UnboundedSender<Notice>) -> Self {
        let shared = Arc::new(Shared {
            inbox: Mutex::new(Vec::new()),
            gains: RwLock::new(HashMap::new()),
            music_gain: AtomicU32::new(1.0f32.to_bits()),
            sink: RwLock::new(None),
            send: RwLock::new(None),
            outgoing: Mutex::new(HashMap::new()),
            notices,
            reset: AtomicBool::new(false),
            epoch: Instant::now(),
        });
        let clock = shared.clone();
        if let Err(err) = std::thread::Builder::new()
            .name("bred-voice".into())
            .spawn(move || run_playout(clock))
        {
            tracing::error!(%err, "поток звука не поднялся — входящего звука не будет");
        }
        Self { shared }
    }

    /// Куда отдавать сведённый звук — в вебвью.
    pub fn set_sink(&self, sink: Sender<Vec<u8>>) {
        *self.shared.sink.write() = Some(sink);
    }

    /// Куда отдавать закодированные кадры — в сеть.
    pub fn set_send(&self, send: Outlet) {
        *self.shared.send.write() = Some(send);
    }

    /// Кадр от собеседника. Зовётся из сети: только положить и уйти.
    pub fn push(&self, packet: AudioPacket) {
        let at_ms = self.shared.epoch.elapsed().as_secs_f64() * 1000.0;
        self.shared.inbox.lock().push(Arrival {
            author: packet.author,
            track: packet.track,
            seq: packet.seq,
            data: packet.data,
            at_ms,
        });
    }

    /// Свой звук — в сеть. Отсчёты чередуются по каналам; мелкие куски
    /// копятся до целого кадра.
    pub fn send_pcm(&self, track: Track, pcm: &[f32]) -> Result<()> {
        let profile = match track {
            Track::Audio => Profile::Voice,
            Track::ScreenAudio => Profile::Screen,
            Track::Music => Profile::Music,
            _ => return Err(anyhow!("дорожка {} — не звук", track.name())),
        };
        let Some(send) = self.shared.send.read().clone() else {
            return Ok(()); // сеть ещё не поднялась
        };

        let mut outgoing = self.shared.outgoing.lock();
        let lane = match outgoing.entry(track) {
            std::collections::hash_map::Entry::Occupied(lane) => lane.into_mut(),
            std::collections::hash_map::Entry::Vacant(slot) => slot.insert(Outgoing {
                encoder: VoiceEncoder::new(profile)?,
                pending: Vec::with_capacity(FRAME * 2 * 2),
                ts: 0,
            }),
        };
        lane.pending.extend_from_slice(pcm);

        let step = FRAME * lane.encoder.profile().channels();
        // Число каналов музыки едет там же, где у картинки кодек: старые
        // версии настраивают по нему свой декодер.
        let codec = lane.encoder.profile().channels() as u8;
        let mut at = 0;
        let mut failure = None;
        while lane.pending.len() - at >= step {
            let frame = &lane.pending[at..at + step];
            at += step;
            lane.ts += 20_000;
            // Неудачный кадр выбрасываем вместе с хвостом до него: иначе
            // очередь росла бы с каждым вызовом и звук отставал всё сильнее.
            match lane.encoder.encode(frame) {
                Ok(packet) => send(track, codec, lane.ts - 20_000, &packet),
                Err(err) => failure = Some(err),
            }
        }
        lane.pending.drain(..at);
        failure.map_or(Ok(()), Err)
    }

    /// Вышли из звонка: забыть кодировщики и все входящие потоки.
    pub fn stop(&self) {
        self.shared.outgoing.lock().clear();
        self.shared.inbox.lock().clear();
        self.shared.reset.store(true, Ordering::Release);
    }

    /// Громкость собеседника у себя: 0 — не слышно, до 4 — усиление.
    pub fn set_volume(&self, author: Id, gain: f32) {
        let gain = if gain.is_finite() {
            gain.clamp(0.0, 4.0)
        } else {
            1.0
        };
        self.shared.gains.write().insert(author, gain);
    }

    /// Громкость общего плеера у себя.
    pub fn set_music_volume(&self, gain: f32) {
        let gain = if gain.is_finite() {
            gain.clamp(0.0, 2.0)
        } else {
            1.0
        };
        self.shared
            .music_gain
            .store(gain.to_bits(), Ordering::Relaxed);
    }
}

/// Входящий поток одного собеседника по одной дорожке.
struct Stream {
    jitter: Jitter<VoiceDecoder>,
    heard: Instant,
}

/// Часы сведения: раз в двадцать миллисекунд — кадр от каждого, сумма, в вебвью.
fn run_playout(shared: Arc<Shared>) {
    let mut streams: HashMap<(Id, Track), Stream> = HashMap::new();
    let mut speaking = Speaking::default();
    let mut deadline = Instant::now();

    loop {
        deadline += TICK;
        tick(&shared, &mut streams, &mut speaking);

        let now = Instant::now();
        if deadline > now {
            std::thread::sleep(deadline - now);
        } else if now - deadline > MAX_LAG {
            // Система спала или поток долго не получал процессор: догонять
            // пропущенное бессмысленно, начинаем такт заново.
            deadline = now;
        }
    }
}

fn tick(shared: &Shared, streams: &mut HashMap<(Id, Track), Stream>, speaking: &mut Speaking) {
    if shared.reset.swap(false, Ordering::AcqRel) {
        streams.clear();
        speaking.clear(&shared.notices);
    }

    let arrivals = std::mem::take(&mut *shared.inbox.lock());
    let now = Instant::now();
    for arrival in arrivals {
        let key = (arrival.author, arrival.track);
        let stream = match streams.entry(key) {
            std::collections::hash_map::Entry::Occupied(stream) => stream.into_mut(),
            std::collections::hash_map::Entry::Vacant(slot) => {
                let channels = if arrival.track == Track::Music { 2 } else { 1 };
                match VoiceDecoder::new(channels) {
                    Ok(decoder) => slot.insert(Stream {
                        jitter: Jitter::new(decoder),
                        heard: now,
                    }),
                    Err(err) => {
                        tracing::warn!(%err, "декодер звука не создан");
                        continue;
                    }
                }
            }
        };
        stream.heard = now;
        stream.jitter.push(arrival.seq, arrival.data, arrival.at_ms);
    }

    let gains = shared.gains.read().clone();
    let music_gain = f32::from_bits(shared.music_gain.load(Ordering::Relaxed));
    let mut mix = vec![0f32; FRAME * 2];
    let mut audible = false;

    for ((author, track), stream) in streams.iter_mut() {
        let Some(pcm) = stream.jitter.pull() else {
            continue;
        };
        audible = true;
        // Подсветка — по громкости до регулятора: она говорит о человеке, а не
        // о том, как громко его сделали у себя. Музыка сюда не попадает.
        if *track != Track::Music {
            speaking.hear(*author, rms(&pcm));
        }
        let gain = if *track == Track::Music {
            music_gain
        } else {
            gains.get(author).copied().unwrap_or(1.0)
        };
        if stream.jitter.channels() == 2 {
            for (out, sample) in mix.iter_mut().zip(pcm.iter()) {
                *out += sample * gain;
            }
        } else {
            for (frame, sample) in mix.as_chunks_mut::<2>().0.iter_mut().zip(pcm.iter()) {
                frame[0] += sample * gain;
                frame[1] += sample * gain;
            }
        }
    }

    streams.retain(|(author, track), stream| {
        let keep = stream.jitter.playing() || stream.heard.elapsed() < STREAM_IDLE;
        if !keep {
            let stats = stream.jitter.stats();
            tracing::debug!(
                author = %author.short(),
                track = track.name(),
                played = stats.played,
                recovered = stats.recovered,
                concealed = stats.concealed,
                late = stats.late,
                "звуковой поток закрыт"
            );
        }
        keep
    });

    speaking.step(&shared.notices);

    if !audible {
        return; // играть нечего — и слать нечего, вебвью сам доиграет тишину
    }
    let Some(sink) = shared.sink.read().clone() else {
        return;
    };
    // Полный канал до вебвью — значит, он не успевает. Кадр выбрасываем:
    // сыгранный с опозданием, он только растянул бы задержку.
    let _ = sink.try_send(encode_out(&mix));
}

/// Кадр для вебвью: заголовок и отсчёты int16, чередующиеся по каналам.
///
/// Сумма нескольких громких голосов легко вылезает за единицу. Обрезать её
/// жёстко — это треск, поэтому выше порога звук плавно поджимается.
fn encode_out(mix: &[f32]) -> Vec<u8> {
    let frames = mix.len() / 2;
    let mut out = Vec::with_capacity(OUT_HEADER + mix.len() * 2);
    out.push(OUT_VERSION);
    out.push(2);
    out.extend_from_slice(&(frames as u16).to_le_bytes());
    for sample in mix {
        let limited = soft_clip(*sample);
        out.extend_from_slice(&((limited * 32767.0) as i16).to_le_bytes());
    }
    out
}

fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let magnitude = x.abs();
    if magnitude <= KNEE {
        return x;
    }
    let over = (magnitude - KNEE) / (1.0 - KNEE);
    x.signum() * (KNEE + (1.0 - KNEE) * over.tanh())
}

/// Кто сейчас говорит — по громкости за последние сто миллисекунд.
#[derive(Default)]
struct Speaking {
    levels: HashMap<Id, f32>,
    ticks: usize,
    shown: Vec<Id>,
    sent: Option<Instant>,
}

impl Speaking {
    fn hear(&mut self, author: Id, level: f32) {
        let peak = self.levels.entry(author).or_insert(0.0);
        *peak = peak.max(level);
    }

    fn step(&mut self, notices: &UnboundedSender<Notice>) {
        self.ticks += 1;
        if self.ticks < LEVEL_WINDOW {
            return;
        }
        self.ticks = 0;
        let mut now: Vec<Id> = self
            .levels
            .drain()
            .filter(|(_, level)| *level > SPEAKING_RMS)
            .map(|(author, _)| author)
            .collect();
        now.sort_by_key(|author| author.0);

        let stale = self.sent.is_none_or(|at| at.elapsed() >= SPEAKING_REFRESH);
        if now != self.shown || (!now.is_empty() && stale) {
            self.sent = Some(Instant::now());
            self.shown = now.clone();
            let _ = notices.send(Notice::Speaking { authors: now });
        }
    }

    fn clear(&mut self, notices: &UnboundedSender<Notice>) {
        self.levels.clear();
        if !self.shown.is_empty() {
            self.shown.clear();
            let _ = notices.send(Notice::Speaking {
                authors: Vec::new(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> (AudioEngine, tokio::sync::mpsc::UnboundedReceiver<Notice>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (AudioEngine::new(tx), rx)
    }

    #[test]
    fn own_voice_is_cut_into_twenty_millisecond_packets() {
        let (engine, _rx) = engine();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let log = sent.clone();
        engine.set_send(Arc::new(move |track, _codec, ts, data: &[u8]| {
            log.lock().push((track, ts, data.len()));
        }));

        // Вебвью отдаёт блоки любого размера — кадры обязаны выйти ровными.
        let tone: Vec<f32> = (0..FRAME * 3 + 100)
            .map(|i| (i as f32 * 0.05).sin() * 0.2)
            .collect();
        for chunk in tone.chunks(700) {
            engine.send_pcm(Track::Audio, chunk).unwrap();
        }
        let sent = sent.lock();
        assert_eq!(
            sent.len(),
            3,
            "три полных кадра, хвост ждёт следующих отсчётов"
        );
        assert_eq!(sent[1].1 - sent[0].1, 20_000, "время кадра растёт на 20 мс");
        assert!(sent.iter().all(|(track, _, _)| *track == Track::Audio));
    }

    #[test]
    fn video_is_not_audio() {
        let (engine, _rx) = engine();
        engine.set_send(Arc::new(|_, _, _, _: &[u8]| {}));
        assert!(engine.send_pcm(Track::Video, &[0.0; FRAME]).is_err());
    }

    #[test]
    fn soft_clip_keeps_quiet_and_tames_loud() {
        assert_eq!(soft_clip(0.5), 0.5);
        assert!(soft_clip(3.0) <= 1.0);
        assert!(soft_clip(-3.0) >= -1.0);
        assert!(
            soft_clip(0.95) > 0.8,
            "выше порога звук поджат, но не обрезан"
        );
    }

    #[test]
    fn mixed_frame_has_header_and_both_channels() {
        let out = encode_out(&vec![0.25; FRAME * 2]);
        assert_eq!(out[0], OUT_VERSION);
        assert_eq!(out[1], 2);
        assert_eq!(u16::from_le_bytes([out[2], out[3]]) as usize, FRAME);
        assert_eq!(out.len(), OUT_HEADER + FRAME * 2 * 2);
    }

    #[test]
    fn speaking_is_reported_and_then_cleared() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut speaking = Speaking::default();
        let marina = Id([4u8; 32]);
        for _ in 0..LEVEL_WINDOW {
            speaking.hear(marina, 0.2);
            speaking.step(&tx);
        }
        match rx.try_recv() {
            Ok(Notice::Speaking { authors }) => assert_eq!(authors, vec![marina]),
            other => panic!("ждали подсветку, пришло {other:?}"),
        }
        for _ in 0..LEVEL_WINDOW {
            speaking.step(&tx);
        }
        match rx.try_recv() {
            Ok(Notice::Speaking { authors }) => assert!(authors.is_empty(), "замолчала — гасим"),
            other => panic!("ждали гашение, пришло {other:?}"),
        }
    }
}
