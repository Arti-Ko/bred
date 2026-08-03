//! Команды, доступные фронтенду. Тонкий слой: разбор аргументов, вызов
//! прикладного слоя, перевод ошибок в строку для UI.

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::{
    app::App,
    domain::{Attachment, Id, SpaceId},
    store::{ChannelRow, EmojiRow, MemberRow, MessageRow, SpaceRow},
};

/// Ошибки наружу отдаём текстом: UI показывает их в статус-строке как есть.
type Answer<T> = Result<T, String>;

fn fail(err: impl std::fmt::Display) -> String {
    err.to_string()
}

#[derive(Serialize)]
pub struct Bootstrap {
    pub me: Id,
    pub nick: String,
    pub endpoint: String,
    pub spaces: Vec<SpaceRow>,
}

#[derive(Serialize)]
pub struct NetStatus {
    pub endpoint: String,
    pub online: bool,
    pub spaces: usize,
}

#[tauri::command]
pub fn bootstrap(handle: tauri::AppHandle) -> Answer<Bootstrap> {
    use tauri::Manager;

    // Ядро могло не подняться — тогда честно отдаём причину, а не молчим.
    let Some(app) = handle.try_state::<Arc<App>>() else {
        return Err(crate::startup_error().unwrap_or_else(|| "ядро не запустилось".to_string()));
    };

    Ok(Bootstrap {
        me: app.me(),
        nick: app.nick(),
        endpoint: app.net.endpoint_id().to_string(),
        spaces: app.spaces().map_err(fail)?,
    })
}

#[tauri::command]
pub fn net_status(app: State<'_, Arc<App>>) -> Answer<NetStatus> {
    Ok(NetStatus {
        endpoint: app.net.endpoint_id().to_string(),
        online: !app.net.addr_bytes().is_empty(),
        spaces: app.ctx.space_list().len(),
    })
}

#[tauri::command]
pub fn list_spaces(app: State<'_, Arc<App>>) -> Answer<Vec<SpaceRow>> {
    app.spaces().map_err(fail)
}

#[tauri::command]
pub fn list_channels(app: State<'_, Arc<App>>, space: SpaceId) -> Answer<Vec<ChannelRow>> {
    app.channels(space).map_err(fail)
}

#[tauri::command]
pub fn list_messages(
    app: State<'_, Arc<App>>,
    channel: Id,
    before: Option<u64>,
) -> Answer<Vec<MessageRow>> {
    app.messages(channel, before).map_err(fail)
}

#[tauri::command]
pub fn list_thread(app: State<'_, Arc<App>>, root: Id) -> Answer<Vec<MessageRow>> {
    app.thread(root).map_err(fail)
}

/// Байты вложения для показа в ленте. Отдаём сырыми — картинка в JSON
/// раздулась бы на треть и грузила бы разбор.
#[tauri::command]
pub async fn attachment_bytes(app: State<'_, Arc<App>>, hash: Id) -> Answer<tauri::ipc::Response> {
    let bytes = app.attachment_bytes(hash).await.map_err(fail)?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub async fn collect_garbage(app: State<'_, Arc<App>>) -> Answer<u64> {
    app.collect_garbage().await.map_err(fail)
}

#[tauri::command]
pub fn get_message(app: State<'_, Arc<App>>, id: Id) -> Answer<Option<MessageRow>> {
    app.message(id).map_err(fail)
}

#[tauri::command]
pub fn list_members(app: State<'_, Arc<App>>, space: SpaceId) -> Answer<Vec<MemberRow>> {
    app.members(space).map_err(fail)
}

#[tauri::command]
pub async fn create_space(app: State<'_, Arc<App>>, name: String) -> Answer<SpaceId> {
    app.create_space(&name).await.map_err(fail)
}

#[tauri::command]
pub async fn join_space(app: State<'_, Arc<App>>, ticket: String) -> Answer<SpaceId> {
    app.join_space(&ticket).await.map_err(fail)
}

#[tauri::command]
pub async fn leave_space(app: State<'_, Arc<App>>, space: SpaceId) -> Answer<()> {
    app.leave_space(space).await.map_err(fail)
}

/// Открыть личную переписку. Приглашение не нужно.
#[tauri::command]
pub async fn open_direct(app: State<'_, Arc<App>>, peer: Id) -> Answer<SpaceId> {
    app.open_direct(peer).await.map_err(fail)
}

/// Ссылка-визитка для связи один на один.
#[tauri::command]
pub fn personal_link(app: State<'_, Arc<App>>) -> Answer<String> {
    Ok(app.personal_link())
}

#[tauri::command]
pub async fn open_direct_link(app: State<'_, Arc<App>>, link: String) -> Answer<SpaceId> {
    app.open_direct_link(&link).await.map_err(fail)
}

#[tauri::command]
pub fn list_emojis(app: State<'_, Arc<App>>, space: SpaceId) -> Answer<Vec<EmojiRow>> {
    app.emojis(space).map_err(fail)
}

#[tauri::command]
pub async fn add_emoji(
    app: State<'_, Arc<App>>,
    space: SpaceId,
    name: String,
    path: String,
    sticker: bool,
) -> Answer<()> {
    app.add_emoji(space, &name, std::path::Path::new(&path), sticker)
        .await
        .map_err(fail)
}

#[tauri::command]
pub async fn remove_emoji(app: State<'_, Arc<App>>, space: SpaceId, name: String) -> Answer<()> {
    app.remove_emoji(space, &name).await.map_err(fail)
}

#[tauri::command]
pub fn space_invite(app: State<'_, Arc<App>>, space: SpaceId) -> Answer<String> {
    app.invite(space).map_err(fail)
}

#[tauri::command]
pub async fn create_channel(
    app: State<'_, Arc<App>>,
    space: SpaceId,
    name: String,
    category: String,
    voice: bool,
) -> Answer<Id> {
    app.create_channel(space, &name, &category, voice)
        .await
        .map_err(fail)
}

#[tauri::command]
pub async fn send_message(
    app: State<'_, Arc<App>>,
    space: SpaceId,
    channel: Id,
    body: String,
    reply_to: Option<Id>,
    thread: Option<Id>,
    attachments: Option<Vec<Attachment>>,
) -> Answer<Id> {
    app.send_message(
        space,
        channel,
        &body,
        reply_to,
        thread,
        attachments.unwrap_or_default(),
    )
    .await
    .map_err(fail)
}

// ── вложения ────────────────────────────────────────────────────────────────

/// Подготовить файл к отправке: посчитать хеш и положить в хранилище вложений.
#[tauri::command]
pub async fn attach_file(app: State<'_, Arc<App>>, path: String) -> Answer<Attachment> {
    app.attach(std::path::Path::new(&path)).await.map_err(fail)
}

/// Скачать вложение у того, у кого оно есть. Возвращает путь к файлу.
/// Поставить картинку профиля: годится и PNG, и анимированный GIF.
#[tauri::command]
pub async fn set_avatar(app: State<'_, Arc<App>>, path: String) -> Answer<Attachment> {
    app.set_avatar(std::path::Path::new(&path))
        .await
        .map_err(fail)
}

#[tauri::command]
pub async fn download_attachment(
    app: State<'_, Arc<App>>,
    space: SpaceId,
    hash: Id,
) -> Answer<String> {
    app.download(space, hash).await.map_err(fail)
}

// ── звонки ──────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct CallState {
    pub space: Option<Id>,
    pub channel: Option<Id>,
    pub participants: Vec<Id>,
}

