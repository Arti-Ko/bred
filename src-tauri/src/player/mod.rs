//! Общий плеер комнаты: звук чужого приложения — сразу у всех.
//!
//! Синхронизировать здесь нечего, и это главное упрощение. Звук идёт живым
//! потоком от ведущего, а «пауза у всех» — не общий таймлайн, а пульт: нажал
//! любой участник → команда уехала ведущему → ведущий нажал паузу в своей
//! Яндекс Музыке → замолчало сразу у всех, включая её саму. Общие таймлайны
//! разъезжаются на любой потере пакета; пульт разъехаться не может.
//!
//! Захват нативный, и иначе быть не могло: вебвью на маке системный звук не
//! отдаёт вовсе — `getDisplayMedia({audio: true})` возвращает одну картинку.
//! Кодирование при этом осталось в вебвью, как и у микрофона: сюда приходят
//! только отсчёты, а в Opus их упаковывает тот же путь, что и голос.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as sys;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as sys;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod stub;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use stub as sys;

/// Частота и раскладка того, что отдаёт захват. Совпадает с тем, что ждёт
/// кодировщик в вебвью: пересчёт частоты по дороге никому не нужен.
pub const RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;

/// Особый источник: всё, что звучит в системе, кроме нас самих.
///
/// Нужен не для полноты картины, а по делу. Электронные приложения — а Яндекс
/// Музыка из них — играют звук не из главного процесса, а из процесса-помощника,
/// и захват «по приложению» у них может оказаться тишиной. Системный звук
/// целиком работает всегда, а эхо от самих себя отсекается на уровне ОС: на
/// маке — отдельным флагом, на Windows — той же петлёй по процессу, только
/// вывернутой наизнанку.
pub const WHOLE_SYSTEM: &str = "*";

/// Приложение, чей звук можно подключить.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    /// Идентификатор бандла, либо [`WHOLE_SYSTEM`].
    pub id: String,
    pub name: String,
}

/// Пульт. Больше команд не нужно: перемотка требует знать длину трека, а её
/// система без приватных API не отдаёт.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Command {
    /// Пауза, а из паузы — снова играть.
    Toggle,
    Next,
    Previous,
}

/// Куда уходят отсчёты: чередующиеся, стерео, 48 кГц.
pub type Sink = Arc<dyn Fn(&[f32]) + Send + Sync>;

/// Что можно подключить прямо сейчас.
pub fn sources() -> Result<Vec<Source>> {
    let mut list = vec![Source {
        id: WHOLE_SYSTEM.to_string(),
        name: "весь звук системы".to_string(),
    }];
    list.extend(sys::applications()?);
    Ok(list)
}

/// Нажать кнопку на пульте источника.
pub fn control(command: Command) -> Result<()> {
    sys::control(command)
}

pub use sys::Tap;
