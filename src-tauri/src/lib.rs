//! БРЕД — Быстрая Ретрансляция Электронных Данных.
//!
//! Мессенджер без сервера: узлы находят друг друга сами и обмениваются
//! подписанными событиями напрямую.

pub mod app;
pub mod audio;
pub mod commands;
pub mod domain;
pub mod http;
pub mod identity;
pub mod logs;
pub mod net;
pub mod news;
pub mod player;
pub mod report;
pub mod store;
pub mod vault;

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
    Keyframe,
    Bitrate {
        track: &'static str,
        bps: u32,
    },
    Player {
        space: Id,
    },
    Speaking {
        authors: Vec<Id>,
    },
    Ring {
        space: Id,
        channel: Id,
        from: Id,
        nick: String,
        /// Строкой: в JavaScript u64 не помещается без потерь.
        id: String,
    },
    RingCancel {
        space: Id,
        from: Id,
        id: String,
    },
    News {
        version: String,
        title: String,
    },
    NewsFeed,
    /// Ключ пространства сменили без нас.
    KeyLost {
        space: Id,
    },
    /// Нас исключили из пространства.
    Removed {
        space: Id,
        name: String,
    },
    /// Ключ пространства сменился (у нас тоже).
    Rekeyed {
        space: Id,
    },
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
            Notice::Player { space } => UiNotice::Player { space },
            // Сюда не доходит: нажатие на пульте исполняет ядро, а интерфейсу
            // показывать нечего — человек узнаёт о результате по тому, что
            // музыка замолчала.
            Notice::PlayerCommand { .. } => UiNotice::Net,
            Notice::Keyframe => UiNotice::Keyframe,
            Notice::Bitrate { track, bps } => UiNotice::Bitrate { track, bps },
            Notice::Speaking { authors } => UiNotice::Speaking { authors },
            Notice::Ring {
                space,
                channel,
                from,
                nick,
                id,
            } => UiNotice::Ring {
                space,
                channel,
                from,
                nick,
                id: id.to_string(),
            },
            Notice::RingCancel { space, from, id } => UiNotice::RingCancel {
                space,
                from,
                id: id.to_string(),
            },
            Notice::News { version, title } => UiNotice::News { version, title },
            Notice::NewsFeed => UiNotice::NewsFeed,
            Notice::KeyLost { space } => UiNotice::KeyLost { space },
            Notice::Removed { space, name } => UiNotice::Removed { space, name },
            Notice::Rekeyed { space } => UiNotice::Rekeyed { space },
        }
    }
}

/// Поднимает хранилище, личность и сеть. Ошибка здесь не должна ронять окно:
/// приложение локальное, и показать историю оно обязано даже когда что-то
/// пошло не так.
/// База, которая ждёт кода: ядро не поднято, пока его не введут.
static LOCKED: parking_lot::Mutex<Option<std::path::PathBuf>> = parking_lot::Mutex::new(None);

/// Заперто ли приложение кодом.
pub fn locked() -> bool {
    LOCKED.lock().is_some()
}

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

    let vault = vault::Vault::at(&data_dir);
    if vault.lock()? == Some(vault::Lock::Passcode) {
        tracing::info!("база закрыта код-паролем — ждём его");
        *LOCKED.lock() = Some(db_path);
        return Ok(());
    }
    let key = vault.open_key()?;
    tauri::async_runtime::block_on(launch(handle.clone(), db_path, key, None))
}

/// Разблокировка и стирание — по одному за раз: два одновременных верных
/// кода подняли бы ядро дважды, а параллельный перебор обходил бы паузу.
static UNLOCKING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Открыть запертую базу кодом и поднять ядро.
pub async fn unlock(handle: tauri::AppHandle, passcode: &str) -> anyhow::Result<()> {
    let _one_at_a_time = UNLOCKING.lock().await;
    let Some(db_path) = LOCKED.lock().clone() else {
        return Ok(()); // уже открыто
    };
    let vault = vault::Vault::at(app::data_dir_of(&db_path));
    let (key, previous) = match vault.unlock_with_previous(passcode) {
        Ok(keys) => keys,
        Err(err) => {
            // Пауза на каждую неудачу: перебирать коды руками у открытого
            // ноутбука становится бессмысленно.
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            return Err(err);
        }
    };
    launch(handle, db_path, key, previous).await?;
    *LOCKED.lock() = None;
    Ok(())
}

