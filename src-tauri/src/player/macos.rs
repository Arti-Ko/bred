//! Захват звука приложения через ScreenCaptureKit и пульт через медиа-клавиши.
//!
//! ScreenCaptureKit — единственный публичный способ снять звук чужого
//! приложения без установки виртуального аудиоустройства. Картинка при этом не
//! нужна, но поток без неё не собирается, поэтому просим кадр два на два
//! пикселя раз в секунду: дешевле, чем кажется, и честнее, чем не работать.
//!
//! Управление — системные медиа-клавиши. Своего API у музыкальных приложений
//! нет, а эти клавиши слушает то из них, которое сейчас «играет» с точки зрения
//! системы. Заодно поэтому пульт и не врёт: состояние «играет/пауза» мы не
//! предполагаем по нажатиям, а слышим по самому потоку.

use anyhow::{anyhow, Result};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::{define_class, rc::Retained, runtime::ProtocolObject, AllocAnyThread, DefinedClass};
use objc2_core_audio_types::AudioBufferList;
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeFlags};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCRunningApplication, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamOutput, SCStreamOutputType,
};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};

use super::{Command, Sink, Source, CHANNELS, RATE, WHOLE_SYSTEM};

/// Сколько ждём ответа системы. Первый вызов может упереться в диалог разрешения,
/// и человек отвечает на него не мгновенно.
const ASK: Duration = Duration::from_secs(30);
/// Тише этого считаем тишиной: у цифрового нуля всё равно есть шумок.
const SILENCE: f32 = 0.0005;
/// Столько молчания — и считаем, что источник на паузе.
const QUIET_MS: u64 = 1_500;

/// Коды медиа-клавиш из IOKit (`NX_KEYTYPE_*`).
const KEY_PLAY: isize = 16;
const KEY_NEXT: isize = 17;
const KEY_PREV: isize = 18;

struct Ivars {
    sink: Sink,
    /// Когда в последний раз пришёл не-тихий блок, миллисекунды от старта.
    sounded: Arc<AtomicU64>,
    started: Instant,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "BredAudioTap"]
    #[ivars = Ivars]
    struct Output;

    unsafe impl NSObjectProtocol for Output {}

    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn on_sample(
            &self,
            _stream: &SCStream,
            buffer: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind != SCStreamOutputType::Audio {
                return;
            }
            let samples = unsafe { interleaved(buffer) };
            if samples.is_empty() {
                return;
            }
            let ivars = self.ivars();
            if samples.iter().any(|s| s.abs() > SILENCE) {
                let ms = ivars.started.elapsed().as_millis() as u64;
                ivars.sounded.store(ms, Ordering::Relaxed);
            }
            (ivars.sink)(&samples);
        }
    }
);

/// Достать из буфера ScreenCaptureKit чередующиеся отсчёты.
///
/// Система отдаёт каналы порознь, по плоскости на каждый, а кодировщику в
/// вебвью нужны они вперемежку — там это один вызов вместо ручной сборки.
unsafe fn interleaved(buffer: &CMSampleBuffer) -> Vec<f32> {
    let mut needed: usize = 0;
    unsafe {
        buffer.audio_buffer_list_with_retained_block_buffer(
            &mut needed,
            std::ptr::null_mut(),
            0,
            None,
            None,
            0,
            std::ptr::null_mut(),
        );
    }
    if needed == 0 {
        return Vec::new();
    }

    let mut storage = vec![0u8; needed];
    let list = storage.as_mut_ptr() as *mut AudioBufferList;
    let mut block: *mut objc2_core_media::CMBlockBuffer = std::ptr::null_mut();
    let status = unsafe {
        buffer.audio_buffer_list_with_retained_block_buffer(
            std::ptr::null_mut(),
            list,
            needed,
            None,
            None,
            0,
            &mut block,
        )
    };
    // Блок с байтами система удерживает за нас — отпускаем, как только скопируем.
    let _block = std::ptr::NonNull::new(block)
        .map(|block| unsafe { objc2_core_foundation::CFRetained::from_raw(block) });
    if status != 0 {
        return Vec::new();
    }

    let list = unsafe { &*list };
    let planes =
        unsafe { std::slice::from_raw_parts(list.mBuffers.as_ptr(), list.mNumberBuffers as usize) };
    let planes: Vec<&[f32]> = planes
        .iter()
        .filter(|plane| !plane.mData.is_null())
        .map(|plane| unsafe {
            std::slice::from_raw_parts(
                plane.mData as *const f32,
                plane.mDataByteSize as usize / std::mem::size_of::<f32>(),
            )
        })
        .collect();

    let Some(frames) = planes.iter().map(|plane| plane.len()).min() else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(frames * planes.len());
    for frame in 0..frames {
        for plane in &planes {
            out.push(plane[frame]);
        }
    }
    out
}

