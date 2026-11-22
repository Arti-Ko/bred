//! Качество связи в звонке: кто до кого как дотягивается.
//!
//! Каждый участник раз в секунду меряет свои соединения — задержку, потери,
//! напрямую или через ретранслятор — и кладёт итог в пинг живости, который и
//! так уходит всем раз в секунду. Поэтому у каждого есть не только своя
//! сторона, но и чужие: «у Синки 400 мс до всех» и «у Синки 400 мс до меня»
//! — разные беды, и окно «Связь» в интерфейсе их различает.
//!
//! Пинг старых версий пустой, а прежняя просьба о ключевом кадре начинается
//! с другого байта — новый формат ни с чем не путается и старым не мешает.

use serde::Serialize;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use crate::domain::Id;

/// Метка отчёта о связи в пинге.
pub const PING_LINKS: u8 = 2;
/// Чужой отчёт старше этого — уже не правда: человек, видимо, отвалился.
const REPORT_FRESH: Duration = Duration::from_secs(5);
/// Больше собеседников в одном пинге не описываем: датаграмма не резиновая.
const MAX_LINKS: usize = 24;
/// Размер записи о собеседнике в пинге: ключ, задержка, флаги, потери.
const ENTRY: usize = 32 + 2 + 1 + 1;

/// Связь с одним собеседником.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LinkStat {
    pub peer: Id,
    /// Время туда и обратно, мс. Ноль — ещё не измерено.
    pub rtt_ms: u32,
    /// Через ретранслятор, а не напрямую.
    pub relay: bool,
    /// Потери пакетов за последний замер, проценты.
    pub loss: u8,
}

/// Чужой отчёт: как связь видит другой участник.
#[derive(Debug, Clone, Serialize)]
pub struct PeerReport {
    pub from: Id,
    pub age_ms: u64,
    pub links: Vec<LinkStat>,
}

/// Всё, что окно «Связь» показывает.
#[derive(Debug, Clone, Serialize)]
pub struct LinkReport {
    pub mine: Vec<LinkStat>,
    pub others: Vec<PeerReport>,
}

/// Сырые счётчики пути на момент замера.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    pub peer: Id,
    pub rtt: Duration,
    pub relay: bool,
    pub lost: u64,
    pub sent: u64,
}

/// Книга замеров: свои по последнему разу и чужие из пингов.
#[derive(Default)]
pub struct LinkBook {
    /// Счётчики прошлого замера: потери считаем приростом, а не за всё время.
    counters: HashMap<Id, (u64, u64)>,
    mine: Vec<LinkStat>,
    heard: HashMap<Id, (Instant, Vec<LinkStat>)>,
}

impl LinkBook {
    /// Записать свежий замер своих соединений и вернуть его.
    pub fn measure(&mut self, samples: &[Sample]) -> &[LinkStat] {
        self.counters
            .retain(|peer, _| samples.iter().any(|s| s.peer == *peer));
        self.mine = samples
            .iter()
            .map(|s| {
                let (lost_before, sent_before) = self
                    .counters
                    .insert(s.peer, (s.lost, s.sent))
                    .unwrap_or((s.lost, s.sent));
                // Путь сменился — счётчики начались заново; прирост тогда ноль.
                let lost = s.lost.saturating_sub(lost_before);
                let sent = s.sent.saturating_sub(sent_before);
                LinkStat {
                    peer: s.peer,
                    rtt_ms: u32::try_from(s.rtt.as_millis()).unwrap_or(u32::MAX),
                    relay: s.relay,
                    loss: loss_percent(lost, sent),
                }
            })
            .collect();
        &self.mine
    }

    /// Запомнить чужой отчёт.
    pub fn hear(&mut self, from: Id, links: Vec<LinkStat>) {
        self.heard.insert(from, (Instant::now(), links));
    }

    /// Звонок кончился — всё забыть.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn report(&self) -> LinkReport {
        let others = self
            .heard
            .iter()
            .filter(|(_, (at, _))| at.elapsed() < REPORT_FRESH)
            .map(|(from, (at, links))| PeerReport {
                from: *from,
                age_ms: u64::try_from(at.elapsed().as_millis()).unwrap_or(u64::MAX),
                links: links.clone(),
            })
            .collect();
        LinkReport {
            mine: self.mine.clone(),
            others,
        }
    }
}