#[tauri::command]
pub async fn join_call(app: State<'_, Arc<App>>, space: SpaceId, channel: Id) -> Answer<()> {
    app.join_call(space, channel).await.map_err(fail)
}

#[tauri::command]
pub async fn leave_call(app: State<'_, Arc<App>>) -> Answer<()> {
    app.leave_call().await.map_err(fail)
}

#[tauri::command]
pub fn call_state(app: State<'_, Arc<App>>) -> Answer<CallState> {
    let active = app.call_state();
    Ok(CallState {
        space: active.map(|(space, _)| space),
        channel: active.map(|(_, channel)| channel),
        participants: app.call_participants(),
    })
}

/// Кто в каком голосовом канале — чтобы рисовать состав комнат в списке каналов.
#[tauri::command]
pub fn voice_map(app: State<'_, Arc<App>>, space: SpaceId) -> Answer<Vec<(Id, Id)>> {
    Ok(app.voice_map(space))
}

/// Кадр от интерфейса. Команда синхронная и возвращает управление сразу:
/// кодировщик не должен ждать сеть, иначе поплывёт задержка звука.
#[tauri::command]
pub fn send_media(app: State<'_, Arc<App>>, request: tauri::ipc::Request<'_>) -> Answer<()> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("ожидались сырые байты кадра".to_string());
    };
    let frame = bytes.clone();
    let app = (*app).clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = app.send_media(&frame).await {
            tracing::debug!(%err, "кадр не ушёл");
        }
    });
    Ok(())
}

/// Подписка на входящие кадры. Байты идут сырыми, без JSON:
/// видео в base64 стоило бы лишней трети трафика и заметного CPU.
#[tauri::command]
pub fn media_stream(
    app: State<'_, Arc<App>>,
    channel: tauri::ipc::Channel<tauri::ipc::InvokeResponseBody>,
) -> Answer<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    app.net.media().set_sink(tx);

    tauri::async_runtime::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if channel
                .send(tauri::ipc::InvokeResponseBody::Raw(frame))
                .is_err()
            {
                break; // окно закрыли — поток больше некому слушать
            }
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn react(
    app: State<'_, Arc<App>>,
    space: SpaceId,
    target: Id,
    emoji: String,
    remove: bool,
) -> Answer<()> {
    app.react(space, target, &emoji, remove).await.map_err(fail)
}

#[tauri::command]
pub async fn edit_message(
    app: State<'_, Arc<App>>,
    space: SpaceId,
    target: Id,
    body: String,
) -> Answer<()> {
    app.edit_message(space, target, &body).await.map_err(fail)
}

#[tauri::command]
pub async fn delete_message(app: State<'_, Arc<App>>, space: SpaceId, target: Id) -> Answer<()> {
    app.delete_message(space, target).await.map_err(fail)
}

#[tauri::command]
pub async fn set_nick(app: State<'_, Arc<App>>, nick: String) -> Answer<()> {
    app.set_nick(&nick).await.map_err(fail)
}

#[tauri::command]
pub fn mark_read(app: State<'_, Arc<App>>, channel: Id) -> Answer<()> {
    app.mark_read(channel).map_err(fail)
}

#[tauri::command]
pub async fn typing(app: State<'_, Arc<App>>, space: SpaceId, channel: Id) -> Answer<()> {
    app.typing(space, channel).await.map_err(fail)
}
