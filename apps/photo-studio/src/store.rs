//! Small storage helpers: ids, timestamps, atomic writes, size-capped reads, random tokens.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)).unwrap_or_default()
}

/// `n` random bytes from the thread-local CSPRNG.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    rand::rng().fill_bytes(&mut buf);
    buf
}

/// A new object id: 128 random bits as lowercase hex (case-insensitive file systems can't collide).
pub fn new_id() -> String {
    hex(&random_bytes::<16>())
}

/// A random 256-bit token, base64url without padding.
pub fn new_token() -> String {
    b64url(&random_bytes::<32>())
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    s
}

pub fn b64url(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Constant-time comparison of two equal-length digests.
pub fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Constant-time comparison of two strings (the length may leak; contents don't).
pub fn ct_eq_str(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Ids accepted in paths and file names: 1–64 of `[A-Za-z0-9_-]`. This is also what keeps file
/// paths inside the data directory.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Lock a std mutex, recovering the data if a previous holder panicked (the data is still
/// consistent for our uses: every critical section is a few plain field updates).
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Write `bytes` to `path` atomically (temporary file in the same directory, fsync, rename).
/// `private` makes the file readable by the owner only (0600 on Unix).
pub fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> Result<(), String> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().ok_or_else(|| format!("{}: no parent directory", path.display()))?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(".{name}.tmp-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
    let write = || -> std::io::Result<()> {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        #[cfg(not(unix))]
        let _ = private; // no POSIX modes here; the data directory's ACLs apply
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    };
    write().map_err(|e| {
        // Best effort: don't leave a half-written temporary file behind.
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })?;
    // Make the rename durable too (not supported everywhere; the data is already written).
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// [`write_atomic`] on the blocking pool.
pub async fn write_atomic_async(path: PathBuf, bytes: Vec<u8>, private: bool) -> Result<(), String> {
    tokio::task::spawn_blocking(move || write_atomic(&path, &bytes, private)).await.map_err(|e| format!("write task: {e}"))?
}

/// Read `path` if it exists and is at most `max` bytes. `Ok(None)` when it doesn't exist.
pub fn read_capped(path: &Path, max: u64) -> Result<Option<Vec<u8>>, String> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if !meta.is_file() {
        return Err(format!("{}: not a file", path.display()));
    }
    if meta.len() > max {
        return Err(format!("{}: {} bytes is over the {max}-byte limit", path.display(), meta.len()));
    }
    std::fs::read(path).map(Some).map_err(|e| format!("{}: {e}", path.display()))
}

/// The `*.json` files of `dir` whose stem is a valid id, at most `max` of them.
pub fn json_ids(dir: &Path, max: usize) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut out = vec![];
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(id) = name.strip_suffix(".json") else { continue };
        if valid_id(id) && entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            out.push(id.to_string());
            if out.len() >= max {
                log::warn!("{}: more than {max} entries; the rest are ignored", dir.display());
                break;
            }
        }
    }
    out
}

/// Create `dir` (and parents); on Unix with `private`, restrict it to the owner.
pub fn ensure_dir(dir: &Path, private: bool) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    #[cfg(not(unix))]
    let _ = private; // no POSIX modes here
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_tokens() {
        let id = new_id();
        assert_eq!(id.len(), 32);
        assert!(valid_id(&id));
        assert_ne!(new_id(), id);
        assert_eq!(new_token().len(), 43);
        assert!(!valid_id(""));
        assert!(!valid_id("../x"));
        assert!(!valid_id(&"a".repeat(65)));
        assert_eq!(hex(&[0, 0xab, 0xff]), "00abff");
    }

    #[test]
    fn constant_time_eq() {
        let a = sha256(b"one");
        assert!(ct_eq(&a, &sha256(b"one")));
        assert!(!ct_eq(&a, &sha256(b"two")));
    }

    #[test]
    fn atomic_write_and_capped_read() {
        let dir = std::env::temp_dir().join(format!("ps-store-{}", new_id()));
        ensure_dir(&dir, true).unwrap();
        let p = dir.join("x.json");
        write_atomic(&p, b"{}", true).unwrap();
        assert_eq!(read_capped(&p, 10).unwrap().unwrap(), b"{}");
        assert!(read_capped(&p, 1).is_err());
        assert!(read_capped(&dir.join("missing"), 10).unwrap().is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(json_ids(&dir, 10), vec!["x".to_string()]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
