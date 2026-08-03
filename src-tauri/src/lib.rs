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

/// Пустой ответ схемы вложений: файла нет или он ещё не скачан.
fn not_found() -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(404)
        .body(Vec::new())
        .expect("пустой ответ собирается всегда")
}

/// Причина, по которой приложение не смогло подняться.
///
/// Раньше ошибка запуска летела из `setup` наружу и превращалась в abort —
/// человек видел отчёт о падении вместо объяснения. Теперь окно открывается
/// всегда, а причина показывается в интерфейсе.
static STARTUP_ERROR: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn startup_error() -> Option<String> {
    STARTUP_ERROR.get().cloned()
}

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
    Version {
        space: Id,
        theirs: u16,
        ours: u16,
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
            Notice::Version {
                space,
                theirs,
                ours,
            } => UiNotice::Version {
                space,
                theirs,
                ours,
            },
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

/// Поднимает хранилище, личность и сеть. Ошибка здесь не должна ронять окно:
/// приложение локальное, и показать историю оно обязано даже когда что-то
/// пошло не так.
fn start_core(handle: &tauri::AppHandle) -> anyhow::Result<()> {
    // BRED_DATA_DIR позволяет держать несколько независимых профилей:
    // без него два экземпляра на одной машине подхватили бы один ключ
    // и оказались бы одним и тем же узлом.
    let data_dir = match std::env::var_os("BRED_DATA_DIR") {
        Some(custom) => std::path::PathBuf::from(custom),
        None => handle.path().app_data_dir()?,
    };
    let db_path = data_dir.join("bred.sqlite");
    tracing::info!(dir = %data_dir.display(), "каталог данных");

    let handle = handle.clone();
    tauri::async_runtime::block_on(async move {
        let (application, mut notices) = App::start(&db_path).await?;

        // Уборка при запуске: файлы могли осиротеть, пока приложение
        // было закрыто (например, автор удалил сообщение).
        let janitor = application.clone();
        handle.manage(application);
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
    })
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
        // Вложения отдаём собственной схемой, а не через мост команд: сырой
        // ответ на десятки мегабайт рвал IPC — в интерфейсе это выглядело как
        // «connection lost», после чего переставало работать вообще всё.
        .register_uri_scheme_protocol("bredfile", |ctx, request| {
            use tauri::Manager;

            let Some(app) = ctx.app_handle().try_state::<std::sync::Arc<App>>() else {
                return not_found();
            };
            // Путь вида /<хеш>: имя файла в адресе не нужно, адресация по содержимому.
            let hash = request.uri().path().trim_start_matches('/').to_string();
            let Ok(hash) = Id::parse(&hash) else {
                return not_found();
            };

            match app.attachment_file(hash) {
                Some((bytes, mime)) => tauri::http::Response::builder()
                    .status(200)
                    .header("Content-Type", mime)
                    .header("Cache-Control", "max-age=31536000, immutable")
                    .body(bytes)
                    .unwrap_or_else(|_| not_found()),
                None => not_found(),
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            if let Err(err) = start_core(&handle) {
                tracing::error!(%err, "не удалось поднять ядро");
                let _ = STARTUP_ERROR.set(format!("{err:#}"));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::net_status,
            commands::list_spaces,
            commands::list_channels,
            commands::list_messages,
            commands::list_thread,
            commands::attachment_url,
            commands::ensure_attachment,
            commands::save_attachment,
            commands::collect_garbage,
            commands::get_message,
            commands::list_members,
            commands::create_space,
            commands::join_space,
            commands::space_invite,
            commands::open_direct,
            commands::personal_link,
            commands::open_direct_link,
            commands::list_emojis,
            commands::add_emoji,
            commands::remove_emoji,
            commands::leave_space,
            commands::create_channel,
            commands::delete_channel,
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
        .unwrap_or_else(|err| {
            // Сюда попадаем, только если не удалось создать само окно.
            tracing::error!(%err, "БРЕД не смог открыть окно");
            std::process::exit(1);
        });
}
