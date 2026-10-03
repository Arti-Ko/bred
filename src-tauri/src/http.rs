//! HTTP наружу: отчёты в Telegram и лента обновлений с GitHub.
//!
//! `reqwest` собран без своего набора шифров — им служит `ring`, та же, что у
//! iroh. Набор ставится один раз на процесс, повторная установка — не ошибка.
//! Без этого клиент падал бы при создании.

use anyhow::{Context, Result};
use std::time::Duration;

pub fn client(timeout: Duration) -> Result<reqwest::Client> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(concat!("BRED/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("HTTP-клиент не поднялся")
}
