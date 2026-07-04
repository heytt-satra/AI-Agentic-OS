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

/// A cryptographically strong random password from the OS RNG. Uses an
/// unambiguous character set (no 0/O/1/l/I) plus optional symbols. Length is
/// clamped to [8, 128].
pub fn random_password(len: usize, symbols: bool) -> String {
    use aes_gcm::aead::rand_core::RngCore;
    let mut charset: Vec<u8> = Vec::new();
    charset.extend_from_slice(b"ABCDEFGHJKLMNPQRSTUVWXYZ"); // no I, O
    charset.extend_from_slice(b"abcdefghijkmnpqrstuvwxyz"); // no l
    charset.extend_from_slice(b"23456789"); // no 0, 1
    if symbols {
        charset.extend_from_slice(b"!@#$%^&*-_=+?");
    }
    let len = len.clamp(8, 128);
    let mut buf = vec![0u8; len];
    OsRng.fill_bytes(&mut buf);
    buf.into_iter().map(|b| charset[b as usize % charset.len()] as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_length_and_charset() {
        let p = random_password(24, true);
        assert_eq!(p.chars().count(), 24);
        // never contains the ambiguous characters we excluded
        assert!(!p.contains(['0', 'O', '1', 'l', 'I']));
        // length clamps
        assert_eq!(random_password(2, false).chars().count(), 8);
        assert_eq!(random_password(9999, false).chars().count(), 128);
        // no-symbols variant is alphanumeric only
        assert!(random_password(40, false).chars().all(|c| c.is_ascii_alphanumeric()));
        // two draws differ (astronomically unlikely to collide)
        assert_ne!(random_password(20, true), random_password(20, true));
    }

    #[test]
    fn roundtrip() {
        let enc = encrypt("a secret clipboard value");
        assert!(enc.starts_with("enc:"));
        assert_eq!(decrypt(&enc), "a secret clipboard value");
    }

    #[test]
    fn legacy_plaintext_passthrough() {
        // Old rows have no "enc:" prefix and must come back unchanged.
        assert_eq!(decrypt("just plain text"), "just plain text");
    }

    #[test]
    fn corrupt_ciphertext_is_safe() {
        // Garbage after enc: must not panic; returns the input.
        assert_eq!(decrypt("enc:notbase64!!"), "enc:notbase64!!");
    }
}
