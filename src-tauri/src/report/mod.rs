//! Сообщить о проблеме: из настроек — прямо в Telegram-чат разработчика.
//!
//! Сервера у БРЕДа нет и здесь: отчёт уходит в Bot API Telegram напрямую.
//! Ключ бота в публичный репозиторий не попадает — он подставляется при сборке
//! из секретов GitHub (`build.rs`) и лежит в бинаре под маской, а не открытым
//! текстом. Честно: маска останавливает `strings`, а не того, кто будет
//! разбирать приложение. Поэтому бот — отдельный, только для отчётов, с
//! единственным правом писать в один чат; утёкший ключ отзывается в
//! @BotFather одной командой.

pub mod format;
pub mod telegram;

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crate::domain::Id;
use format::{Card, Kind};
use telegram::{Bot, Media, MediaKind};

mod secrets {
    include!(concat!(env!("OUT_DIR"), "/report_secrets.rs"));
}

/// Не чаще одного отчёта за столько: кнопку жмут дважды, а чат — не мусорка.
const COOLDOWN: Duration = Duration::from_secs(30);
/// Вложений в одном отчёте.
pub const MAX_FILES: usize = 10;
/// Потолок Bot API на загрузку файла — пятьдесят мегабайт.
const MAX_FILE: u64 = 50 * 1024 * 1024;
/// Фото больше десяти мегабайт Telegram как фото не принимает — уйдёт документом.
const MAX_PHOTO: u64 = 10 * 1024 * 1024;
/// Сколько всего можно приложить: дальше отправка тянется минутами.
const MAX_TOTAL: u64 = 150 * 1024 * 1024;
const API: &str = "https://api.telegram.org";

/// Что заполнил человек.
#[derive(Debug, Clone, Deserialize)]
pub struct Draft {
    pub kind: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub files: Vec<PathBuf>,
    #[serde(default)]
    pub with_log: bool,
}

/// Что известно о приложении в момент отправки.
pub struct Context {
    pub nick: String,
    pub account: Id,
    pub device: Id,
    pub network: String,
    pub log: Option<String>,
    pub sent_at: String,
}

/// Вложение глазами интерфейса: что выбрано и как уйдёт.
#[derive(Debug, Clone, Serialize)]
pub struct FileInfo {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    /// `photo`, `video` или `document`.
    pub kind: &'static str,
    /// Почему не уйдёт. Пусто — уйдёт.
    pub problem: Option<String>,
}

/// Куда слать. Пусто — в этой сборке отчёты не настроены.
#[derive(Clone)]
struct Destination {
    api: String,
    token: String,
    chat: String,
}

fn destination() -> Option<Destination> {
    // Переменные окружения — для своей сборки и проверок: ключ не обязан
    // зашиваться, чтобы можно было попробовать отправку локально.
    let token = std::env::var("BRED_REPORT_TOKEN")
        .ok()
        .or_else(|| unmask(secrets::TOKEN))?;
    let chat = std::env::var("BRED_REPORT_CHAT")
        .ok()
        .or_else(|| unmask(secrets::CHAT))?;
    let api = std::env::var("BRED_REPORT_API").unwrap_or_else(|_| API.to_string());
    (!token.is_empty() && !chat.is_empty()).then_some(Destination { api, token, chat })
}

fn unmask(masked: &[u8]) -> Option<String> {
    if masked.is_empty() {
        return None;
    }
    let plain: Vec<u8> = masked
        .iter()
        .zip(secrets::MASK.iter().cycle())
        .map(|(byte, mask)| byte ^ mask)
        .collect();
    String::from_utf8(plain).ok()
}

/// Настроена ли отправка в этой сборке.
pub fn configured() -> bool {
    destination().is_some()
}

/// Как уйдёт файл — или почему не уйдёт.
pub fn inspect(path: &Path) -> FileInfo {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "файл".into());
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let kind = match ext.as_str() {
        "png" | "jpg" | "jpeg" | "webp" if size <= MAX_PHOTO => MediaKind::Photo,
        "mp4" | "mov" | "m4v" | "webm" => MediaKind::Video,
        _ => MediaKind::Document,
    };
    let problem = if size == 0 {
        Some("файл пустой или недоступен".to_string())
    } else if size > MAX_FILE {
        Some(format!(
            "больше {} МБ — Telegram такой не примет",
            MAX_FILE / 1024 / 1024
        ))
    } else {
        None
    };
    FileInfo {
        path: path.to_path_buf(),
        name,
        size,
        kind: match kind {
            MediaKind::Photo => "photo",
            MediaKind::Video => "video",
            MediaKind::Document => "document",
        },
        problem,
    }
}

fn mime_of(path: &Path) -> &'static str {
    match path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("heic") => "image/heic",
        Some("mp4" | "m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        _ => "application/octet-stream",
    }
}

pub struct Reporter {
    last: Mutex<Option<Instant>>,
}

impl Default for Reporter {
    fn default() -> Self {
        Self::new()
    }
}

impl Reporter {
    pub fn new() -> Self {
        Self {
            last: Mutex::new(None),
        }
    }

