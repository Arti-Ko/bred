//! Как отчёт выглядит в Telegram.
//!
//! Сообщение собирается в HTML-разметке Bot API: жирный заголовок, текст
//! человека цитатой, служебное — моноширинным, а в конце хэштеги. Хэштеги —
//! главное для поиска: по `#звук` находятся все жалобы на звук, по `#u48291305`
//! — всё от одного человека, по `#R_K7Q2XM` — конкретный отчёт.

use crate::domain::Id;

/// Виды проблем: код для интерфейса, подпись для людей, хэштег для поиска.
pub const KINDS: &[Kind] = &[
    Kind::new("audio", "Звук", "звук", "🎧"),
    Kind::new("video", "Видео и демонстрация", "видео", "🖥"),
    Kind::new("network", "Связь и подключение", "связь", "📡"),
    Kind::new("messages", "Сообщения и история", "сообщения", "💬"),
    Kind::new("files", "Файлы, стикеры, эмодзи", "файлы", "📎"),
    Kind::new("ui", "Интерфейс", "интерфейс", "🪟"),
    Kind::new("games", "Игры", "игры", "🎮"),
    Kind::new("crash", "Вылет или зависание", "вылет", "💥"),
    Kind::new("idea", "Предложение", "предложение", "💡"),
    Kind::new("other", "Другое", "другое", "❔"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Kind {
    pub code: &'static str,
    pub label: &'static str,
    #[serde(skip)]
    pub tag: &'static str,
    #[serde(skip)]
    pub icon: &'static str,
}

impl Kind {
    const fn new(
        code: &'static str,
        label: &'static str,
        tag: &'static str,
        icon: &'static str,
    ) -> Self {
        Self {
            code,
            label,
            tag,
            icon,
        }
    }

    pub fn find(code: &str) -> Option<Kind> {
        KINDS.iter().copied().find(|kind| kind.code == code)
    }

    /// Предложение — не баг: у него свой первый хэштег.
    fn family(self) -> &'static str {
        if self.code == "idea" {
            "идея"
        } else {
            "баг"
        }
    }
}

/// Потолок сообщения Bot API — 4096 знаков после разметки. Текст человека
/// урезается так, чтобы с шапкой и хвостом сообщение туда помещалось.
pub const MESSAGE_LIMIT: usize = 4096;
const BODY_BUDGET: usize = 2800;
/// Потолок подписи к вложениям — 1024 знака после разметки. С запасом: Telegram
/// считает в единицах UTF-16, а мы меряем так же, но лучше не впритык.
pub const CAPTION_LIMIT: usize = 1000;
const CLIPPED_HERE: &str = "[текст урезан]";
const CLIPPED_TO_FILE: &str = "[целиком — в файле отчёта]";

/// Номер пользователя: восемь цифр из ключа аккаунта, вида `4829-1305`.
///
/// Ключ аккаунта — пятьдесят два знака, продиктовать его голосом нельзя. Номер
/// человек видит в настройках и может назвать, а в чате отчётов по нему ищутся
/// все его обращения. Совпадение двух номеров возможно, но на круг друзей —
/// один шанс на десятки миллионов.
pub fn user_number(account: Id) -> String {
    let hash = blake3::hash(&account.0);
    let value = u64::from_le_bytes(hash.as_bytes()[..8].try_into().expect("8 байт"));
    let digits = format!("{:08}", value % 100_000_000);
    format!("{}-{}", &digits[..4], &digits[4..])
}

/// Номер отчёта: `R-` и шесть знаков без похожих друг на друга букв.
pub fn report_id(seed: u64) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut value = seed;
    let mut out = String::from("R-");
    for _ in 0..6 {
        out.push(ALPHABET[(value % ALPHABET.len() as u64) as usize] as char);
        value /= ALPHABET.len() as u64;
    }
    out
}

/// Всё, что попадёт в сообщение.
pub struct Card<'a> {
    pub id: &'a str,
    pub kind: Kind,
    pub title: &'a str,
    pub body: &'a str,
    pub nick: &'a str,
    pub account: Id,
    pub device: Id,
    pub version: &'a str,
    pub os: &'a str,
    pub network: &'a str,
    pub attachments: usize,
    pub with_log: bool,
    /// Время отправки по часам человека, уже строкой: часовой пояс знает только он.
    pub sent_at: &'a str,
}

