//! Отправка в Telegram через Bot API — напрямую, без своего сервера.
//!
//! Карточка отчёта уходит первым сообщением, вложения — ответом на неё:
//! фото и видео альбомами, всё прочее документами, журнал — файлом. В чате
//! это одна ветка, и по хэштегу из карточки находится всё сразу.

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

    /// Карточка отчёта. Возвращает номер сообщения — вложения уходят ответом на него.
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

    /// Вложения ответом на карточку: фото и видео — альбомами, остальное —
    /// альбомами документов. В одном альбоме Telegram не смешивает документы
    /// с фото, поэтому их два вида.
    pub async fn send_media(&self, media: &[Media], caption: &str, reply_to: i64) -> Result<()> {
        let (visual, documents): (Vec<&Media>, Vec<&Media>) = media
            .iter()
            .partition(|item| item.kind != MediaKind::Document);
        let mut first = true;
        for group in visual.chunks(ALBUM).chain(documents.chunks(ALBUM)) {
            // Подпись — только у первого вложения: в альбоме она одна на всех.
            let caption = if first { caption } else { "" };
            first = false;
            let sent = self.send_group(group, caption, reply_to).await;
            if sent.is_ok() || group.iter().all(|item| item.kind == MediaKind::Document) {
                sent?;
                continue;
            }
            // Telegram не смог обработать фото или видео — необычный размер,
            // редкий кодек, повреждённый файл. Не повод терять вложение и весь
            // отчёт: те же файлы уходят документами, как есть.
            tracing::debug!(error = %sent.unwrap_err(), "вложение не принято как фото, шлём файлом");
            let as_files: Vec<Media> = group
                .iter()
                .map(|item| Media {
                    kind: MediaKind::Document,
                    name: item.name.clone(),
                    mime: item.mime.clone(),
                    bytes: item.bytes.clone(),
                })
                .collect();
            let refs: Vec<&Media> = as_files.iter().collect();
            self.send_group(&refs, caption, reply_to).await?;
        }
        Ok(())
    }

    async fn send_group(&self, group: &[&Media], caption: &str, reply_to: i64) -> Result<()> {
        if group.len() == 1 {
            self.send_single(group[0], caption, reply_to).await
        } else {
            self.send_album(group, caption, reply_to).await
        }
    }

    /// Журнал — текстовым файлом ответом на карточку.
    pub async fn send_text_file(
        &self,
        name: &str,
        text: &str,
        caption: &str,
        reply_to: i64,
    ) -> Result<()> {
        let file = Media {
            kind: MediaKind::Document,
            name: name.to_string(),
            mime: "text/plain".to_string(),
            bytes: text.as_bytes().to_vec(),
        };
        self.send_single(&file, caption, reply_to).await
    }

    async fn send_single(&self, item: &Media, caption: &str, reply_to: i64) -> Result<()> {
        self.call(item.kind.method(), || {
            let mut form = self
                .reply_form(reply_to)
                .part(item.kind.field(), file_part(item));
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

    async fn send_album(&self, group: &[&Media], caption: &str, reply_to: i64) -> Result<()> {
        let described: Vec<serde_json::Value> = group
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let mut entry = serde_json::json!({
                    "type": item.kind.field(),
                    "media": format!("attach://file{index}"),
                });
                if index == 0 && !caption.is_empty() {
                    entry["caption"] = caption.into();
                    entry["parse_mode"] = "HTML".into();
                }
                entry
            })
            .collect();
        self.call("sendMediaGroup", || {
            let mut form = self.reply_form(reply_to).text(
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

    fn reply_form(&self, reply_to: i64) -> Form {
        Form::new().text("chat_id", self.chat.clone()).text(
            "reply_parameters",
            format!(r#"{{"message_id":{reply_to},"allow_sending_without_reply":true}}"#),
        )
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
