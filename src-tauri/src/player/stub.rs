//! Заглушка для платформ помимо macOS и Windows.
//!
//! Молча возвращать пустой список нельзя: со стороны это «кнопка есть, а не
//! работает». Честный отказ с причиной хотя бы объясняет, что происходит.

use anyhow::{anyhow, Result};

use super::{Command, Sink, Source};

pub fn applications() -> Result<Vec<Source>> {
    Ok(Vec::new())
}

pub fn control(_command: Command) -> Result<()> {
    Err(anyhow!(
        "управление источником на этой платформе пока не сделано"
    ))
}

pub struct Tap;

impl Tap {
    pub fn start(_source: &str, _sink: Sink) -> Result<Self> {
        Err(anyhow!(
            "захват звука приложений на этой платформе пока не сделан"
        ))
    }

    /// Звучит ли источник прямо сейчас. Заглушке звучать нечем.
    pub fn sounding(&self) -> bool {
        false
    }
}
