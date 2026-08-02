//! БРЕД — Быстрая Ретрансляция Электронных Данных.
//!
//! Мессенджер без сервера: узлы находят друг друга сами и обмениваются
//! подписанными событиями напрямую.

pub mod app;
pub mod commands;
pub mod domain;
pub mod identity;
pub mod net;
pub mod store;

use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::{app::App, domain::Id, net::Notice};

/// Имя события, по которому UI слушает изменения.
const NOTICE_EVENT: &str = "bred://notice";

/// Уведомление в формате фронтенда.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum UiNotice {
    Applied {
        space: Id,
        event: Id,
    },
    Presence {
        space: Id,
    },
    Fork {
        space: Id,
        author: Id,
    },
    Typing {
        space: Id,
        channel: Id,
        author: Id,
        nick: String,
    },
    Net,
}

impl From<Notice> for UiNotice {
    fn from(notice: Notice) -> Self {
        match notice {
            Notice::Applied { space, event } => UiNotice::Applied { space, event },
            Notice::Presence { space } => UiNotice::Presence { space },
            Notice::Fork { space, author } => UiNotice::Fork { space, author },
            Notice::Typing {
                space,
                channel,
                author,
                nick,
            } => UiNotice::Typing {
                space,
                channel,
                author,
                nick,
            },
            Notice::Net => UiNotice::Net,
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("BRED_LOG")
                .unwrap_or_else(|_| "bred=info,iroh=warn".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let handle = app.handle().clone();
            // BRED_DATA_DIR позволяет держать несколько независимых профилей:
            // без него два экземпляра на одной машине подхватили бы один ключ
            // и оказались бы одним и тем же узлом.
            let data_dir = match std::env::var_os("BRED_DATA_DIR") {
                Some(custom) => std::path::PathBuf::from(custom),
                None => handle.path().app_data_dir()?,
            };
            let db_path = data_dir.join("bred.sqlite");
            tracing::info!(dir = %data_dir.display(), "каталог данных");

            tauri::async_runtime::block_on(async move {
                let (application, mut notices) = App::start(&db_path).await?;
                let janitor = application.clone();
                handle.manage(application);

                // Уборка при запуске: файлы могли осиротеть, пока приложение
                // было закрыто (например, автор удалил сообщение).
                tauri::async_runtime::spawn(async move {
                    if let Err(err) = janitor.collect_garbage().await {
                        tracing::debug!(%err, "уборка вложений не удалась");
                    }
                });

                let emitter = handle.clone();
                tauri::async_runtime::spawn(async move {
                    while let Some(notice) = notices.recv().await {
                        let _ = emitter.emit(NOTICE_EVENT, UiNotice::from(notice));
                    }
                });
                anyhow::Ok(())
            })?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::net_status,
            commands::list_spaces,
            commands::list_channels,
            commands::list_messages,
            commands::list_thread,
            commands::attachment_bytes,
            commands::collect_garbage,
            commands::get_message,
            commands::list_members,
            commands::create_space,
            commands::join_space,
            commands::space_invite,
            commands::open_direct,
            commands::list_emojis,
            commands::add_emoji,
            commands::remove_emoji,
            commands::leave_space,
            commands::create_channel,
            commands::send_message,
            commands::react,
            commands::edit_message,
            commands::delete_message,
            commands::set_nick,
            commands::mark_read,
            commands::typing,
            commands::attach_file,
            commands::set_avatar,
            commands::download_attachment,
            commands::join_call,
            commands::leave_call,
            commands::call_state,
            commands::voice_map,
            commands::send_media,
            commands::media_stream,
        ])
        .run(tauri::generate_context!())
        .expect("не удалось запустить БРЕД");
}
