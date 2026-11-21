//! Передача файлов между узлами.
//!
//! В событие сообщения едет только описание вложения (хеш, имя, размер), а сами
//! байты забираются по требованию у того, у кого они есть. Иначе лог, который
//! реплицируется целиком, распухал бы от медиа до неприличия.
//!
//! Адресация контентная: имя файла ничего не гарантирует, а blake3-хеш — и
//! идентификатор, и проверка целостности. Скачивание возобновляемое: докачка
//! начинается со смещения, а не с нуля.

use anyhow::{anyhow, Context, Result};
use iroh::{
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr, EndpointId,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    ctx::Ctx,
    wire::{open, read_frame, seal, write_frame},
};
use crate::domain::{Attachment, Id, SpaceId};

pub const BLOB_ALPN: &[u8] = b"bred/blob/1";

/// Потолок на размер файла. Без него один пир может занять весь диск другого.
pub const MAX_BLOB: u64 = 512 * 1024 * 1024;

/// Размер куска при чтении и записи.
const CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlobRequest {
    pub space: SpaceId,
    pub hash: Id,
    /// С какого байта продолжать. Ноль — качаем с начала.
    pub offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BlobReply {
    /// Файл есть, дальше по потоку идут сырые байты начиная со смещения.
    Sending { total: u64 },
    /// У этого узла файла нет — спросим следующего.
    Missing,
}

/// Приём входящих запросов на файлы.
#[derive(Clone)]
pub struct BlobProtocol {
    ctx: Arc<Ctx>,
}

impl BlobProtocol {
    pub fn new(ctx: Arc<Ctx>) -> Self {
        Self { ctx }
    }
}

impl std::fmt::Debug for BlobProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BlobProtocol")
    }
}

impl ProtocolHandler for BlobProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        if let Err(err) = serve(self.ctx.clone(), connection).await {
            tracing::warn!(%err, "раздача файла прервана");
        }
        Ok(())
    }
}

/// Раздача по одному соединению — сколько угодно файлов, каждый своим потоком.
///
/// Раньше соединение обслуживало один файл, и на каждую картинку получатель
/// заново искал адрес, пробивал NAT и проходил рукопожатие — через
/// ретранслятор это секунды на файл. Цикл заканчивается, когда получатель сам
/// закрывает соединение, поэтому байты, которые он ещё не дочитал, не
/// обрываются: это ровно то, ради чего раньше здесь ждали `closed()`.
///
/// Старые версии открывают один поток и закрывают соединение — для них ничего
/// не изменилось.
async fn serve(ctx: Arc<Ctx>, connection: Connection) -> Result<()> {
    let peer = Id(*connection.remote_id().as_bytes());
    while let Ok((send, recv)) = connection.accept_bi().await {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            if let Err(err) = serve_stream(&ctx, send, recv, Some(peer)).await {
                tracing::debug!(%err, "раздача файла прервана");
            }
        });
    }
    Ok(())
}

/// Раздача поверх пары потоков. Отделена от QUIC-соединения, чтобы протокол
/// можно было прогнать в тесте через обычную трубу, а не поднимать сеть.
pub async fn serve_stream<W, R>(ctx: &Ctx, mut send: W, mut recv: R, peer: Option<Id>) -> Result<()>
where
    W: AsyncWriteExt + Unpin,
    R: AsyncReadExt + Unpin,
{
    let raw = read_frame(&mut recv).await?;
    // Запрос зашифрован ключом пространства: это и есть проверка доступа —
    // без ключа корректный запрос не собрать.
    let Some((space, request)) = ctx.open_with_known_key::<BlobRequest>(&raw) else {
        return Err(anyhow!("запрос не подошёл ни к одному известному ключу"));
    };
    if request.space != space {
        return Err(anyhow!("пространство в запросе не совпало с ключом"));
    }
    if peer.is_some_and(|peer| ctx.is_removed(space, peer)) {
        return Err(anyhow!("файл просит исключённый участник"));
    }
    let key = ctx
        .space(space)
        .ok_or_else(|| anyhow!("неизвестное пространство"))?
        .key;

    let path = ctx.store.blob_path(request.hash)?;
    let Some(path) = path.filter(|p| p.exists()) else {
        write_frame(&mut send, &seal(&key, &BlobReply::Missing)?).await?;
        send.flush().await?;
        send.shutdown().await?;
        return Ok(());
    };

    let mut file = tokio::fs::File::open(&path).await?;
    let total = file.metadata().await?.len();
    if request.offset > total {
        return Err(anyhow!("смещение за пределами файла"));
    }

    write_frame(&mut send, &seal(&key, &BlobReply::Sending { total })?).await?;
    tokio::io::AsyncSeekExt::seek(&mut file, std::io::SeekFrom::Start(request.offset)).await?;

    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        send.write_all(&buffer[..read]).await?;
    }
    send.flush().await?;
    send.shutdown().await?;
    tracing::debug!(hash = %request.hash.short(), total, "файл отдан");
    Ok(())
}