/// Спросить систему, что вообще доступно для захвата.
fn shareable() -> Result<Retained<SCShareableContent>> {
    let (tx, rx) = mpsc::channel();
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let got = if content.is_null() {
                Err(unsafe { error.as_ref() }
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or_else(|| "система не объяснила отказ".to_string()))
            } else {
                unsafe { Retained::retain(content) }.ok_or_else(|| "пустой ответ".to_string())
            };
            let _ = tx.send(got);
        },
    );
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };

    match rx.recv_timeout(ASK) {
        Ok(Ok(content)) => Ok(content),
        Ok(Err(why)) => Err(anyhow!("ScreenCaptureKit отказал: {why}")),
        Err(_) => Err(anyhow!(
            "система не ответила — похоже, БРЕДу не разрешена запись экрана"
        )),
    }
}

pub fn applications() -> Result<Vec<Source>> {
    let content = shareable()?;
    let mut seen = std::collections::HashSet::new();
    let mut list = Vec::new();
    for app in unsafe { content.applications() }.iter() {
        let id = unsafe { app.bundleIdentifier() }.to_string();
        let name = unsafe { app.applicationName() }.to_string();
        // Служебные процессы системы человеку не нужны, а список раздувают втрое.
        if id.is_empty() || name.is_empty() || id.starts_with("com.apple.") {
            continue;
        }
        if seen.insert(id.clone()) {
            list.push(Source { id, name });
        }
    }
    list.sort_by_key(|source| source.name.to_lowercase());
    Ok(list)
}

pub fn control(command: Command) -> Result<()> {
    let key = match command {
        Command::Toggle => KEY_PLAY,
        Command::Next => KEY_NEXT,
        Command::Previous => KEY_PREV,
    };
    if media_key(key) {
        Ok(())
    } else {
        Err(anyhow!("система не приняла нажатие медиа-клавиши"))
    }
}

/// Нажать и отпустить системную медиа-клавишу.
///
/// Событие уходит на самый низкий уровень ввода, откуда его получает то
/// приложение, которое сейчас числится играющим. Своего API ни у одного
/// музыкального приложения для этого нет.
fn media_key(key: isize) -> bool {
    use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
    use objc2_core_graphics::{CGEvent, CGEventTapLocation};
    use objc2_foundation::NSPoint;

    for down in [true, false] {
        let state: isize = if down { 0xA } else { 0xB };
        let event =
            NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
                NSEventType::SystemDefined,
                NSPoint::ZERO,
                NSEventModifierFlags((state as usize) << 8),
                0.0,
                0,
                None,
                8,
                (key << 16) | (state << 8),
                -1,
            );
        let Some(event) = event else { return false };
        let Some(raw) = event.CGEvent() else {
            return false;
        };
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&raw));
    }
    true
}

/// Живой захват. Останавливается вместе с уничтожением.
pub struct Tap {
    held: Held,
    sounded: Arc<AtomicU64>,
    started: Instant,
}

/// Держатель объектов Objective-C.
///
/// В биндингах `SCStream` не помечен `Send` — по умолчанию так помечать нельзя
/// ни один класс. Но Apple обещает, что запуск и остановка потока зовутся из
/// любого потока, а подсчёт ссылок в Objective-C и так атомарный. Нам этого
/// достаточно: между потоками мы только держим объект и один раз просим его
/// остановиться.
struct Held {
    stream: Retained<SCStream>,
    _output: Retained<Output>,
    _queue: dispatch2::DispatchRetained<DispatchQueue>,
}

