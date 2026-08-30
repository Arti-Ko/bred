//! Захват звука приложения через WASAPI и пульт через медиа-клавиши.
//!
//! На Windows звук чужого приложения снимается «петлёй по процессу»: WASAPI с
//! Windows 10 сборки 20348 умеет отдать поток конкретного процесса вместе с его
//! потомками. Это важнее, чем кажется: электронные приложения — а Яндекс Музыка
//! из них — играют не из главного процесса, а из процесса-помощника, и захват
//! ровно одного PID вернул бы тишину.
//!
//! Тем же механизмом снимается и «весь звук системы», только наоборот:
//! исключаем собственное дерево процессов. Иначе БРЕД услышал бы сам себя —
//! сначала эхо, потом эхо от эха.
//!
//! Управление — системные медиа-клавиши, как и на маке. Своего API у
//! музыкальных приложений нет, а эти клавиши слушает то из них, которое сейчас
//! играет с точки зрения системы.

use anyhow::{anyhow, Result};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use windows::{
    core::{implement, Interface, Ref, BOOL, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, LPARAM, MAX_PATH, WAIT_OBJECT_0},
        Media::Audio::{
            IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
            IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
            AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
            AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
            AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
            PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
            PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, WAVEFORMATEX,
        },
        System::{
            Com::StructuredStorage::{
                PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
            },
            Com::{CoInitializeEx, CoUninitialize, BLOB, COINIT_MULTITHREADED},
            Threading::{
                CreateEventW, GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW,
                SetEvent, WaitForMultipleObjects, INFINITE, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
            Variant::VT_BLOB,
        },
        UI::{
            Input::KeyboardAndMouse::{
                SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
                VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK,
            },
            WindowsAndMessaging::{
                EnumWindows, GetWindowTextLengthW, GetWindowThreadProcessId, IsWindowVisible,
            },
        },
    },
};

use super::{Command, Sink, Source, CHANNELS, RATE, WHOLE_SYSTEM};

/// Сколько ждём, пока система отдаст клиент захвата.
const ASK: Duration = Duration::from_secs(10);
/// Тише этого считаем тишиной: у цифрового нуля всё равно есть шумок.
const SILENCE: f32 = 0.0005;
/// Столько молчания — и считаем, что источник на паузе.
const QUIET_MS: u64 = 1_500;
/// Длина буфера захвата в сотнях наносекунд — двадцать миллисекунд.
const BUFFER_HNS: i64 = 200_000;
/// Отсчёты с плавающей точкой — `WAVE_FORMAT_IEEE_FLOAT` из `mmreg.h`.
///
/// Константа объявлена здесь, а не взята из привязок: ради одного числа тянуть
/// весь модуль мультимедиа не стоит.
const FORMAT_FLOAT: u16 = 3;

/// Строка устройства, за которой прячется петля по процессу.
const LOOPBACK_DEVICE: PCWSTR = windows::core::w!("VAD\\Process_Loopback");

/// Дескриптор ядра, который можно передать в рабочий поток.
///
/// `HANDLE` — это просто число, и события ядра для того и существуют, чтобы их
/// ждали из другого потока. В привязках он не помечен `Send` только потому, что
/// помечать так весь тип нельзя.
#[derive(Clone, Copy)]
struct Signal(HANDLE);

// SAFETY: см. комментарий к `Signal`.
unsafe impl Send for Signal {}
// SAFETY: см. комментарий к `Signal`.
unsafe impl Sync for Signal {}

/// Обработчик, которым система сообщает, что клиент готов.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Ready(Signal);

impl IActivateAudioInterfaceCompletionHandler_Impl for Ready_Impl {
    fn ActivateCompleted(
        &self,
        _operation: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        unsafe { SetEvent(self.0 .0) }
    }
}

/// Список приложений с видимым окном.
///
/// Именно окна, а не звуковые сессии: у приложения на паузе сессии может не
/// быть вовсе, и включить его звук заранее стало бы невозможно.
pub fn applications() -> Result<Vec<Source>> {
    let mut pids: Vec<u32> = Vec::new();
    // SAFETY: обратный вызов пишет в вектор, переданный через `LPARAM`, и живёт
    // ровно на время перечисления.
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut pids as *mut Vec<u32> as isize));
    }

    let mine = unsafe { GetCurrentProcessId() };
    let mut by_name: HashMap<String, String> = HashMap::new();
    for pid in pids {
        if pid == mine {
            continue;
        }
        let Some(exe) = executable(pid) else { continue };
        let name = exe.trim_end_matches(".exe").trim_end_matches(".EXE");
        if name.is_empty() {
            continue;
        }
        by_name.insert(exe.clone(), name.to_string());
    }

    let mut list: Vec<Source> = by_name
        .into_iter()
        .map(|(id, name)| Source { id, name })
        .collect();
    list.sort_by_key(|source| source.name.to_lowercase());
    Ok(list)
}