/// Сколько ждём, пока узел возьмёт трубку. Мёртвый адрес должен отваливаться
/// быстро: за ним в очереди стоят живые.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
/// Сколько ждём ответа «есть / нет» на уже поднятом соединении.
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);
/// Обрыв — это не «долго», а «ни байта за это время».
///
/// Раньше на весь файл было три минуты. Через ретранслятор за них проходит
/// сотня-другая мегабайт, и большой файл рвался на середине при совершенно
/// живой связи — каждая следующая попытка упиралась в тот же срок.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(20);

/// У этого узла файла нет — спрашивать его снова незачем, в отличие от
/// узла, который просто не ответил.
#[derive(Debug)]
pub struct NotHere;

impl std::fmt::Display for NotHere {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("у пира нет этого файла")
    }
}

impl std::error::Error for NotHere {}

/// Ответ держателя: файл есть, байты пойдут по этому потоку.
pub struct Offer {
    pub total: u64,
    /// Держим, пока принимаем: закрытие нашей стороны держатель понял бы как
    /// отказ.
    _send: SendStream,
    recv: RecvStream,
}

/// Соединения для файлов — по одному на держателя, переиспользуются.
#[derive(Default)]
pub struct BlobClient {
    pool: Mutex<HashMap<EndpointId, Connection>>,
}

impl BlobClient {
    /// Спросить держателя о файле. Сперва по живому соединению, и только если
    /// его нет или оно умерло — новым.
    pub async fn ask(
        &self,
        endpoint: &Endpoint,
        peer: EndpointId,
        space: SpaceId,
        hash: Id,
        key: &[u8; 32],
        offset: u64,
    ) -> Result<Offer> {
        let pooled = self.pool.lock().get(&peer).cloned();
        if let Some(connection) = pooled {
            match ask_over(&connection, space, hash, key, offset).await {
                Ok(offer) => return Ok(offer),
                Err(err) if err.is::<NotHere>() => return Err(err),
                Err(_) => {
                    self.pool.lock().remove(&peer);
                }
            }
        }
        let connection = tokio::time::timeout(
            CONNECT_TIMEOUT,
            endpoint.connect(EndpointAddr::from(peer), BLOB_ALPN),
        )
        .await
        .map_err(|_| anyhow!("узел не отвечает"))??;
        self.pool.lock().insert(peer, connection.clone());
        ask_over(&connection, space, hash, key, offset).await
    }
}

async fn ask_over(
    connection: &Connection,
    space: SpaceId,
    hash: Id,
    key: &[u8; 32],
    offset: u64,
) -> Result<Offer> {
    tokio::time::timeout(REPLY_TIMEOUT, async {
        let (mut send, mut recv) = connection.open_bi().await?;
        let total = request(&mut send, &mut recv, space, hash, key, offset).await?;
        Ok(Offer {
            total,
            _send: send,
            recv,
        })
    })
    .await
    .map_err(|_| anyhow!("держатель молчит"))?
}

/// Отправить запрос и прочитать ответ. `Ok` — сколько всего байт в файле.
async fn request<W, R>(
    send: &mut W,
    recv: &mut R,
    space: SpaceId,
    hash: Id,
    key: &[u8; 32],
    offset: u64,
) -> Result<u64>
where
    W: AsyncWriteExt + Unpin,
    R: AsyncReadExt + Unpin,
{
    let request = BlobRequest {
        space,
        hash,
        offset,
    };
    write_frame(send, &seal(key, &request)?).await?;
    send.flush().await?;

    let raw = read_frame(recv).await?;
    let total = match open::<BlobReply>(key, &raw)? {
        BlobReply::Sending { total } => total,
        BlobReply::Missing => return Err(NotHere.into()),
    };
    if total > MAX_BLOB {
        return Err(anyhow!("пир заявил файл больше допустимого"));
    }
    if offset > total {
        return Err(anyhow!("у пира файл короче, чем мы уже скачали"));
    }
    Ok(total)
}

/// Принять файл по ответу держателя.
pub async fn receive(
    ctx: &Ctx,
    offer: Offer,
    hash: Id,
    target: &Path,
    partial: &Path,
    offset: u64,
) -> Result<PathBuf> {
    let Offer { total, recv, _send } = offer;
    let path = receive_into(ctx, recv, total, hash, target, partial, offset).await;
    drop(_send);
    path
}

/// Скачивание поверх пары потоков — см. пояснение к `serve_stream`.
#[allow(clippy::too_many_arguments)]
pub async fn fetch_stream<W, R>(
    ctx: &Ctx,
    mut send: W,
    mut recv: R,
    space: SpaceId,
    hash: Id,
    key: &[u8; 32],
    target: &Path,
    partial: &Path,
    offset: u64,
) -> Result<PathBuf>
where
    W: AsyncWriteExt + Unpin,
    R: AsyncReadExt + Unpin,
{
    let total = request(&mut send, &mut recv, space, hash, key, offset).await?;
    receive_into(ctx, recv, total, hash, target, partial, offset).await
}

