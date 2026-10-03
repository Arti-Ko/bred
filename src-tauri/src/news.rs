//! Канал «Обновления БРЕД»: что нового в каждой версии — у всех и сразу.
//!
//! Писать в канал может только разработчик, и пишет он не в БРЕДе, а в
//! релизе на GitHub: заголовок релиза становится заголовком поста, описание —
//! текстом. Поэтому прав внутри приложения здесь не нужно вовсе: подделать пост
//! можно, только выпустив подписанный релиз.
//!
//! Пост появляется не в момент тега, а когда обновление и правда можно
//! поставить: у релиза есть `latest.json`, который конвейер выкладывает только
//! после того, как собрались все платформы. Иначе люди читали бы про версию,
//! которую ещё нельзя получить.
//!
//! Сервера у этого канала тоже нет: лента — это страница релизов GitHub, туда
//! же ходит встроенное обновление.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

use crate::{net::Notice, store::Store};

/// Чьи релизы читаем. Переопределяется для проверок.
const RELEASES: &str = "https://api.github.com/repos/Arti-Ko/bred/releases?per_page=30";
/// Как часто заглядываем. GitHub без ключа даёт шестьдесят запросов в час на
/// адрес — раз в двадцать минут далеко от предела.
const POLL: Duration = Duration::from_secs(20 * 60);
/// Первый взгляд после запуска — не сразу: сначала поднимается сеть.
const FIRST_LOOK: Duration = Duration::from_secs(8);
const POSTS_SETTING: &str = "news.posts";
const READ_SETTING: &str = "news.read";

/// Пост канала — один выпуск.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Post {
    pub version: String,
    /// «Обновление звонков 0.7» — то, что разработчик написал в заголовке.
    pub title: String,
    /// Описание в Markdown, с эмодзи и списками.
    pub body: String,
    /// Когда вышел, миллисекунды.
    pub published: i64,
}

/// Состояние канала для интерфейса.
#[derive(Debug, Clone, Serialize)]
pub struct Feed {
    /// Новые сверху.
    pub posts: Vec<Post>,
    /// До какой версии включительно человек уже прочитал.
    pub read: Option<String>,
    /// Установленная версия — чтобы показать «это у вас уже есть».
    pub current: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    draft: bool,
    prerelease: bool,
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
}

/// Сравнение версий вида `0.7.1`: по числам, а не по строкам — иначе `0.10`
/// оказалась бы старше `0.9`.
pub fn newer(a: &str, b: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.trim_start_matches('v')
            .split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    parse(a) > parse(b)
}

