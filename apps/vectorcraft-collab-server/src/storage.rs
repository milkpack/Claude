//! Rooms on disk: `<data dir>/<room>.ydoc` holds the whole document as one Yjs v1 update
//! (`encode_state_as_update_v1`). Writes are atomic (temporary file, fsync, rename).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

/// Extension of stored rooms.
pub const EXT: &str = "ydoc";
/// Refuse to load room files larger than this (a document is far smaller; protects memory).
pub const MAX_FILE_BYTES: u64 = 1 << 30;
/// List at most this many stored rooms.
const MAX_LISTED: usize = 10_000;

/// Room ids: 1–64 characters from `[A-Za-z0-9_-]`. They become file names, so this is also what
/// keeps paths inside the data directory.
pub fn valid_room(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub fn room_path(dir: &Path, room: &str) -> PathBuf {
    dir.join(format!("{room}.{EXT}"))
}

/// The stored update of `room`, `None` when the room was never saved.
pub fn load(dir: &Path, room: &str) -> Result<Option<Vec<u8>>, String> {
    let path = room_path(dir, room);
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if !meta.is_file() {
        return Err(format!("{}: not a file", path.display()));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("{}: {} bytes is over the {MAX_FILE_BYTES}-byte limit", path.display(), meta.len()));
    }
    std::fs::read(&path).map(Some).map_err(|e| format!("{}: {e}", path.display()))
}

/// Write `bytes` as the stored state of `room`, atomically.
pub fn save(dir: &Path, room: &str, bytes: &[u8]) -> Result<(), String> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let path = room_path(dir, room);
    let tmp = dir.join(format!(".{room}.{EXT}.tmp-{}-{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &path)
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

/// A stored room file that can't be read as a document is moved aside (never overwritten), so an
/// operator can inspect it. Returns where it went.
pub fn quarantine(dir: &Path, room: &str) -> Result<PathBuf, String> {
    let path = room_path(dir, room);
    let stamp = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default();
    let to = dir.join(format!("{room}.{EXT}.corrupt-{stamp}"));
    std::fs::rename(&path, &to).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(to)
}

/// One stored room.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredRoom {
    pub id: String,
    pub bytes: u64,
    /// Last write, milliseconds since the Unix epoch.
    pub updated_at: Option<u64>,
}

/// The rooms stored in `dir`, newest first.
pub fn list(dir: &Path) -> Result<Vec<StoredRoom>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    let mut out = vec![];
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(id) = name.strip_suffix(&format!(".{EXT}")) else { continue };
        if !valid_room(id) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let updated_at =
            meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
        out.push(StoredRoom { id: id.to_string(), bytes: meta.len(), updated_at });
        if out.len() >= MAX_LISTED {
            break;
        }
    }
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then_with(|| a.id.cmp(&b.id)));
    Ok(out)
}