async fn receive_into<R>(
    ctx: &Ctx,
    mut recv: R,
    total: u64,
    hash: Id,
    target: &Path,
    partial: &Path,
    offset: u64,
) -> Result<PathBuf>
where
    R: AsyncReadExt + Unpin,
{
    if let Some(dir) = partial.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(partial)
        .await?;

    let mut buffer = vec![0u8; CHUNK];
    let mut written = offset;
    loop {
        let read = tokio::time::timeout(STALL_TIMEOUT, recv.read(&mut buffer))
            .await
            .map_err(|_| anyhow!("держатель замолчал на {written} из {total} байт"))??;
        if read == 0 {
            break;
        }
        written += read as u64;
        if written > total {
            drop(file);
            // Лишнее нельзя просто отрезать: неизвестно, где начался мусор.
            let _ = tokio::fs::remove_file(partial).await;
            return Err(anyhow!("пир прислал больше, чем обещал"));
        }
        file.write_all(&buffer[..read]).await?;
    }
    file.flush().await?;
    drop(file);

    if written != total {
        // Недокачано — оставляем .part, следующая попытка продолжит с этого места.
        return Err(anyhow!("получено {written} из {total} байт"));
    }

    verify(partial, hash).await?;
    tokio::fs::rename(partial, target).await?;
    ctx.store.record_blob(hash, target, total)?;
    tracing::info!(hash = %hash.short(), total, "файл получен");
    Ok(target.to_path_buf())
}

/// Положить локальный файл в хранилище вложений и получить его описание.
pub async fn import(ctx: &Ctx, source: &Path) -> Result<Attachment> {
    let meta = tokio::fs::metadata(source)
        .await
        .with_context(|| format!("не удалось прочитать {}", source.display()))?;
    if meta.len() > MAX_BLOB {
        return Err(anyhow!("файл больше допустимых {MAX_BLOB} байт"));
    }

    let hash = hash_file(source).await?;
    let target = ctx.blob_path(hash);
    if let Some(dir) = target.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    // Уже есть — второй раз не копируем: адресация контентная, файл тот же.
    if !target.exists() {
        tokio::fs::copy(source, &target).await?;
    }
    ctx.store.record_blob(hash, &target, meta.len())?;

    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| hash.short());

    Ok(Attachment {
        hash,
        name,
        size: meta.len(),
        mime: guess_mime(source),
    })
}

/// Хеш файла потоково: файл может не поместиться в память.
pub async fn hash_file(path: &Path) -> Result<Id> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(Id(*hasher.finalize().as_bytes()))
}

async fn verify(path: &Path, expected: Id) -> Result<()> {
    let actual = hash_file(path).await?;
    if actual != expected {
        // Файл битый или подменённый — удаляем, чтобы докачка не продолжала мусор.
        let _ = tokio::fs::remove_file(path).await;
        return Err(anyhow!("хеш не сошёлся: файл повреждён или подменён"));
    }
    Ok(())
}

/// Тип содержимого по расширению. Нужен только для показа в UI.
fn guess_mime(path: &Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain",
        "json" => "application/json",
        "zip" => "application/zip",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hash_is_stable_and_content_addressed() {
        let dir = std::env::temp_dir().join(format!("bred-blob-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();

        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        tokio::fs::write(&a, "одинаковое содержимое").await.unwrap();
        tokio::fs::write(&b, "одинаковое содержимое").await.unwrap();
        assert_eq!(
            hash_file(&a).await.unwrap(),
            hash_file(&b).await.unwrap(),
            "одинаковые байты обязаны дать одинаковый адрес"
        );

        tokio::fs::write(&b, "другое содержимое").await.unwrap();
        assert_ne!(hash_file(&a).await.unwrap(), hash_file(&b).await.unwrap());

        tokio::fs::remove_dir_all(&dir).await.ok();
    }

    #[tokio::test]
    async fn tampered_file_fails_verification() {
        let dir = std::env::temp_dir().join(format!("bred-verify-{}", std::process::id()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let path = dir.join("file.bin");

        tokio::fs::write(&path, "исходные байты").await.unwrap();
        let hash = hash_file(&path).await.unwrap();
        assert!(verify(&path, hash).await.is_ok());

        tokio::fs::write(&path, "подменённые байты").await.unwrap();
        assert!(
            verify(&path, hash).await.is_err(),
            "подмена содержимого обязана ловиться хешем"
        );
        assert!(!path.exists(), "битый файл должен удаляться");

        tokio::fs::remove_dir_all(&dir).await.ok();
    }

    #[test]
    fn mime_is_guessed_from_extension() {
        assert_eq!(guess_mime(Path::new("скрин.png")), "image/png");
        assert_eq!(guess_mime(Path::new("архив.zip")), "application/zip");
        assert_eq!(
            guess_mime(Path::new("без-расширения")),
            "application/octet-stream"
        );
    }
}
