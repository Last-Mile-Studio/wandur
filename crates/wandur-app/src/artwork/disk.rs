//! A small disk cache of resized thumbnails (JPEG), so a picture seen before is not downloaded and
//! decoded from full size again. Bounded in bytes: the oldest files (by modification time, which a
//! read refreshes) are removed when the cache opens and after writes push it over its limit.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use image::RgbaImage;
use wandur_core::directory::listing::fnv1a;

use super::decode::encode_thumbnail;

/// The default limit: 64 MB.
pub const DEFAULT_LIMIT: u64 = 64 * 1024 * 1024;

pub struct ThumbCache {
    dir: PathBuf,
    limit: u64,
    /// Bytes written since the last trim (approximate, to decide when to trim again).
    written: AtomicU64,
    trimming: Mutex<()>,
}

impl ThumbCache {
    /// Open (and create) the cache directory. It is trimmed to `limit` bytes by the first artwork
    /// worker, not here: listing a full cache took 10 to 14 ms on the UI thread at start.
    pub fn open(dir: PathBuf, limit: u64) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        Self {
            dir,
            limit,
            written: AtomicU64::new(0),
            trimming: Mutex::new(()),
        }
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{:016x}.jpg", fnv1a(key.as_bytes())))
    }

    pub fn read(&self, key: &str) -> Option<Vec<u8>> {
        let path = self.path(key);
        let bytes = std::fs::read(&path).ok()?;
        // Mark it recently used.
        if let Ok(f) = std::fs::File::options().write(true).open(&path) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
        Some(bytes)
    }

    pub fn write(&self, key: &str, image: &RgbaImage) {
        let Ok(bytes) = encode_thumbnail(image) else { return };
        if wandur_core::settings::write_atomic(&self.path(key), &bytes).is_ok() {
            let total = self.written.fetch_add(bytes.len() as u64, Ordering::Relaxed) + bytes.len() as u64;
            if total > self.limit / 8 {
                self.written.store(0, Ordering::Relaxed);
                self.trim();
            }
        }
    }

    /// Remove the oldest files until the cache fits its limit.
    pub fn trim(&self) {
        let Ok(_guard) = self.trimming.try_lock() else { return };
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                meta.is_file()
                    .then(|| (meta.modified().unwrap_or(std::time::UNIX_EPOCH), meta.len(), e.path()))
            })
            .collect();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        files.sort_by_key(|f| f.0);
        for (_, len, path) in files {
            if total <= self.limit {
                break;
            }
            if std::fs::remove_file(&path).is_ok() {
                total -= len;
            }
        }
    }

    /// Bytes in the cache now.
    pub fn size(&self) -> u64 {
        std::fs::read_dir(&self.dir)
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter_map(|e| e.metadata().ok())
                    .map(|m| m.len())
                    .sum()
            })
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_within_its_limit() {
        let dir = std::env::temp_dir().join(format!("wandur-thumb-limit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = ThumbCache::open(dir.clone(), 40_000);
        let img = crate::artwork::decode::tests::picture(200, 120);
        for i in 0..20 {
            cache.write(&format!("k{i}"), &img);
        }
        cache.trim();
        assert!(cache.size() <= 40_000, "{}", cache.size());
        assert!(cache.read("k19").is_some(), "the newest survives");
        assert!(cache.read("k0").is_none(), "the oldest went");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