    /// Проверить черновик до отправки: ошибки — человеческими словами.
    fn validate(draft: &Draft) -> Result<Kind> {
        let kind = Kind::find(&draft.kind).ok_or_else(|| anyhow!("выберите, что не так"))?;
        let title = draft.title.trim().chars().count();
        if title == 0 {
            return Err(anyhow!("нужен заголовок — пара слов о проблеме"));
        }
        if title > 120 {
            return Err(anyhow!("заголовок длиннее 120 знаков — сократите"));
        }
        if draft.body.trim().is_empty() {
            return Err(anyhow!("опишите, что случилось и что вы делали перед этим"));
        }
        if draft.files.len() > MAX_FILES {
            return Err(anyhow!("не больше {MAX_FILES} вложений"));
        }
        let mut total = 0;
        for path in &draft.files {
            let info = inspect(path);
            if let Some(problem) = info.problem {
                return Err(anyhow!("«{}»: {problem}", info.name));
            }
            total += info.size;
        }
        if total > MAX_TOTAL {
            return Err(anyhow!(
                "вложения вместе больше {} МБ — уберите часть",
                MAX_TOTAL / 1024 / 1024
            ));
        }
        Ok(kind)
    }

    /// Отправить отчёт. Возвращает его номер — по нему отчёт ищется в чате.
    pub async fn send(&self, draft: Draft, context: Context) -> Result<String> {
        let target =
            destination().ok_or_else(|| anyhow!("в этой сборке отправка отчётов не настроена"))?;
        let kind = Self::validate(&draft)?;
        {
            let mut last = self.last.lock();
            if let Some(at) = *last {
                let left = COOLDOWN.saturating_sub(at.elapsed());
                if !left.is_zero() {
                    return Err(anyhow!(
                        "предыдущий отчёт ушёл только что — подождите {} с",
                        left.as_secs().max(1)
                    ));
                }
            }
            *last = Some(Instant::now());
        }

        let result = send_to(&target, kind, &draft, &context).await;
        if result.is_err() {
            // Не дошло — повтор не должен упираться в ожидание.
            *self.last.lock() = None;
        }
        result
    }
}

async fn send_to(
    target: &Destination,
    kind: Kind,
    draft: &Draft,
    context: &Context,
) -> Result<String> {
    let id = format::report_id(rand::random::<u64>());
    let version = env!("CARGO_PKG_VERSION");
    let os = format!("{} {}", os_label(), std::env::consts::ARCH);
    let with_log = draft.with_log && context.log.as_deref().is_some_and(|l| !l.is_empty());
    let card = Card {
        id: &id,
        kind,
        title: &draft.title,
        body: &draft.body,
        nick: &context.nick,
        account: context.account,
        device: context.device,
        version,
        os: &os,
        network: &context.network,
        attachments: draft.files.len(),
        with_log,
        sent_at: &context.sent_at,
    };

    let bot = Bot::new(&target.api, &target.token, &target.chat)?;
    let message = bot.send_message(&format::compose(&card)).await?;

    // Файлы читаются только сейчас: человек мог выбрать видео на полсотни
    // мегабайт, и держать его в памяти с момента выбора незачем.
    if !draft.files.is_empty() {
        let mut media = Vec::with_capacity(draft.files.len());
        for path in &draft.files {
            let info = inspect(path);
            let bytes = tokio::fs::read(path)
                .await
                .map_err(|err| anyhow!("«{}» не читается: {err}", info.name))?;
            media.push(Media {
                kind: match info.kind {
                    "photo" => MediaKind::Photo,
                    "video" => MediaKind::Video,
                    _ => MediaKind::Document,
                },
                name: info.name,
                mime: mime_of(path).to_string(),
                bytes,
            });
        }
        bot.send_media(&media, &format!("📎 <code>{id}</code>"), message)
            .await?;
    }

    if with_log {
        if let Some(log) = &context.log {
            bot.send_text_file(
                &format!("журнал-{id}.txt"),
                log,
                &format!("🧾 журнал · <code>{id}</code>"),
                message,
            )
            .await?;
        }
    }

    tracing::info!(report = %id, "отчёт о проблеме отправлен");
    Ok(id)
}

fn os_label() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macOS",
        "windows" => "Windows",
        "ios" => "iOS",
        "android" => "Android",
        _ => "Linux",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> Draft {
        Draft {
            kind: "audio".into(),
            title: "Хрипит звук".into(),
            body: "Минут через десять разговора".into(),
            files: Vec::new(),
            with_log: true,
        }
    }

    #[test]
    fn empty_fields_are_explained() {
        let mut no_title = draft();
        no_title.title = "  ".into();
        assert!(Reporter::validate(&no_title).is_err());

        let mut no_body = draft();
        no_body.body = String::new();
        assert!(Reporter::validate(&no_body).is_err());

        let mut unknown = draft();
        unknown.kind = "что-то".into();
        assert!(Reporter::validate(&unknown).is_err());

        assert!(Reporter::validate(&draft()).is_ok());
    }

    #[test]
    fn files_are_sorted_by_how_telegram_shows_them() {
        let dir = std::env::temp_dir().join(format!("bred-report-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shot = dir.join("снимок.png");
        std::fs::write(&shot, [1u8; 100]).unwrap();
        let clip = dir.join("запись.mov");
        std::fs::write(&clip, [1u8; 100]).unwrap();
        let gif = dir.join("смешно.gif");
        std::fs::write(&gif, [1u8; 100]).unwrap();

        assert_eq!(inspect(&shot).kind, "photo");
        assert_eq!(inspect(&clip).kind, "video");
        assert_eq!(
            inspect(&gif).kind,
            "document",
            "GIF в альбом с фото не встанет"
        );
        assert!(inspect(&dir.join("нет такого.png")).problem.is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn masked_secret_round_trips() {
        let plain = b"123:secret";
        let masked: Vec<u8> = plain
            .iter()
            .zip(secrets::MASK.iter().cycle())
            .map(|(b, m)| b ^ m)
            .collect();
        assert_eq!(unmask(&masked).as_deref(), Some("123:secret"));
        assert_eq!(unmask(&[]), None, "не задано при сборке — не настроено");
    }
}
