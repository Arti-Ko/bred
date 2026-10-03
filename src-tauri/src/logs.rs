//! Журнал последних минут работы — в памяти, для отчёта о проблеме.
//!
//! Журнал по-прежнему идёт в терминал, но заодно его хвост держится здесь:
//! человек, у которого «опять не соединяет», в терминал не смотрит, а к отчёту
//! журнал прикладывается одной галочкой. На диск ничего не пишется — после
//! перезапуска хвост начинается заново.

use parking_lot::Mutex;
use std::{collections::VecDeque, io::Write, sync::OnceLock};
use tracing_subscriber::fmt::MakeWriter;

/// Сколько строк помним. Строка журнала — сотня-другая байт, так что это
/// меньше полумегабайта даже на самых длинных.
const LINES: usize = 2000;

fn tail() -> &'static Mutex<VecDeque<String>> {
    static TAIL: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
    TAIL.get_or_init(|| Mutex::new(VecDeque::with_capacity(LINES)))
}

/// Хвост журнала одним текстом, от старого к новому.
pub fn snapshot() -> String {
    let lines = tail().lock();
    let mut out = String::with_capacity(lines.iter().map(|l| l.len() + 1).sum());
    for line in lines.iter() {
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn remember(raw: &[u8]) {
    let text = strip_ansi(&String::from_utf8_lossy(raw));
    let mut lines = tail().lock();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if lines.len() == LINES {
            lines.pop_front();
        }
        lines.push_back(line.to_string());
    }
}

/// Цвета терминала в отчёте — мусор вида `\x1b[2m`.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Писатель для `tracing`: строка уходит в терминал и в хвост.
#[derive(Clone, Copy, Default)]
pub struct Tee;

impl<'a> MakeWriter<'a> for Tee {
    type Writer = TeeLine;

    fn make_writer(&'a self) -> Self::Writer {
        TeeLine(Vec::with_capacity(256))
    }
}

/// Одна запись журнала: копится, а при завершении уходит в оба места.
pub struct TeeLine(Vec<u8>);

impl Write for TeeLine {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for TeeLine {
    fn drop(&mut self) {
        let _ = std::io::stdout().write_all(&self.0);
        remember(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_stripped() {
        assert_eq!(
            strip_ansi("\u{1b}[2m2026\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m звонок"),
            "2026  INFO звонок"
        );
    }

    #[test]
    fn tail_keeps_the_latest_lines() {
        for i in 0..(LINES + 5) {
            remember(format!("строка {i}\n").as_bytes());
        }
        let text = snapshot();
        assert!(text.ends_with(&format!("строка {}\n", LINES + 4)));
        assert!(!text.contains("строка 0\n"), "старое вытесняется");
    }
}
