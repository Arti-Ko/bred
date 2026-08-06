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
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr,
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
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

async fn serve(ctx: Arc<Ctx>, connection: Connection) -> Result<()> {
    let (send, recv) = connection.accept_bi().await?;
    serve_stream(&ctx, send, recv).await?;

    // Ждём, пока получатель закроет соединение сам.
    //
    // Если вернуть управление сразу, соединение закроется здесь — вместе с
    // байтами, которые собеседник ещё не успел дочитать. У него это выглядит
    // как «connection lost», хотя файл был отдан целиком. Синхронизация
    // сообщений живёт в цикле и потому уцелела, а раздача файлов — разовая,
    // и обрывалась тем чаще, чем крупнее файл.
    connection.closed().await;
    Ok(())
}

/// Раздача поверх пары потоков. Отделена от QUIC-соединения, чтобы протокол
/// можно было прогнать в тесте через обычную трубу, а не поднимать сеть.
pub async fn serve_stream<W, R>(ctx: &Ctx, mut send: W, mut recv: R) -> Result<()>
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
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(6);

/// Скачать вложение у конкретного пира. Возвращает путь к готовому файлу.
pub async fn fetch(
    ctx: Arc<Ctx>,
    endpoint: &Endpoint,
    peer: EndpointAddr,
    space: SpaceId,
    hash: Id,
    expected_size: u64,
) -> Result<PathBuf> {
    if expected_size > MAX_BLOB {
        return Err(anyhow!("файл больше допустимых {MAX_BLOB} байт"));
    }
    let key = ctx
        .space(space)
        .ok_or_else(|| anyhow!("неизвестное пространство"))?
        .key;

    let target = ctx.blob_path(hash);
    let partial = target.with_extension("part");
    // Докачиваем: то, что уже успели получить в прошлый раз, не выбрасываем.
    let offset = tokio::fs::metadata(&partial)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    let connection = tokio::time::timeout(CONNECT_TIMEOUT, endpoint.connect(peer, BLOB_ALPN))
        .await
        .map_err(|_| anyhow!("узел не отвечает"))??;
    let (send, recv) = connection.open_bi().await?;
    fetch_stream(
        ctx.as_ref(),
        send,
        recv,
        space,
        hash,
        &key,
        &target,
        &partial,
        offset,
    )
    .await
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
    let request = BlobRequest {
        space,
        hash,
        offset,
    };
    write_frame(&mut send, &seal(key, &request)?).await?;
    send.flush().await?;

    let raw = read_frame(&mut recv).await?;
    let total = match open::<BlobReply>(key, &raw)? {
        BlobReply::Sending { total } => total,
        BlobReply::Missing => return Err(anyhow!("у пира нет этого файла")),
    };
    if total > MAX_BLOB {
        return Err(anyhow!("пир заявил файл больше допустимого"));
    }

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
        let read = recv.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        written += read as u64;
        if written > total {
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
