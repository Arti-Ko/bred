//! Сейф: ключ, которым зашифрована база, и необязательный код-пароль.
//!
//! База — это вся переписка и все ключи: личный, аккаунта, пространств.
//! Раньше она лежала на диске открытым текстом, и любой, кто прочитал файл,
//! получал и историю, и возможность говорить от вашего имени. Теперь база
//! зашифрована (SQLCipher), а её ключ хранится так, как позволяет система:
//!
//! - **код-пароль** — ключ закрыт производной от него (Argon2id). Без кода
//!   база — шум, на любой системе и с любой копии диска;
//! - **Windows без кода** — ключ в диспетчере учётных данных, его защищает
//!   вход в учётную запись;
//! - **macOS и Linux без кода** — ключ в файле рядом с базой. Связка ключей
//!   macOS спрашивала бы пароль после каждого обновления: сборка подписана
//!   без сертификата разработчика, и система каждый раз видит «другое»
//!   приложение. Это защищает копию базы, утёкшую отдельно, но не диск
//!   целиком — для этого и нужен код-пароль.

use anyhow::{anyhow, bail, Context, Result};
use chacha20poly1305::{aead::Aead, KeyInit, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const VAULT_FILE: &str = "vault.bin";
/// Короче — подбирается перебором за часы даже с Argon2.
pub const MIN_PASSCODE: usize = 6;

/// Цена одной попытки: 64 МиБ памяти и три прохода — около трети секунды
/// на ноутбуке, и неподъёмно для перебора на видеокартах.
const KDF_MEMORY_KIB: u32 = 64 * 1024;
const KDF_PASSES: u32 = 3;
const KDF_LANES: u32 = 1;

#[derive(Serialize, Deserialize)]
enum Sealed {
    /// Ключ в хранилище системы, здесь только отметка об этом.
    System,
    /// Ключ как есть.
    Plain([u8; 32]),
    /// Ключ закрыт код-паролем.
    Passcode {
        salt: [u8; 16],
        memory: u32,
        passes: u32,
        lanes: u32,
        sealed: Vec<u8>,
        /// Прежний ключ базы — только пока база переходит на новый. Сбой
        /// посреди перехода иначе оставил бы базу на ключе, которого нет в сейфе.
        #[serde(default)]
        previous: Option<Vec<u8>>,
    },
}

/// Нужен ли код, чтобы открыть базу.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lock {
    Open,
    Passcode,
}

pub struct Vault {
    path: PathBuf,
    /// Под каким именем ключ лежит в хранилище системы. Своё на каждый
    /// каталог данных: профилей на одной машине может быть несколько.
    #[cfg_attr(not(windows), allow(dead_code))]
    account: String,
}

impl Vault {
    pub fn at(dir: &Path) -> Self {
        let account = blake3::hash(dir.to_string_lossy().as_bytes()).to_hex()[..16].to_string();
        Self {
            path: dir.join(VAULT_FILE),
            account,
        }
    }

    /// `None` — сейфа ещё нет: первый запуск этой версии.
    pub fn lock(&self) -> Result<Option<Lock>> {
        Ok(self.read()?.map(|sealed| match sealed {
            Sealed::Passcode { .. } => Lock::Passcode,
            _ => Lock::Open,
        }))
    }

    /// Ключ базы без кода. Сейфа нет — заводит новый.
    pub fn open_key(&self) -> Result<[u8; 32]> {
        match self.read()? {
            None => {
                let key: [u8; 32] = rand::random();
                self.keep_on_device(&key)?;
                Ok(key)
            }
            Some(Sealed::Plain(key)) => Ok(key),
            Some(Sealed::System) => system::load(&self.account),
            Some(Sealed::Passcode { .. }) => bail!("база закрыта код-паролем"),
        }
    }

    /// Открыть сейф кодом.
    pub fn unlock(&self, passcode: &str) -> Result<[u8; 32]> {
        Ok(self.unlock_with_previous(passcode)?.0)
    }