/// Код забыт: стереть данные этого устройства и начать заново.
///
/// Другого пути нет и быть не должно — если бы базу можно было открыть без
/// кода, код ничего бы не защищал.
pub async fn wipe_locked(handle: tauri::AppHandle) -> anyhow::Result<()> {
    let _one_at_a_time = UNLOCKING.lock().await;
    let Some(db_path) = LOCKED.lock().clone() else {
        anyhow::bail!("приложение не заперто");
    };
    let dir = app::data_dir_of(&db_path).to_path_buf();
    for name in [
        "bred.sqlite",
        "bred.sqlite-wal",
        "bred.sqlite-shm",
        "vault.bin",
    ] {
        let _ = std::fs::remove_file(dir.join(name));
    }
    let _ = std::fs::remove_dir_all(dir.join("blobs"));
    tracing::warn!("код забыт — данные устройства стёрты");
    let key = vault::Vault::at(&dir).open_key()?;
    launch(handle, db_path, key, None).await?;
    *LOCKED.lock() = None;
    Ok(())
}

async fn launch(
    handle: tauri::AppHandle,
    db_path: std::path::PathBuf,
    key: [u8; 32],
    previous: Option<[u8; 32]>,
) -> anyhow::Result<()> {
    let (application, mut notices) = App::start_with_keys(&db_path, key, previous).await?;

    // Уборка при запуске: файлы могли осиротеть, пока приложение
    // было закрыто (например, автор удалил сообщение).
    let janitor = application.clone();
    // Канал «Обновления БРЕД»: заглядываем в релизы, пока приложение открыто.
    application.news.watch();
    handle.manage(application);
    tauri::async_runtime::spawn(async move {
        if let Err(err) = janitor.collect_garbage().await {
            tracing::debug!(%err, "уборка вложений не удалась");
        }
    });

    let emitter = handle.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(notice) = notices.recv().await {
            // Пульт исполняется здесь и наверх не идёт: нажали у соседа —
            // нажать надо в приложении-источнике, а не в интерфейсе.
            if let Notice::PlayerCommand { command } = notice {
                if let Some(app) = emitter.try_state::<std::sync::Arc<App>>() {
                    app.player_command(command);
                }
                continue;
            }
            let _ = emitter.emit(NOTICE_EVENT, UiNotice::from(notice));
        }
    });
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("BRED_LOG")
                .unwrap_or_else(|_| "bred=info,iroh=warn".into()),
        )
        // Журнал идёт и в терминал, и в хвост в памяти — его можно приложить
        // к отчёту о проблеме одной галочкой.
        .with_writer(logs::Tee)
        .init();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init());
    // Обновления и перезапуск — дело десктопа: на телефоне сборку ставит магазин.
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init());

    builder
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
            commands::lock_state,
            commands::unlock,
            commands::wipe_locked,
            commands::set_passcode,
            commands::clear_passcode,
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
            commands::space_governance,
            commands::list_invites,
            commands::revoke_invite,
            commands::remove_member,
            commands::rotate_space_key,
            commands::set_admin,
            commands::privacy_info,
            commands::set_hide_ip,
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
            commands::ring,
            commands::account_info,
            commands::report_info,
            commands::report_files,
            commands::send_report,
            commands::news_feed,
            commands::news_read,
            commands::news_refresh,
            commands::voice_map,
            commands::send_media,
            commands::music_sources,
            commands::music_start,
            commands::music_stop,
            commands::music_control,
            commands::player_state,
            commands::media_stream,
            commands::voice_stream,
            commands::send_pcm,
            commands::set_voice_volume,
            commands::set_music_volume,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|err| {
            // Сюда попадаем, только если не удалось создать само окно.
            tracing::error!(%err, "БРЕД не смог открыть окно");
            std::process::exit(1);
        });
}
