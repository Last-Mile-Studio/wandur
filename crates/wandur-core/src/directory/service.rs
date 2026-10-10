//! The directory on a background thread: it reads the cached snapshot from disk at start, fetches a
//! fresh one when asked (at most every five minutes unless forced, as the C# client throttles), and
//! saves what it fetched so the directory works offline. The UI reads [`DirectoryStatus`] each frame
//! (a lock and an `Arc` clone) and never waits on the network or the disk.

use crate::l10n::{S, t, tf};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use super::catalog::Catalog;
use super::client::{Fetcher, MAX_SNAPSHOT_BYTES};
use super::snapshot;
use crate::session::Waker;

/// The snapshot file in the data directory.
pub const CACHE_FILE: &str = "directory.json";
/// Fetches closer together than this are skipped unless forced.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// What the UI shows about the directory.
#[derive(Clone, Debug, Default)]
pub struct DirectoryStatus {
    pub catalog: Option<Arc<Catalog>>,
    /// A fetch is running.
    pub loading: bool,
    /// Why the last fetch or the cache read failed, or that the server sent a saved copy.
    pub warning: Option<String>,
    /// Changes whenever the catalog or the flags change.
    pub revision: u64,
    /// The catalog came from this run's fetch (not only the disk cache).
    pub fetched_this_run: bool,
}

enum Command {
    Refresh { force: bool },
}

pub struct DirectoryService {
    tx: Option<Sender<Command>>,
    status: Arc<Mutex<DirectoryStatus>>,
    worker: Option<thread::JoinHandle<()>>,
    base: String,
}

impl DirectoryService {
    /// Start the worker. `cache_dir` is where `directory.json` lives (none: no offline copy).
    pub fn start(base: String, cache_dir: Option<PathBuf>, fetcher: Arc<dyn Fetcher>, waker: Waker) -> Self {
        let status = Arc::new(Mutex::new(DirectoryStatus::default()));
        let (tx, rx) = channel();
        let shared = Arc::clone(&status);
        let url = base.clone();
        let worker = thread::Builder::new()
            .name("wandur-directory".into())
            .spawn(move || worker_main(&url, cache_dir, &*fetcher, &rx, &shared, &waker))
            .ok();
        Self {
            tx: Some(tx),
            status,
            worker,
            base,
        }
    }

    /// The directory's base address.
    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn status(&self) -> DirectoryStatus {
        self.status.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The revision alone (cheaper than [`Self::status`] when nothing changed).
    pub fn revision(&self) -> u64 {
        self.status.lock().unwrap_or_else(PoisonError::into_inner).revision
    }

    /// Ask for a fresh snapshot; skipped if one was fetched within five minutes unless `force`.
    pub fn refresh(&self, force: bool) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Command::Refresh { force });
        }
    }
}

impl Drop for DirectoryService {
    fn drop(&mut self) {
        self.tx.take();
        // A fetch in progress may take a while; the thread ends on its own when it is done.
        drop(self.worker.take());
    }
}

fn update(shared: &Mutex<DirectoryStatus>, waker: &Waker, change: impl FnOnce(&mut DirectoryStatus)) {
    {
        let mut s = shared.lock().unwrap_or_else(PoisonError::into_inner);
        change(&mut s);
        s.revision += 1;
    }
    waker();
}

