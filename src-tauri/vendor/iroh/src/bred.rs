//! bred: сколько раз ретранслятор не ответил на пинг.
//!
//! По этому счётчику БРЕД понимает, что соединения с ретранслятором
//! замерзают (так ведёт себя ТСПУ), и решает, идти ли мимо VPN или через него.
//! Публичного способа узнать это у iroh нет.

use std::sync::atomic::{AtomicU64, Ordering};

static RELAY_PING_TIMEOUTS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn note_relay_ping_timeout() {
    RELAY_PING_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
}

/// Сколько всего пингов ретранслятору ушло без ответа с запуска процесса.
pub fn relay_ping_timeouts() -> u64 {
    RELAY_PING_TIMEOUTS.load(Ordering::Relaxed)
}