    /// Открыть сейф кодом — вместе с прежним ключом, если база не успела
    /// перейти на новый.
    pub fn unlock_with_previous(&self, passcode: &str) -> Result<([u8; 32], Option<[u8; 32]>)> {
        let Some(Sealed::Passcode {
            salt,
            memory,
            passes,
            lanes,
            sealed,
            previous,
        }) = self.read()?
        else {
            return Ok((self.open_key()?, None));
        };
        let kek = derive(passcode, &salt, memory, passes, lanes)?;
        let key = open_sealed(&kek, &sealed).map_err(|_| anyhow!("неверный код-пароль"))?;
        let previous = previous.map(|raw| open_sealed(&kek, &raw)).transpose()?;
        Ok((key, previous))
    }

    /// Закрыть ключ кодом. Копию в хранилище системы убираем: иначе код
    /// ничего бы не защищал. `previous` — прежний ключ на время перехода базы.
    pub fn set_passcode(
        &self,
        key: &[u8; 32],
        passcode: &str,
        previous: Option<&[u8; 32]>,
    ) -> Result<()> {
        if passcode.chars().count() < MIN_PASSCODE {
            bail!("код-пароль — не короче {MIN_PASSCODE} знаков");
        }
        let salt: [u8; 16] = rand::random();
        let kek = derive(passcode, &salt, KDF_MEMORY_KIB, KDF_PASSES, KDF_LANES)?;
        self.write(&Sealed::Passcode {
            salt,
            memory: KDF_MEMORY_KIB,
            passes: KDF_PASSES,
            lanes: KDF_LANES,
            sealed: seal(&kek, key)?,
            previous: previous.map(|old| seal(&kek, old)).transpose()?,
        })?;
        system::forget(&self.account);
        Ok(())
    }

    /// Убрать код: ключ снова хранится как без него.
    pub fn clear_passcode(&self, key: &[u8; 32]) -> Result<()> {
        self.keep_on_device(key)
    }

    fn keep_on_device(&self, key: &[u8; 32]) -> Result<()> {
        match system::save(&self.account, key) {
            Ok(true) => self.write(&Sealed::System),
            Ok(false) => self.write(&Sealed::Plain(*key)),
            Err(err) => {
                tracing::warn!(%err, "хранилище системы недоступно, ключ базы — в файле");
                self.write(&Sealed::Plain(*key))
            }
        }
    }

    fn read(&self) -> Result<Option<Sealed>> {
        match std::fs::read(&self.path) {
            Ok(raw) => Ok(Some(postcard::from_bytes(&raw).context("сейф повреждён")?)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    /// Запись целиком или никак: оборванная запись сейфа — потерянная база.
    fn write(&self, sealed: &Sealed) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = self.path.with_extension("tmp");
        let _ = std::fs::remove_file(&temp);
        {
            use std::io::Write;
            let mut file = private_file(&temp)?;
            file.write_all(&postcard::to_stdvec(sealed)?)?;
            // На диск — до подмены: после сбоя питания сейф иначе мог бы
            // оказаться пустым, а база без него не откроется уже никогда.
            file.sync_all()?;
        }
        std::fs::rename(&temp, &self.path)?;
        sync_dir(&self.path);
        Ok(())
    }
}

/// Новый файл, который с первого байта читает только владелец. Создать и
/// потом сузить права — значит оставить окно, когда ключ читают все.
fn private_file(path: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

/// Записать на диск сам каталог — иначе переименование может не пережить сбой.
fn sync_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        if let Ok(dir) = std::fs::File::open(dir) {
            let _ = dir.sync_all();
        }
    }
    let _ = path;
}

fn seal(kek: &[u8; 32], key: &[u8; 32]) -> Result<Vec<u8>> {
    let nonce: [u8; 24] = rand::random();
    let body = XChaCha20Poly1305::new(kek.into())
        .encrypt(XNonce::from_slice(&nonce), key.as_slice())
        .map_err(|_| anyhow!("не удалось закрыть ключ"))?;
    let mut sealed = nonce.to_vec();
    sealed.extend_from_slice(&body);
    Ok(sealed)
}

fn open_sealed(kek: &[u8; 32], sealed: &[u8]) -> Result<[u8; 32]> {
    if sealed.len() < 24 {
        bail!("сейф повреждён");
    }
    let (nonce, body) = sealed.split_at(24);
    let plain = XChaCha20Poly1305::new(kek.into())
        .decrypt(XNonce::from_slice(nonce), body)
        .map_err(|_| anyhow!("не удалось открыть ключ"))?;
    plain
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("сейф повреждён"))
}