unsafe extern "system" fn collect(window: HWND, out: LPARAM) -> BOOL {
    // SAFETY: `out` — это указатель на вектор, который живёт всё перечисление.
    let list = unsafe { &mut *(out.0 as *mut Vec<u32>) };
    let visible = unsafe { IsWindowVisible(window) }.as_bool();
    let titled = unsafe { GetWindowTextLengthW(window) } > 0;
    if visible && titled {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
        if pid != 0 && !list.contains(&pid) {
            list.push(pid);
        }
    }
    // Перечисление продолжается, пока возвращаем «истину».
    BOOL(1)
}

/// Имя исполняемого файла процесса — без пути.
fn executable(pid: u32) -> Option<String> {
    // SAFETY: дескриптор закрывается сразу после запроса имени.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = [0u16; MAX_PATH as usize];
    let mut size = buffer.len() as u32;
    let got = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
    };
    let _ = unsafe { CloseHandle(handle) };
    got.ok()?;

    let path = String::from_utf16_lossy(&buffer[..size as usize]);
    path.rsplit(['\\', '/']).next().map(str::to_string)
}

pub fn control(command: Command) -> Result<()> {
    let key = match command {
        Command::Toggle => VK_MEDIA_PLAY_PAUSE,
        Command::Next => VK_MEDIA_NEXT_TRACK,
        Command::Previous => VK_MEDIA_PREV_TRACK,
    };

    let press = |up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    Default::default()
                },
                ..Default::default()
            },
        },
    };
    let events = [press(false), press(true)];

    // SAFETY: массив живёт до конца вызова, размер записи берём у самого типа.
    let sent = unsafe { SendInput(&events, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize == events.len() {
        Ok(())
    } else {
        Err(anyhow!("система не приняла нажатие медиа-клавиши"))
    }
}

/// Живой захват. Останавливается вместе с уничтожением.
pub struct Tap {
    stop: Signal,
    worker: Option<JoinHandle<()>>,
    sounded: Arc<AtomicU64>,
    started: Instant,
}

impl Tap {
    pub fn start(source: &str, sink: Sink) -> Result<Self> {
        let (target, include) = if source == WHOLE_SYSTEM {
            // «Всё, кроме нас самих»: та же петля, только вывернутая наизнанку.
            (unsafe { GetCurrentProcessId() }, false)
        } else {
            (find_process(source)?, true)
        };

        // SAFETY: событие закрывается в `Drop`.
        let stop = Signal(unsafe { CreateEventW(None, true, false, PCWSTR::null()) }?);
        let started = Instant::now();
        let sounded = Arc::new(AtomicU64::new(0));

        let (tx, rx) = mpsc::channel::<Result<(), String>>();
        let worker = {
            let sounded = sounded.clone();
            std::thread::Builder::new()
                .name("bred-player".into())
                .spawn(move || pump(target, include, stop, sink, sounded, started, tx))?
        };

        match rx.recv_timeout(ASK) {
            Ok(Ok(())) => Ok(Self {
                stop,
                worker: Some(worker),
                sounded,
                started,
            }),
            Ok(Err(why)) => Err(anyhow!("захват не начался: {why}")),
            Err(_) => Err(anyhow!("захват не ответил на запуск")),
        }
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
        // SAFETY: событие ещё живо — его закрывает только этот же метод, ниже.
        let _ = unsafe { SetEvent(self.stop.0) };
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let _ = unsafe { CloseHandle(self.stop.0) };
    }
}

/// Найти процесс приложения по имени исполняемого файла.
fn find_process(exe: &str) -> Result<u32> {
    let mut pids: Vec<u32> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut pids as *mut Vec<u32> as isize));
    }
    pids.into_iter()
        .find(|pid| executable(*pid).is_some_and(|name| name.eq_ignore_ascii_case(exe)))
        .ok_or_else(|| anyhow!("приложение больше не запущено"))
}

/// Рабочий поток захвата: живёт от запуска до остановки и владеет всем COM.
#[allow(clippy::too_many_arguments)]
fn pump(
    target: u32,
    include: bool,
    stop: Signal,
    sink: Sink,
    sounded: Arc<AtomicU64>,
    started: Instant,
    report: mpsc::Sender<Result<(), String>>,
) {
    // SAFETY: поток целиком принадлежит захвату, COM закрывается на выходе.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if com.is_err() {
        let _ = report.send(Err("не удалось поднять COM".into()));
        return;
    }

    match run(target, include, stop, &sink, &sounded, started, &report) {
        Ok(()) => {}
        Err(err) => {
            // Ошибку после успешного старта слать некому: канал уже закрыт, а
            // человек увидит, что звук просто прекратился.
            let _ = report.send(Err(err.to_string()));
        }
    }
    unsafe { CoUninitialize() };
}