fn loss_percent(lost: u64, sent: u64) -> u8 {
    if sent == 0 {
        return 0;
    }
    u8::try_from((lost.min(sent) * 100).div_ceil(sent)).unwrap_or(100)
}

/// Отчёт для пинга: метка, число записей и записи подряд.
pub fn encode(links: &[LinkStat]) -> Vec<u8> {
    let links = &links[..links.len().min(MAX_LINKS)];
    let mut out = Vec::with_capacity(2 + links.len() * ENTRY);
    out.push(PING_LINKS);
    out.push(links.len() as u8); // не больше MAX_LINKS — влезает
    for link in links {
        out.extend_from_slice(&link.peer.0);
        out.extend_from_slice(&u16::try_from(link.rtt_ms).unwrap_or(u16::MAX).to_be_bytes());
        out.push(u8::from(link.relay));
        out.push(link.loss.min(100));
    }
    out
}

/// Разобрать отчёт из пинга. Не отчёт или битый — `None`.
pub fn decode(ping: &[u8]) -> Option<Vec<LinkStat>> {
    let (&mark, rest) = ping.split_first()?;
    if mark != PING_LINKS {
        return None;
    }
    let (&count, body) = rest.split_first()?;
    let count = usize::from(count);
    if count > MAX_LINKS || body.len() != count * ENTRY {
        return None;
    }
    body.as_chunks::<ENTRY>()
        .0
        .iter()
        .map(|entry| {
            let peer = Id(entry[..32].try_into().ok()?);
            Some(LinkStat {
                peer,
                rtt_ms: u32::from(u16::from_be_bytes([entry[32], entry[33]])),
                relay: entry[34] & 1 == 1,
                loss: entry[35].min(100),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(byte: u8, rtt_ms: u32, relay: bool, loss: u8) -> LinkStat {
        LinkStat {
            peer: Id([byte; 32]),
            rtt_ms,
            relay,
            loss,
        }
    }

    #[test]
    fn report_survives_the_wire() {
        let links = vec![stat(1, 42, false, 0), stat(2, 380, true, 7)];
        assert_eq!(decode(&encode(&links)), Some(links));
    }

    #[test]
    fn huge_rtt_is_capped_not_wrapped() {
        let decoded = decode(&encode(&[stat(1, 90_000, false, 0)])).unwrap();
        assert_eq!(decoded[0].rtt_ms, u32::from(u16::MAX));
    }

    #[test]
    fn other_pings_are_not_reports() {
        assert_eq!(decode(&[]), None);
        assert_eq!(decode(&[1, 2, 3]), None); // просьба о ключевом кадре
        assert_eq!(decode(&[PING_LINKS, 1, 0, 0]), None); // обрезанный
    }

    #[test]
    fn loss_is_counted_since_last_measure() {
        let mut book = LinkBook::default();
        let sample = |lost, sent| Sample {
            peer: Id([9; 32]),
            rtt: Duration::from_millis(50),
            relay: false,
            lost,
            sent,
        };
        // Первый замер — без прошлого, прироста нет.
        assert_eq!(book.measure(&[sample(500, 10_000)])[0].loss, 0);
        // Потом 10 потерь на 100 отправленных — 10 %.
        assert_eq!(book.measure(&[sample(510, 10_100)])[0].loss, 10);
        // Путь сменился, счётчики обнулились — не отрицательные проценты.
        assert_eq!(book.measure(&[sample(0, 50)])[0].loss, 0);
    }

    #[test]
    fn stale_reports_are_dropped() {
        let mut book = LinkBook::default();
        book.hear(Id([3; 32]), vec![stat(4, 30, false, 0)]);
        assert_eq!(book.report().others.len(), 1);
        book.heard.get_mut(&Id([3; 32])).unwrap().0 -= REPORT_FRESH;
        assert!(book.report().others.is_empty());
    }
}