/// Хэштеги отчёта. Telegram понимает в них буквы, цифры и подчёркивание —
/// поэтому точки версии и дефис номера заменены.
pub fn tags(card: &Card) -> String {
    [
        format!("#{}", card.kind.family()),
        format!("#{}", card.kind.tag),
        format!(
            "#{}",
            tag_safe(card.os.split_whitespace().next().unwrap_or("os"))
        ),
        format!("#v{}", tag_safe(card.version)),
        format!("#u{}", user_number(card.account).replace('-', "")),
        format!("#{}", card.id.replace('-', "_")),
    ]
    .join(" ")
}

fn tag_safe(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Экранирование для HTML-разметки Bot API: ровно три знака, больше он не просит.
pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Урезать текст по знакам, а не по байтам: кириллицу пополам не режем.
fn clip(text: &str, limit: usize, note: &str) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push('…');
    if !note.is_empty() {
        out.push('\n');
        out.push_str(note);
    }
    out
}

/// Сколько знаков сообщения увидит Telegram: без разметки, сущности
/// раскрыты, счёт в единицах UTF-16 — так он меряет свои пределы.
pub fn visible_len(html: &str) -> usize {
    let mut plain = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => plain.push(c),
            _ => {}
        }
    }
    plain
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .encode_utf16()
        .count()
}

/// Сообщение целиком — когда вложений нет и карточка идёт текстом.
pub fn compose(card: &Card) -> String {
    compose_with(card, BODY_BUDGET, CLIPPED_HERE)
}

/// Карточка подписью к вложениям: всё одним сообщением.
///
/// Подпись втрое короче сообщения. Если текст человека в неё не влезает, он
/// урезается, а целиком уходит файлом отчёта в том же сообщении — второй флаг.
pub fn caption(card: &Card) -> (String, bool) {
    let full = compose_with(card, usize::MAX, CLIPPED_TO_FILE);
    if visible_len(&full) <= CAPTION_LIMIT {
        return (full, false);
    }
    let body = card.body.trim().chars().count();
    let mut budget = body;
    while budget > 0 {
        let overflow =
            visible_len(&compose_with(card, budget, CLIPPED_TO_FILE)).saturating_sub(CAPTION_LIMIT);
        if overflow == 0 {
            break;
        }
        budget = budget.saturating_sub(overflow.max(16));
    }
    (compose_with(card, budget, CLIPPED_TO_FILE), true)
}

/// Файл отчёта: полный текст, если в подпись он не влез, и журнал.
/// `None` — ни того, ни другого не нужно.
pub fn text_file(card: &Card, clipped: bool, log: Option<&str>) -> Option<(String, String)> {
    match (clipped, log) {
        (false, None) => None,
        (false, Some(log)) => Some((format!("журнал-{}.txt", card.id), log.to_string())),
        (true, log) => {
            let mut text = format!(
                "Отчёт {id} · {kind} · {title}\n\n{body}\n",
                id = card.id,
                kind = card.kind.label,
                title = card.title.trim(),
                body = card.body.trim(),
            );
            if let Some(log) = log {
                text.push_str("\n──────── журнал ────────\n");
                text.push_str(log);
            }
            Some((format!("отчёт-{}.txt", card.id), text))
        }
    }
}

