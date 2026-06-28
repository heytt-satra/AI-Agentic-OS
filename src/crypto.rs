// ── src/crypto.rs : at-rest encryption for sensitive local data ─────────────
//
// Pure-Rust AES-256-GCM (no OpenSSL, no SQLCipher) so we keep the zero-install,
// pure-Rust rule. Used to encrypt the scariest data Jarvis stores - the activity
// log (window titles + clipboard) - before it touches disk.
//
// Design choices that keep it SAFE (no migration, no data loss):
//   - The key lives in a local key-file (~/.jarvis-key), generated once. This
//     protects against backup/sync/cloud leakage and casual disk access. A
//     determined attacker with full disk access still needs the key-file; a
//     passphrase / OS-keystore option is a later upgrade.
//   - Encrypted values are stored as "enc:<base64(nonce||ciphertext)>". Anything
//     WITHOUT that prefix is treated as legacy plaintext and returned as-is, so
//     old rows keep working and nothing has to be migrated.

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine as _;
use std::sync::OnceLock;

static KEY: OnceLock<[u8; 32]> = OnceLock::new();

fn key_path() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from(".")).join(".jarvis-key")
}

fn key() -> &'static [u8; 32] {
    KEY.get_or_init(|| {
        let path = key_path();
        if let Ok(b) = std::fs::read(&path) {
            if b.len() == 32 {
                let mut k = [0u8; 32];
                k.copy_from_slice(&b);
                return k;
            }
        }
        // First run: generate a key and persist it.
        let k = Aes256Gcm::generate_key(OsRng);
        let _ = std::fs::write(&path, k.as_slice());
        let mut arr = [0u8; 32];
        arr.copy_from_slice(k.as_slice());
        arr
    })
}

// Encrypt a string for storage. Returns "enc:<base64>" or, if encryption fails,
// the original (never lose data over a crypto hiccup).
pub fn encrypt(plain: &str) -> String {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key()));
    let nonce = Aes256Gcm::generate_nonce(OsRng); // 96-bit, unique per message
    match cipher.encrypt(&nonce, plain.as_bytes()) {
        Ok(ct) => {
            let mut blob = nonce.to_vec();
            blob.extend_from_slice(&ct);
            format!("enc:{}", base64::engine::general_purpose::STANDARD.encode(blob))
        }
        Err(_) => plain.to_string(),
    }
}

// Decrypt a stored value. Legacy plaintext (no "enc:" prefix) is returned as-is.
pub fn decrypt(stored: &str) -> String {
    let Some(b64) = stored.strip_prefix("enc:") else { return stored.to_string() };
    let Ok(blob) = base64::engine::general_purpose::STANDARD.decode(b64) else { return stored.to_string() };
    if blob.len() < 12 {
        return stored.to_string();
    }
    let (nonce, ct) = blob.split_at(12);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key()));
    match cipher.decrypt(Nonce::from_slice(nonce), ct) {
        Ok(pt) => String::from_utf8_lossy(&pt).to_string(),
        Err(_) => stored.to_string(),
    }
}
