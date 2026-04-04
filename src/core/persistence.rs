use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, KeyInit},
};
use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::Path;

const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

/// Encrypt plaintext with AES-256-GCM. Returns nonce || ciphertext.
pub fn encrypt(plaintext: &[u8], key: &[u8; KEY_LEN]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::fill(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| anyhow::anyhow!("encryption failed: {e}"))?;

    let mut output = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

/// Decrypt data produced by `encrypt`. Expects nonce || ciphertext.
pub fn decrypt(data: &[u8], key: &[u8; KEY_LEN]) -> Result<Vec<u8>> {
    if data.len() < NONCE_LEN {
        anyhow::bail!("ciphertext too short");
    }
    let (nonce_bytes, ciphertext) = data.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("decryption failed (wrong key or corrupted data): {e}"))
}

/// Read or create a 256-bit encryption key in `data_dir/.session-key`.
pub fn get_or_create_key(data_dir: &Path) -> Result<[u8; KEY_LEN]> {
    let key_path = data_dir.join(".session-key");

    if key_path.exists() {
        let raw = fs::read(&key_path)
            .with_context(|| format!("reading session key: {}", key_path.display()))?;
        let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &raw)
            .context("decoding session key")?;
        let key: [u8; KEY_LEN] = decoded
            .try_into()
            .map_err(|_| anyhow::anyhow!("session key has wrong length"))?;
        return Ok(key);
    }

    // Generate new key
    let mut key = [0u8; KEY_LEN];
    rand::fill(&mut key);

    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, key);
    atomic_write(&key_path, encoded.as_bytes())?;

    Ok(key)
}

/// Atomically write data to a file (write tmp, fsync, rename).
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("creating directory: {}", parent.display()))?;

    let tmp_path = path.with_extension("tmp");
    let mut file = fs::File::create(&tmp_path)
        .with_context(|| format!("creating temp file: {}", tmp_path.display()))?;
    file.write_all(data).context("writing temp file")?;
    file.sync_all().context("fsync temp file")?;
    drop(file);

    fs::rename(&tmp_path, path)
        .with_context(|| format!("renaming {} -> {}", tmp_path.display(), path.display()))?;

    set_permissions_600(path)?;
    Ok(())
}

#[cfg(unix)]
fn set_permissions_600(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("setting permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_permissions_600(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = [42u8; KEY_LEN];
        let plaintext = b"hello browser39 session data";
        let encrypted = encrypt(plaintext, &key).unwrap();
        assert_ne!(&encrypted, plaintext);
        let decrypted = decrypt(&encrypted, &key).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_wrong_key() {
        let key1 = [1u8; KEY_LEN];
        let key2 = [2u8; KEY_LEN];
        let encrypted = encrypt(b"secret", &key1).unwrap();
        assert!(decrypt(&encrypted, &key2).is_err());
    }

    #[test]
    fn test_decrypt_too_short() {
        let key = [0u8; KEY_LEN];
        assert!(decrypt(&[0u8; 5], &key).is_err());
    }

    #[test]
    fn test_get_or_create_key() {
        let dir = tempfile::tempdir().unwrap();
        let key1 = get_or_create_key(dir.path()).unwrap();
        let key2 = get_or_create_key(dir.path()).unwrap();
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_atomic_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.enc");
        atomic_write(&path, b"test data").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"test data");
    }
}