fn run(
    target: u32,
    include: bool,
    stop: Signal,
    sink: &Sink,
    sounded: &Arc<AtomicU64>,
    started: Instant,
    report: &mpsc::Sender<Result<(), String>>,
) -> Result<()> {
    let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: target,
                ProcessLoopbackMode: if include {
                    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE
                } else {
                    PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE
                },
            },
        },
    };

    // Параметры едут в систему безымянным блоком байтов — так описан этот вызов.
    let activation = PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: std::mem::ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_BLOB,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: PROPVARIANT_0_0_0 {
                    blob: BLOB {
                        cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                        pBlobData: &mut params as *mut _ as *mut u8,
                    },
                },
            }),
        },
    };

    // SAFETY: события закрываются в конце функции, все пути ведут туда.
    let ready = Signal(unsafe { CreateEventW(None, false, false, PCWSTR::null()) }?);
    let filled = Signal(unsafe { CreateEventW(None, false, false, PCWSTR::null()) }?);
    let guard = Guard(vec![ready, filled]);

    let handler: IActivateAudioInterfaceCompletionHandler = Ready(ready).into();
    let operation = unsafe {
        windows::Win32::Media::Audio::ActivateAudioInterfaceAsync(
            LOOPBACK_DEVICE,
            &IAudioClient::IID,
            Some(&activation),
            &handler,
        )
    }
    .map_err(|err| {
        anyhow!(
            "система не дала петлю по процессу: {err} — нужна Windows 10 сборки 20348 или новее"
        )
    })?;

    if unsafe { WaitForMultipleObjects(&[ready.0], false, ASK.as_millis() as u32) } != WAIT_OBJECT_0
    {
        return Err(anyhow!("система не ответила на запрос захвата"));
    }

    let mut result = windows::core::HRESULT(0);
    let mut activated: Option<windows::core::IUnknown> = None;
    unsafe { operation.GetActivateResult(&mut result, &mut activated) }?;
    result.ok()?;
    let client: IAudioClient = activated
        .ok_or_else(|| anyhow!("система вернула пустой клиент захвата"))?
        .cast()?;

    // Формат задаём сами: у петли по процессу нет устройства, у которого можно
    // было бы спросить «как ты привык».
    let format = WAVEFORMATEX {
        wFormatTag: FORMAT_FLOAT,
        nChannels: CHANNELS,
        nSamplesPerSec: RATE,
        wBitsPerSample: 32,
        nBlockAlign: CHANNELS * 4,
        nAvgBytesPerSec: RATE * CHANNELS as u32 * 4,
        cbSize: 0,
    };
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            BUFFER_HNS,
            0,
            &format,
            None,
        )
    }?;
    unsafe { client.SetEventHandle(filled.0) }?;
    let capture: IAudioCaptureClient = unsafe { client.GetService() }?;
    unsafe { client.Start() }?;

    let _ = report.send(Ok(()));

    let channels = CHANNELS as usize;
    loop {
        // Ждём либо новый блок, либо просьбу остановиться.
        let which = unsafe { WaitForMultipleObjects(&[filled.0, stop.0], false, INFINITE) };
        if which != WAIT_OBJECT_0 {
            break; // остановка или сбой ожидания — в обоих случаях уходим
        }

        loop {
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            if unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }.is_err()
            {
                break;
            }
            if frames == 0 {
                let _ = unsafe { capture.ReleaseBuffer(0) };
                break;
            }

            let count = frames as usize * channels;
            // Тишину система вправе отдать неинициализированным буфером, и
            // читать его нельзя — но поток обязан идти дальше ровно, поэтому
            // на её месте отправляем настоящие нули.
            let samples: Vec<f32> = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                vec![0.0; count]
            } else {
                // SAFETY: система обещает `frames * nBlockAlign` байтов, а
                // формат мы задали сами — это `f32`.
                let slice = unsafe { std::slice::from_raw_parts(data as *const f32, count) };
                if slice.iter().any(|sample| sample.abs() > SILENCE) {
                    sounded.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                }
                slice.to_vec()
            };
            let _ = unsafe { capture.ReleaseBuffer(frames) };
            sink(&samples);
        }
    }

    let _ = unsafe { client.Stop() };
    drop(guard);
    Ok(())
}

/// Закрывает дескрипторы, каким бы путём функция ни завершилась.
struct Guard(Vec<Signal>);

impl Drop for Guard {
    fn drop(&mut self) {
        for signal in &self.0 {
            // SAFETY: каждый дескриптор создан здесь же и закрывается однажды.
            let _ = unsafe { CloseHandle(signal.0) };
        }
    }
}