/// Релизы GitHub — в посты. Черновики, предварительные и ещё не собранные до
/// конца (без `latest.json`) — не посты.
fn posts_from(releases: Vec<Release>) -> Vec<Post> {
    let mut posts: Vec<Post> = releases
        .into_iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter(|r| r.assets.iter().any(|a| a.name == "latest.json"))
        .map(|r| {
            let version = r.tag_name.trim_start_matches('v').to_string();
            let title = r
                .name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| format!("БРЕД {version}"));
            Post {
                title,
                body: r.body.unwrap_or_default().trim().to_string(),
                published: r
                    .published_at
                    .as_deref()
                    .and_then(parse_time)
                    .unwrap_or_default(),
                version,
            }
        })
        .collect();
    posts.sort_by(|a, b| {
        if newer(&a.version, &b.version) {
            std::cmp::Ordering::Less
        } else if newer(&b.version, &a.version) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    posts
}

/// `2026-10-03T07:15:00Z` → миллисекунды. Без часовых поясов: GitHub пишет UTC.
fn parse_time(text: &str) -> Option<i64> {
    let (date, time) = text.trim_end_matches('Z').split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let mut t = time.split(':').map(|p| p.parse::<f64>());
    let (h, min, s) = (t.next()?.ok()?, t.next()?.ok()?, t.next()?.ok()?);
    // Дни от эпохи — по алгоритму Говарда Хиннанта, без зависимостей.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400) as f64 * 1000.0 + (h * 3600.0 + min * 60.0 + s) * 1000.0) as i64)
}

pub struct News {
    store: Arc<Store>,
    notices: tokio::sync::mpsc::UnboundedSender<Notice>,
}

impl News {
    pub fn new(
        store: Arc<Store>,
        notices: tokio::sync::mpsc::UnboundedSender<Notice>,
    ) -> Arc<Self> {
        Arc::new(Self { store, notices })
    }

    /// Что сейчас в канале — из того, что уже скачано.
    pub fn feed(&self) -> Feed {
        Feed {
            posts: self.cached(),
            read: self
                .store
                .get_setting(READ_SETTING)
                .ok()
                .flatten()
                .map(|raw| String::from_utf8_lossy(&raw).to_string()),
            current: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Человек открыл канал — всё, что там есть, прочитано.
    pub fn mark_read(&self) -> Result<()> {
        if let Some(newest) = self.cached().first() {
            self.store
                .set_setting(READ_SETTING, newest.version.as_bytes())?;
        }
        Ok(())
    }

    fn cached(&self) -> Vec<Post> {
        self.store
            .get_setting(POSTS_SETTING)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default()
    }

    /// Заглядывать в релизы, пока приложение открыто.
    pub fn watch(self: &Arc<Self>) {
        let news = self.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(FIRST_LOOK).await;
            loop {
                if let Err(err) = news.refresh().await {
                    tracing::debug!(%err, "лента обновлений не обновилась");
                }
                tokio::time::sleep(POLL).await;
            }
        });
    }

    /// Скачать релизы и объявить новые посты.
    pub async fn refresh(&self) -> Result<()> {
        let url = std::env::var("BRED_NEWS_API").unwrap_or_else(|_| RELEASES.to_string());
        let client = crate::http::client(Duration::from_secs(20))?;
        let response = client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|err| anyhow!("GitHub недоступен: {err}"))?;
        if !response.status().is_success() {
            return Err(anyhow!("GitHub ответил {}", response.status()));
        }
        let releases: Vec<Release> = response.json().await?;
        self.take(posts_from(releases))
    }

    /// Разобрать свежую ленту: сохранить и сказать о новом.
    fn take(&self, posts: Vec<Post>) -> Result<()> {
        let before = self.cached();
        if posts.is_empty() || posts == before {
            return Ok(());
        }
        self.store
            .set_setting(POSTS_SETTING, &serde_json::to_vec(&posts)?)?;

        let current = env!("CARGO_PKG_VERSION");
        let first_time = before.is_empty();
        for post in posts.iter().rev() {
            let known = before.iter().any(|old| old.version == post.version);
            if known {
                continue;
            }
            // При первом заходе в канал прошлые выпуски — история, а не
            // новость: звенеть десятью уведомлениями разом незачем. Новость —
            // только то, что новее установленной версии, то есть то, что
            // можно поставить прямо сейчас.
            if first_time && !newer(&post.version, current) {
                continue;
            }
            let _ = self.notices.send(Notice::News {
                version: post.version.clone(),
                title: post.title.clone(),
            });
        }
        let _ = self.notices.send(Notice::NewsFeed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, name: &str, built: bool) -> Release {
        Release {
            tag_name: tag.into(),
            name: Some(name.into()),
            body: Some("🎧 звук лучше".into()),
            draft: false,
            prerelease: false,
            published_at: Some("2026-10-03T07:15:00Z".into()),
            assets: if built {
                vec![Asset {
                    name: "latest.json".into(),
                }]
            } else {
                Vec::new()
            },
        }
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(newer("0.10.0", "0.9.9"));
        assert!(newer("v0.7.1", "0.7.0"));
        assert!(!newer("0.7.0", "0.7.0"));
        assert!(!newer("0.6.9", "0.7.0"));
    }

    #[test]
    fn only_finished_releases_become_posts() {
        let posts = posts_from(vec![
            release("v0.7.0", "Обновление звонков 0.7", true),
            release("v0.7.1", "Ещё собирается", false),
            release("v0.6.9", "", true),
        ]);
        let versions: Vec<_> = posts.iter().map(|p| p.version.as_str()).collect();
        assert_eq!(
            versions,
            vec!["0.7.0", "0.6.9"],
            "новые сверху, недособранных нет"
        );
        assert_eq!(posts[0].title, "Обновление звонков 0.7");
        assert_eq!(
            posts[1].title, "БРЕД 0.6.9",
            "без заголовка — по номеру версии"
        );
    }

    #[test]
    fn github_time_is_parsed() {
        // 2026-10-03T07:15:00Z
        assert_eq!(parse_time("2026-10-03T07:15:00Z"), Some(1_791_011_700_000));
        assert_eq!(parse_time("ерунда"), None);
    }

    fn news() -> (Arc<News>, tokio::sync::mpsc::UnboundedReceiver<Notice>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        (News::new(Arc::new(Store::in_memory().unwrap()), tx), rx)
    }

    fn post(version: &str) -> Post {
        Post {
            version: version.into(),
            title: format!("Выпуск {version}"),
            body: String::new(),
            published: 0,
        }
    }

    fn announced(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Notice>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(notice) = rx.try_recv() {
            if let Notice::News { version, .. } = notice {
                out.push(version);
            }
        }
        out
    }

    #[test]
    fn first_look_announces_only_what_can_be_installed() {
        let (news, mut rx) = news();
        news.take(vec![post("99.0.0"), post("0.1.0")]).unwrap();
        assert_eq!(announced(&mut rx), vec!["99.0.0"], "история — не новость");
    }

    #[test]
    fn new_release_is_announced_once() {
        let (news, mut rx) = news();
        news.take(vec![post("0.1.0")]).unwrap();
        announced(&mut rx);
        news.take(vec![post("0.2.0"), post("0.1.0")]).unwrap();
        assert_eq!(announced(&mut rx), vec!["0.2.0"]);
        news.take(vec![post("0.2.0"), post("0.1.0")]).unwrap();
        assert!(announced(&mut rx).is_empty(), "второй раз — не новость");
    }

    #[test]
    fn reading_marks_the_newest() {
        let (news, _rx) = news();
        news.take(vec![post("0.2.0"), post("0.1.0")]).unwrap();
        assert_eq!(news.feed().read, None);
        news.mark_read().unwrap();
        assert_eq!(news.feed().read.as_deref(), Some("0.2.0"));
    }
}