fn derive(
    passcode: &str,
    salt: &[u8; 16],
    memory: u32,
    passes: u32,
    lanes: u32,
) -> Result<[u8; 32]> {
    let params = argon2::Params::new(memory, passes, lanes, Some(32))
        .map_err(|err| anyhow!("параметры Argon2: {err}"))?;
    let mut out = [0u8; 32];
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password_into(passcode.as_bytes(), salt, &mut out)
        .map_err(|err| anyhow!("Argon2: {err}"))?;
    Ok(out)
}

#[cfg(windows)]
mod system {
    use anyhow::Result;

    const SERVICE: &str = "BRED database key";

    /// `true` — ключ сохранён в диспетчере учётных данных.
    pub fn save(account: &str, key: &[u8; 32]) -> Result<bool> {
        keyring::Entry::new(SERVICE, account)?.set_secret(key)?;
        Ok(true)
    }

    pub fn load(account: &str) -> Result<[u8; 32]> {
        let secret = keyring::Entry::new(SERVICE, account)?.get_secret()?;
        secret
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("ключ базы в хранилище Windows повреждён"))
    }

    pub fn forget(account: &str) {
        if let Ok(entry) = keyring::Entry::new(SERVICE, account) {
            let _ = entry.delete_credential();
        }
    }
}

#[cfg(not(windows))]
mod system {
    use anyhow::{bail, Result};

    pub fn save(_account: &str, _key: &[u8; 32]) -> Result<bool> {
        Ok(false)
    }

    pub fn load(_account: &str) -> Result<[u8; 32]> {
        bail!("ключ базы должен был лежать в хранилище системы, а его там нет")
    }

    pub fn forget(_account: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bred-vault-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn first_start_creates_key_and_reopens_it() {
        let vault = Vault::at(&dir());
        assert_eq!(vault.lock().unwrap(), None);
        let key = vault.open_key().unwrap();
        assert_eq!(vault.lock().unwrap(), Some(Lock::Open));
        assert_eq!(vault.open_key().unwrap(), key);
    }

    #[test]
    fn passcode_locks_and_only_right_code_opens() {
        let vault = Vault::at(&dir());
        let key = vault.open_key().unwrap();
        assert!(
            vault.set_passcode(&key, "12345", None).is_err(),
            "короткий код не принимаем"
        );
        vault.set_passcode(&key, "дятел-на-сосне", None).unwrap();
        assert_eq!(vault.lock().unwrap(), Some(Lock::Passcode));
        assert!(vault.open_key().is_err(), "без кода ключа нет");
        assert!(vault.unlock("дятел-на-ели").is_err());
        assert_eq!(vault.unlock("дятел-на-сосне").unwrap(), key);

        // Ключ в файле не лежит ни в каком виде.
        let raw = std::fs::read(&vault.path).unwrap();
        assert!(!raw.windows(32).any(|w| w == key));

        vault.clear_passcode(&key).unwrap();
        assert_eq!(vault.lock().unwrap(), Some(Lock::Open));
        assert_eq!(vault.open_key().unwrap(), key);
    }

    #[test]
    fn transition_keeps_both_keys_until_done() {
        let vault = Vault::at(&dir());
        let old = vault.open_key().unwrap();
        let new = [9u8; 32];
        vault
            .set_passcode(&new, "дятел-на-сосне", Some(&old))
            .unwrap();
        assert_eq!(
            vault.unlock_with_previous("дятел-на-сосне").unwrap(),
            (new, Some(old))
        );
        vault.set_passcode(&new, "дятел-на-сосне", None).unwrap();
        assert_eq!(
            vault.unlock_with_previous("дятел-на-сосне").unwrap(),
            (new, None)
        );
    }
}