// SAFETY: см. комментарий к `Held`.
unsafe impl Send for Held {}
// SAFETY: см. комментарий к `Held`.
unsafe impl Sync for Held {}

impl Tap {
    pub fn start(source: &str, sink: Sink) -> Result<Self> {
        let content = shareable()?;
        let displays = unsafe { content.displays() };
        let display = displays
            .iter()
            .next()
            .ok_or_else(|| anyhow!("система не показала ни одного экрана"))?;

        let filter = if source == WHOLE_SYSTEM {
            unsafe {
                SCContentFilter::initWithDisplay_excludingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &NSArray::new(),
                )
            }
        } else {
            // Все процессы этого бандла, а не один: у электронных приложений
            // звук играет помощник, а не то окно, которое видно человеку.
            let apps: Vec<Retained<SCRunningApplication>> = unsafe { content.applications() }
                .iter()
                .filter(|app| unsafe { app.bundleIdentifier() }.to_string() == source)
                .collect();
            if apps.is_empty() {
                return Err(anyhow!("приложение больше не запущено"));
            }
            unsafe {
                SCContentFilter::initWithDisplay_includingApplications_exceptingWindows(
                    SCContentFilter::alloc(),
                    &display,
                    &NSArray::from_retained_slice(&apps),
                    &NSArray::new(),
                )
            }
        };

        let config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            config.setCapturesAudio(true);
            // Иначе собственный звук БРЕДа уедет обратно в БРЕД: сначала эхо,
            // потом эхо от эха.
            config.setExcludesCurrentProcessAudio(true);
            config.setSampleRate(RATE as isize);
            config.setChannelCount(CHANNELS as isize);
            // Картинка не нужна, но без неё поток не собрать. Просим минимум.
            config.setWidth(2);
            config.setHeight(2);
            config.setMinimumFrameInterval(CMTime {
                value: 1,
                timescale: 1,
                flags: CMTimeFlags(1),
                epoch: 0,
            });
        }

        let started = Instant::now();
        let sounded = Arc::new(AtomicU64::new(0));
        let output = Output::alloc().set_ivars(Ivars {
            sink,
            sounded: sounded.clone(),
            started,
        });
        let output: Retained<Output> = unsafe { objc2::msg_send![super(output), init] };

        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                None,
            )
        };
        let queue = DispatchQueue::new("dev.bred.player", None);
        unsafe {
            stream
                .addStreamOutput_type_sampleHandlerQueue_error(
                    ProtocolObject::from_ref(&*output),
                    SCStreamOutputType::Audio,
                    Some(&queue),
                )
                .map_err(|err| anyhow!("не удалось подписаться на звук: {err}"))?;
        }

        let (tx, rx) = mpsc::channel();
        let done = RcBlock::new(move |error: *mut NSError| {
            let _ =
                tx.send(unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string()));
        });
        unsafe { stream.startCaptureWithCompletionHandler(Some(&done)) };
        match rx.recv_timeout(ASK) {
            Ok(None) => {}
            Ok(Some(why)) => return Err(anyhow!("захват не начался: {why}")),
            Err(_) => return Err(anyhow!("захват не ответил на запуск")),
        }

        Ok(Self {
            held: Held {
                stream,
                _output: output,
                _queue: queue,
            },
            sounded,
            started,
        })
    }

    /// Звучит ли источник прямо сейчас.
    ///
    /// Это и есть состояние «играет», и берётся оно из самого звука, а не из
    /// того, что мы когда-то нажали. Иначе плеер врал бы каждый раз, когда
    /// музыку остановили мимо него — из самой Яндекс Музыки, например.
    pub fn sounding(&self) -> bool {
        let now = self.started.elapsed().as_millis() as u64;
        now.saturating_sub(self.sounded.load(Ordering::Relaxed)) < QUIET_MS
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        unsafe { self.held.stream.stopCaptureWithCompletionHandler(None) };
    }
}
