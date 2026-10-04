//! Отправка в Telegram через Bot API — напрямую, без своего сервера.
//!
//! Отчёт — одно сообщение. Без вложений это карточка текстом. С вложениями —
//! одна группа, а карточка — её подпись: фото и видео альбомом, если ничего
//! другого нет, иначе всё файлами (Telegram не кладёт фото и документы в одну
//! группу). Журнал и полный текст длинного описания — файлом в той же группе.

use anyhow::{anyhow, Result};
use reqwest::multipart::{Form, Part};
use std::time::Duration;

/// Сколько ждём один запрос. Видео на десятки мегабайт через медленный канал
/// едет долго — обрывать его на полпути значит не отправить вовсе.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
/// В альбоме Telegram не больше десяти элементов.
const ALBUM: usize = 10;

/// Как Telegram покажет вложение.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Photo,
    Video,
    /// Всё, что не фото и не видео для Telegram: GIF, HEIC, крупные картинки.
    Document,
}

impl MediaKind {
    fn field(self) -> &'static str {
        match self {
            MediaKind::Photo => "photo",
            MediaKind::Video => "video",
            MediaKind::Document => "document",
        }
    }

    fn method(self) -> &'static str {
        match self {
            MediaKind::Photo => "sendPhoto",
            MediaKind::Video => "sendVideo",
            MediaKind::Document => "sendDocument",
        }
    }
}

/// Где в группе подпись: у альбома фото Telegram показывает подпись первого,
/// у группы файлов — каждую под своим файлом.
#[derive(Clone, Copy)]
enum Caption {
    First,
    Last,
}

pub struct Media {
    pub kind: MediaKind,
    pub name: String,
    pub mime: String,
    pub bytes: Vec<u8>,
}

pub struct Bot {
    client: reqwest::Client,
    /// `https://api.telegram.org/bot<ключ>` — или подменённый адрес в тестах.
    base: String,
    chat: String,
}

impl Bot {
    pub fn new(api: &str, token: &str, chat: &str) -> Result<Self> {
        let client = crate::http::client(REQUEST_TIMEOUT)?;
        Ok(Self {
            client,
            base: format!("{}/bot{}", api.trim_end_matches('/'), token),
            chat: chat.to_string(),
        })
    }

    /// Карточка отчёта текстом — когда вложений нет.
    pub async fn send_message(&self, html: &str) -> Result<i64> {
        let reply = self
            .call("sendMessage", || {
                Form::new()
                    .text("chat_id", self.chat.clone())
                    .text("text", html.to_string())
                    .text("parse_mode", "HTML")
                    .text("link_preview_options", r#"{"is_disabled":true}"#)
            })
            .await?;
        reply["message_id"]
            .as_i64()
            .ok_or_else(|| anyhow!("Telegram не вернул номер сообщения"))
    }

    /// Всё одним сообщением: вложения группой, карточка — подписью.
    ///
    /// Фото и видео без прочего — альбомом с картинками в ленте. Если среди
    /// вложений есть файлы или журнал, — всё файлами: Telegram не смешивает в
    /// одной группе фото с документами, а две группы — уже два сообщения.
    /// Картинки-файлы он показывает миниатюрами, так что видно и их.
    pub async fn send_bundle(&self, media: Vec<Media>, caption: &str) -> Result<()> {
        if media.len() > ALBUM {
            return Err(anyhow!(
                "в одно сообщение Telegram помещается не больше {ALBUM} вложений"
            ));
        }
        if media.iter().all(|item| item.kind != MediaKind::Document) {
            let refs: Vec<&Media> = media.iter().collect();
            match self.send_group(&refs, caption, Caption::First).await {
                Ok(()) => return Ok(()),
                // Telegram не смог обработать фото или видео — необычный
                // размер, редкий кодек, повреждённый файл. Не повод терять
                // отчёт: те же файлы уходят документами, как есть.
                Err(err) => tracing::debug!(%err, "вложения не приняты как фото, шлём файлами"),
            }
        }
        let files: Vec<Media> = media
            .into_iter()
            .map(|item| Media {
                kind: MediaKind::Document,
                ..item
            })
            .collect();
        let refs: Vec<&Media> = files.iter().collect();
        // У группы файлов подпись видна под каждым, у кого она есть, — ставим
        // её последнему, и карточка оказывается внизу, как текст сообщения.
        self.send_group(&refs, caption, Caption::Last).await
    }

    async fn send_group(&self, group: &[&Media], caption: &str, at: Caption) -> Result<()> {
        if group.len() == 1 {
            self.send_single(group[0], caption).await
        } else {
            let index = match at {
                Caption::First => 0,
                Caption::Last => group.len() - 1,
            };
            self.send_album(group, caption, index).await
        }
    }

    async fn send_single(&self, item: &Media, caption: &str) -> Result<()> {
        self.call(item.kind.method(), || {
            let mut form = self.base_form().part(item.kind.field(), file_part(item));
            if !caption.is_empty() {
                form = form
                    .text("caption", caption.to_string())
                    .text("parse_mode", "HTML");
            }
            if item.kind == MediaKind::Video {
                form = form.text("supports_streaming", "true");
            }
            form
        })
        .await
        .map(|_| ())
    }

    async fn send_album(&self, group: &[&Media], caption: &str, at: usize) -> Result<()> {
        let described: Vec<serde_json::Value> = group
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let mut entry = serde_json::json!({
                    "type": item.kind.field(),
                    "media": format!("attach://file{index}"),
                });
                if index == at && !caption.is_empty() {
                    entry["caption"] = caption.into();
                    entry["parse_mode"] = "HTML".into();
                }
                entry
            })
            .collect();
        self.call("sendMediaGroup", || {
            let mut form = self.base_form().text(
                "media",
                serde_json::Value::Array(described.clone()).to_string(),
            );
            for (index, item) in group.iter().enumerate() {
                form = form.part(format!("file{index}"), file_part(item));
            }
            form
        })
        .await
        .map(|_| ())
    }

    fn base_form(&self) -> Form {
        Form::new().text("chat_id", self.chat.clone())
    }

    /// Вызов метода Bot API. Форма собирается заново на каждую попытку: тело
    /// запроса одноразовое. Попросили подождать (429) — ждём и пробуем ещё раз.
    async fn call(&self, method: &str, form: impl Fn() -> Form) -> Result<serde_json::Value> {
        for attempt in 0..2 {
            let response = self
                .client
                .post(format!("{}/{method}", self.base))
                .multipart(form())
                .send()
                .await
                .map_err(|err| anyhow!("Telegram недоступен: {}", err.without_url()))?;
            let reply: serde_json::Value = response
                .json()
                .await
                .map_err(|err| anyhow!("Telegram ответил непонятно: {}", err.without_url()))?;
            if reply["ok"].as_bool() == Some(true) {
                return Ok(reply["result"].clone());
            }
            let wait = reply["parameters"]["retry_after"].as_u64();
            if let (Some(seconds), 0) = (wait, attempt) {
                tokio::time::sleep(Duration::from_secs(seconds.min(30))).await;
                continue;
            }
            let why = reply["description"].as_str().unwrap_or("без объяснений");
            return Err(anyhow!("Telegram отказал: {why}"));
        }
        Err(anyhow!("Telegram просит подождать дольше обычного"))
    }
}

fn file_part(item: &Media) -> Part {
    let part = Part::bytes(item.bytes.clone()).file_name(item.name.clone());
    part.mime_str(&item.mime)
        .unwrap_or_else(|_| Part::bytes(item.bytes.clone()).file_name(item.name.clone()))
}