fn worker_main(
    base: &str,
    cache_dir: Option<PathBuf>,
    fetcher: &dyn Fetcher,
    rx: &Receiver<Command>,
    shared: &Mutex<DirectoryStatus>,
    waker: &Waker,
) {
    let cache = cache_dir.map(|d| d.join(CACHE_FILE));
    if let Some(path) = &cache {
        match std::fs::read_to_string(path) {
            Ok(text) => match snapshot::parse(&text) {
                Ok(s) => {
                    let catalog = Arc::new(Catalog::new(s));
                    update(shared, waker, |st| st.catalog = Some(catalog));
                }
                Err(_) => update(shared, waker, |st| {
                    st.warning = Some(t(S::TheLocalDirectoryCacheCouldNotBeRead).into());
                }),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => update(shared, waker, |st| {
                st.warning = Some(t(S::TheLocalDirectoryCacheCouldNotBeRead).into());
            }),
        }
    }
    let mut last_attempt: Option<Instant> = None;
    while let Ok(Command::Refresh { mut force }) = rx.recv() {
        // Several requests in a row are one fetch.
        while let Ok(Command::Refresh { force: f }) = rx.try_recv() {
            force |= f;
        }
        if !force && last_attempt.is_some_and(|t| t.elapsed() < REFRESH_INTERVAL) {
            continue;
        }
        last_attempt = Some(Instant::now());
        update(shared, waker, |st| {
            st.loading = true;
            st.warning = None;
        });
        let url = format!("{base}directory");
        let result = fetcher.get(&url, MAX_SNAPSHOT_BYTES).and_then(|r| {
            if !(200..300).contains(&r.status) {
                return Err(format!("HTTP {}", r.status));
            }
            let text = String::from_utf8(r.body).map_err(|_| t(S::DirectoryNotUtf8).to_string())?;
            let parsed = snapshot::parse(&text)?;
            Ok((text, parsed, r.stale))
        });
        match result {
            Ok((text, parsed, stale)) => {
                if let Some(path) = &cache {
                    let _ = crate::settings::write_atomic(path, text.as_bytes());
                }
                let catalog = Arc::new(Catalog::new(parsed));
                update(shared, waker, |st| {
                    st.catalog = Some(catalog);
                    st.loading = false;
                    st.fetched_this_run = true;
                    st.warning = stale.then(|| t(S::ShowingTheSavedDirectoryWhileTheServerRefreshesIt).into());
                });
            }
            Err(e) => update(shared, waker, |st| {
                st.loading = false;
                st.warning = Some(tf(S::DirectoryUnavailableAtReason, &[&base, &e]));
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory::client::Fetched;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fake {
        calls: AtomicUsize,
        body: Mutex<Result<String, String>>,
    }

    impl Fetcher for Fake {
        fn get(&self, url: &str, _limit: u64) -> Result<Fetched, String> {
            assert_eq!(url, "http://127.0.0.1:9/directory");
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.body.lock().unwrap().clone().map(|b| Fetched {
                status: 200,
                content_type: Some("application/json".into()),
                body: b.into_bytes(),
                stale: false,
            })
        }
    }

    fn wait(service: &DirectoryService, done: impl Fn(&DirectoryStatus) -> bool) -> DirectoryStatus {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let s = service.status();
            if done(&s) {
                return s;
            }
            assert!(Instant::now() < deadline, "timed out: {s:?}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    const ONE: &str = r#"{"format":"wandur.directory","schema_version":2,"fetched_at":"2026-09-15T20:00:00Z",
        "worlds":[{"id":"a","name":"Alpha","host":"a.example.org","port":4000}]}"#;

    #[test]
    fn fetches_caches_and_reads_the_cache_offline() {
        let dir = std::env::temp_dir().join(format!("wandur-dir-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let fake = Arc::new(Fake {
            calls: AtomicUsize::new(0),
            body: Mutex::new(Ok(ONE.into())),
        });
        let service = DirectoryService::start(
            "http://127.0.0.1:9/".into(),
            Some(dir.clone()),
            fake.clone(),
            Arc::new(|| {}),
        );
        assert!(service.status().catalog.is_none());
        service.refresh(false);
        let s = wait(&service, |s| s.catalog.is_some() && !s.loading);
        assert_eq!(s.catalog.unwrap().worlds[0].name, "Alpha");
        assert!(dir.join(CACHE_FILE).exists());
        // Throttled: a second unforced refresh does not fetch.
        service.refresh(false);
        service.refresh(true);
        wait(&service, |_| fake.calls.load(Ordering::SeqCst) == 2);
        drop(service);

        // Offline: the fetch fails, the cached copy is still served with a warning.
        *fake.body.lock().unwrap() = Err("connection refused".into());
        let service = DirectoryService::start(
            "http://127.0.0.1:9/".into(),
            Some(dir.clone()),
            fake.clone(),
            Arc::new(|| {}),
        );
        let s = wait(&service, |s| s.catalog.is_some());
        assert!(!s.fetched_this_run);
        service.refresh(true);
        let s = wait(&service, |s| s.warning.is_some() && !s.loading);
        assert_eq!(s.catalog.unwrap().len(), 1);
        assert!(s.warning.unwrap().contains("unavailable"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_cache_is_reported_not_fatal() {
        let dir = std::env::temp_dir().join(format!("wandur-dir-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(CACHE_FILE), "{ nope").unwrap();
        let fake = Arc::new(Fake {
            calls: AtomicUsize::new(0),
            body: Mutex::new(Ok(ONE.into())),
        });
        let service = DirectoryService::start("http://127.0.0.1:9/".into(), Some(dir.clone()), fake, Arc::new(|| {}));
        let s = wait(&service, |s| s.warning.is_some());
        assert!(s.catalog.is_none());
        service.refresh(false);
        wait(&service, |s| s.catalog.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