fn compose_with(card: &Card, budget: usize, note: &str) -> String {
    let body = escape(&clip(card.body.trim(), budget, note));
    // Длинный текст — сворачиваемой цитатой: в ленте чата видно начало, а не
    // экран сплошного текста.
    let quote = if card.body.chars().count() > 500 {
        "<blockquote expandable>"
    } else {
        "<blockquote>"
    };
    let extras = match (card.attachments, card.with_log) {
        (0, false) => String::from("без вложений"),
        (0, true) => String::from("журнал приложен"),
        (n, false) => format!("вложений: {n}"),
        (n, true) => format!("вложений: {n} · журнал приложен"),
    };

    let text = format!(
        "{icon} <b>{kind}</b> · <b>{title}</b>\n\
         {quote}{body}</blockquote>\n\
         👤 <b>{nick}</b> · № <code>{number}</code>\n\
         🔑 <code>{account}</code>\n\
         💻 {os} · БРЕД {version} · устройство <code>{device}</code>\n\
         🌐 {network}\n\
         📎 {extras}\n\
         🕐 {sent_at} · <code>{id}</code>\n\
         \n\
         {tags}",
        icon = card.kind.icon,
        kind = escape(card.kind.label),
        title = escape(&clip(card.title.trim(), 120, "")),
        nick = escape(card.nick),
        number = user_number(card.account),
        account = card.account,
        os = escape(card.os),
        version = escape(card.version),
        device = card.device.short(),
        network = escape(card.network),
        sent_at = escape(card.sent_at),
        id = card.id,
        tags = tags(card),
    );
    // Страховка от совсем экзотического случая: всё равно не длиннее предела.
    if text.chars().count() > MESSAGE_LIMIT {
        return text.chars().take(MESSAGE_LIMIT).collect();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card<'a>(body: &'a str) -> Card<'a> {
        Card {
            id: "R-K7Q2XM",
            kind: Kind::find("audio").unwrap(),
            title: "Пропадает <звук> & хрипит",
            body,
            nick: "марина",
            account: Id([3u8; 32]),
            device: Id([4u8; 32]),
            version: "0.7.0",
            os: "macOS aarch64",
            network: "на связи · соседей 3",
            attachments: 2,
            with_log: true,
            sent_at: "03.10.2026 10:42",
        }
    }

    #[test]
    fn user_text_cannot_break_the_markup() {
        let text = compose(&card("<b>не жирный</b> & <script>"));
        assert!(text.contains("&lt;b&gt;не жирный&lt;/b&gt; &amp; &lt;script&gt;"));
        assert!(text.contains("Пропадает &lt;звук&gt; &amp; хрипит"));
    }

    #[test]
    fn tags_are_searchable() {
        let tags = tags(&card("текст"));
        assert!(tags.starts_with("#баг #звук #macos #v0_7_0 #u"));
        assert!(tags.ends_with("#R_K7Q2XM"));
        assert!(
            tags.split(' ')
                .all(|tag| tag[1..].chars().all(|c| c.is_alphanumeric() || c == '_')),
            "в хэштегах только то, что понимает Telegram: {tags}"
        );
    }

    #[test]
    fn idea_is_not_a_bug() {
        let mut idea = card("сделайте тёмную тему");
        idea.kind = Kind::find("idea").unwrap();
        assert!(tags(&idea).starts_with("#идея #предложение"));
    }

    #[test]
    fn long_report_fits_into_one_message() {
        let essay = "очень длинно ".repeat(2000);
        let text = compose(&card(&essay));
        assert!(text.chars().count() <= MESSAGE_LIMIT);
        assert!(text.contains("[текст урезан]"));
        assert!(
            text.contains("<blockquote expandable>"),
            "длинное сворачивается"
        );
        assert!(
            text.contains("#R_K7Q2XM"),
            "хэштеги при урезании не теряются"
        );
    }

    #[test]
    fn short_report_fits_a_caption_whole() {
        let (text, clipped) = caption(&card("пропадает звук через десять минут"));
        assert!(!clipped);
        assert!(visible_len(&text) <= CAPTION_LIMIT);
        assert!(text.contains("пропадает звук"));
        assert!(text.contains("#R_K7Q2XM"));
    }

    #[test]
    fn long_report_is_clipped_in_caption_and_kept_in_file() {
        let essay = "длинное описание ".repeat(200);
        let card = card(&essay);
        let (text, clipped) = caption(&card);
        assert!(clipped);
        assert!(
            visible_len(&text) <= CAPTION_LIMIT,
            "{}",
            visible_len(&text)
        );
        assert!(text.contains("[целиком — в файле отчёта]"));
        assert!(
            text.contains("#R_K7Q2XM"),
            "хэштеги при урезании не теряются"
        );
        let (name, file) = text_file(&card, clipped, Some("строка журнала")).unwrap();
        assert_eq!(name, "отчёт-R-K7Q2XM.txt");
        assert!(file.contains(essay.trim()));
        assert!(file.contains("строка журнала"));
    }

    #[test]
    fn visible_length_counts_like_telegram() {
        assert_eq!(visible_len("<b>a&amp;b</b>"), 3);
        assert_eq!(visible_len("🎧"), 2, "эмодзи — две единицы UTF-16");
    }

    #[test]
    fn user_number_is_stable_and_readable() {
        let number = user_number(Id([3u8; 32]));
        assert_eq!(
            number,
            user_number(Id([3u8; 32])),
            "один и тот же у одного аккаунта"
        );
        assert_ne!(number, user_number(Id([5u8; 32])));
        assert_eq!(number.len(), 9);
        assert!(number.chars().all(|c| c.is_ascii_digit() || c == '-'));
    }

    #[test]
    fn report_ids_avoid_lookalike_letters() {
        for seed in [0u64, 1, 42, u64::MAX, 987_654_321] {
            let id = report_id(seed);
            assert_eq!(id.len(), 8);
            assert!(!id[2..].contains(['0', 'O', '1', 'I']), "{id}");
        }
        assert_ne!(report_id(1), report_id(2));
    }
}
