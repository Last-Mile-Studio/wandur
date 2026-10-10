//! Artwork: bounded, cancellable loading of world pictures into GPU textures, off the UI thread.
//!
//! - **Requests** come from what is on screen this frame ([`ArtLoader::request`]); a picture not
//!   asked for again by the end of the frame is cancelled ([`ArtLoader::end_frame`]): a queued job
//!   is dropped before it fetches or decodes, a running one stops at its next step and its result
//!   is never uploaded. The newest request runs first (cards just scrolled into view).
//! - **Workers** (a small fixed pool) look in the disk cache of resized thumbnails, else fetch the
//!   bytes, decode near the target size ([`decode`]), save the thumbnail, and hand back pixels.
//! - **Textures** live in an LRU with a byte budget ([`TextureLru`]). Evicting drops the egui
//!   `TextureHandle`, which frees the GPU texture at the end of that frame: release is
//!   deterministic, never left to a collector. Textures used in the current frame are not evicted.
//!
//! Nothing here runs on the UI thread except taking finished pixels and creating textures.

pub mod decode;
pub mod disk;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;

use egui::{ColorImage, TextureHandle, TextureOptions};
use wandur_core::Waker;

pub use decode::{Decoded, Target};
pub use disk::ThumbCache;

/// Fetch the bytes at a URL (the directory's HTTP client in the app, a fake in tests).
pub type FetchArt = Arc<dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync>;

/// One picture at one size.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ArtRequest {
    /// Identity of the picture at this size (the cache key).
    pub key: String,
    /// Where to fetch it; the fallback is tried when the first fails.
    pub url: String,
    pub fallback: Option<String>,
    pub target: Target,
}

/// What a view can draw now.
#[derive(Clone, Debug, PartialEq)]
pub enum ArtState {
    Ready { texture: egui::TextureId, size: egui::Vec2 },
    Loading,
    Failed,
}

/// Counters, for tests and measurements.
#[derive(Debug, Default)]
pub struct ArtStats {
    pub fetched: AtomicU64,
    pub from_disk: AtomicU64,
    pub decoded: AtomicU64,
    /// Jobs cancelled before a worker took them.
    pub cancelled_queued: AtomicU64,
    /// Jobs cancelled after fetching, before decoding.
    pub cancelled_running: AtomicU64,
    /// Finished pixels thrown away because the picture was no longer wanted.
    pub dropped: AtomicU64,
    pub uploaded: AtomicU64,
    pub evicted: AtomicU64,
    pub failed: AtomicU64,
}

impl ArtStats {
    pub fn get(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }
}

struct Job {
    request: ArtRequest,
    cancelled: AtomicBool,
}

#[derive(Default)]
struct Queue {
    /// A stack: the newest request is taken first.
    jobs: Vec<Arc<Job>>,
    shutdown: bool,
}

struct Done {
    job: Arc<Job>,
    result: Result<Decoded, String>,
}

/// Decoded textures by key, bounded by a byte budget.
pub struct TextureLru {
    entries: HashMap<String, LruEntry>,
    bytes: usize,
    budget: usize,
}

struct LruEntry {
    handle: TextureHandle,
    bytes: usize,
    last_used: u64,
}

impl TextureLru {
    pub fn new(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            budget,
        }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    fn get(&mut self, key: &str, frame: u64) -> Option<&TextureHandle> {
        let e = self.entries.get_mut(key)?;
        e.last_used = frame;
        Some(&e.handle)
    }

    fn insert(&mut self, key: String, handle: TextureHandle, bytes: usize, frame: u64) {
        if let Some(old) = self.entries.insert(
            key,
            LruEntry {
                handle,
                bytes,
                last_used: frame,
            },
        ) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
    }

    /// Drop least recently used textures until the total fits the budget, sparing those used in
    /// `frame`. Returns how many were dropped.
    fn evict(&mut self, frame: u64) -> u64 {
        let mut dropped = 0;
        while self.bytes > self.budget {
            let oldest = self
                .entries
                .iter()
                .filter(|(_, e)| e.last_used < frame)
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone());
            let Some(key) = oldest else { break };
            if let Some(e) = self.entries.remove(&key) {
                self.bytes -= e.bytes;
                dropped += 1;
            }
        }
        dropped
    }

    /// Drop everything (the handles free their textures).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
}

pub struct ArtLoader {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    done: Receiver<Done>,
    workers: Vec<thread::JoinHandle<()>>,
    pending: HashMap<String, Arc<Job>>,
    wanted: HashSet<String>,
    failed: HashSet<String>,
    lru: TextureLru,
    frame: u64,
    pub stats: Arc<ArtStats>,
}

/// Textures kept by default: 24 MB, about twenty-four 800 by 320 plates (five are on screen at
/// once). Textures live in GPU memory, which counts in the process footprint; a 96 MB budget
/// measured 366 MB of footprint in the directory scenario against 297 MB for the C# client.
pub const DEFAULT_BUDGET: usize = 24 * 1024 * 1024;
/// Workers (the C# client fetches four pictures at a time).
pub const DEFAULT_WORKERS: usize = 4;

impl ArtLoader {
    pub fn new(workers: usize, fetch: FetchArt, disk: Option<ThumbCache>, budget: usize, waker: Waker) -> Self {
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let (tx, done) = channel();
        let stats = Arc::new(ArtStats::default());
        let disk = disk.map(Arc::new);
        let workers = (0..workers.max(1))
            .filter_map(|i| {
                let queue = Arc::clone(&queue);
                let tx = tx.clone();
                let fetch = Arc::clone(&fetch);
                let stats = Arc::clone(&stats);
                let disk = disk.clone();
                let waker = Arc::clone(&waker);
                thread::Builder::new()
                    .name(format!("wandur-art-{i}"))
                    .spawn(move || {
                        // One worker trims the thumbnail cache to its limit before taking jobs.
                        if i == 0
                            && let Some(disk) = disk.as_deref()
                        {
                            disk.trim();
                        }
                        worker(&queue, &tx, &*fetch, disk.as_deref(), &stats, &waker)
                    })
                    .ok()
            })
            .collect();
        Self {
            queue,
            done,
            workers,
            pending: HashMap::new(),
            wanted: HashSet::new(),
            failed: HashSet::new(),
            lru: TextureLru::new(budget),
            frame: 0,
            stats,
        }
    }

    /// Ask for a picture this frame.
    pub fn request(&mut self, request: &ArtRequest) -> ArtState {
        self.wanted.insert(request.key.clone());
        if let Some(handle) = self.lru.get(&request.key, self.frame) {
            let size = handle.size_vec2();
            return ArtState::Ready {
                texture: handle.id(),
                size,
            };
        }
        if self.failed.contains(&request.key) {
            return ArtState::Failed;
        }
        if !self.pending.contains_key(&request.key) {
            let job = Arc::new(Job {
                request: request.clone(),
                cancelled: AtomicBool::new(false),
            });
            self.pending.insert(request.key.clone(), Arc::clone(&job));
            let (lock, ready) = &*self.queue;
            lock.lock().unwrap_or_else(PoisonError::into_inner).jobs.push(job);
            ready.notify_one();
        }
        ArtState::Loading
    }

    /// Take finished pictures: upload those still wanted, drop the rest. Call at the start of a
    /// frame, before views request.
    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        while let Ok(done) = self.done.try_recv() {
            let key = &done.job.request.key;
            // A newer job for the same key may have replaced this one.
            let current = self.pending.get(key).is_some_and(|j| Arc::ptr_eq(j, &done.job));
            if current {
                self.pending.remove(key);
            }
            if done.job.cancelled.load(Ordering::Relaxed) || !current {
                if done.result.is_ok() {
                    self.stats.dropped.fetch_add(1, Ordering::Relaxed);
                }
                continue;
            }
            match done.result {
                Ok(decoded) => {
                    let (w, h) = decoded.image.dimensions();
                    let image = ColorImage::from_rgba_unmultiplied([w as usize, h as usize], decoded.image.as_raw());
                    let handle = ctx.load_texture(format!("art:{key}"), image, TextureOptions::LINEAR);
                    self.lru.insert(key.clone(), handle, (w * h * 4) as usize, self.frame);
                    self.stats.uploaded.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    self.failed.insert(key.clone());
                    self.stats.failed.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    /// Cancel what was not asked for this frame and keep the textures within budget.
    pub fn end_frame(&mut self) {
        let wanted = std::mem::take(&mut self.wanted);
        let mut cancelled = Vec::new();
        self.pending.retain(|key, job| {
            let keep = wanted.contains(key);
            if !keep {
                job.cancelled.store(true, Ordering::Relaxed);
                cancelled.push(Arc::clone(job));
            }
            keep
        });
        if !cancelled.is_empty() {
            let (lock, _) = &*self.queue;
            let mut q = lock.lock().unwrap_or_else(PoisonError::into_inner);
            let before = q.jobs.len();
            q.jobs.retain(|j| !j.cancelled.load(Ordering::Relaxed));
            let removed = (before - q.jobs.len()) as u64;
            self.stats.cancelled_queued.fetch_add(removed, Ordering::Relaxed);
        }
        let evicted = self.lru.evict(self.frame);
        self.stats.evicted.fetch_add(evicted, Ordering::Relaxed);
        self.wanted = wanted;
        self.wanted.clear();
    }

    /// Try failed pictures again (after a directory refresh).
    pub fn forget_failures(&mut self) {
        self.failed.clear();
    }

    /// Drop every texture now (the theme or the window changed).
    pub fn clear_textures(&mut self) {
        self.lru.clear();
    }

    pub fn textures(&self) -> &TextureLru {
        &self.lru
    }

    /// Jobs waiting or running.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

impl Drop for ArtLoader {
    fn drop(&mut self) {
        {
            let (lock, ready) = &*self.queue;
            let mut q = lock.lock().unwrap_or_else(PoisonError::into_inner);
            q.shutdown = true;
            for job in q.jobs.drain(..) {
                job.cancelled.store(true, Ordering::Relaxed);
            }
            ready.notify_all();
        }
        for job in self.pending.values() {
            job.cancelled.store(true, Ordering::Relaxed);
        }
        // A worker in the middle of a download finishes it on its own; do not wait for it.
        self.workers.clear();
    }
}

fn worker(
    queue: &(Mutex<Queue>, Condvar),
    tx: &Sender<Done>,
    fetch: &(dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync),
    disk: Option<&ThumbCache>,
    stats: &ArtStats,
    waker: &Waker,
) {
    loop {
        let job = {
            let (lock, ready) = queue;
            let mut q = lock.lock().unwrap_or_else(PoisonError::into_inner);
            loop {
                if q.shutdown {
                    return;
                }
                if let Some(job) = q.jobs.pop() {
                    break job;
                }
                q = ready.wait(q).unwrap_or_else(PoisonError::into_inner);
            }
        };
        if job.cancelled.load(Ordering::Relaxed) {
            stats.cancelled_queued.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let result = load(&job, fetch, disk, stats);
        let Some(result) = result else {
            stats.cancelled_running.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        if tx.send(Done { job, result }).is_err() {
            return;
        }
        waker();
    }
}

/// Fetch (or read from the disk cache) and decode one picture. `None` when cancelled midway.
fn load(
    job: &Job,
    fetch: &(dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync),
    disk: Option<&ThumbCache>,
    stats: &ArtStats,
) -> Option<Result<Decoded, String>> {
    let request = &job.request;
    if let Some(bytes) = disk.and_then(|d| d.read(&request.key)) {
        stats.from_disk.fetch_add(1, Ordering::Relaxed);
        if job.cancelled.load(Ordering::Relaxed) {
            return None;
        }
        if let Ok(decoded) = decode::decode_near(&bytes, request.target) {
            stats.decoded.fetch_add(1, Ordering::Relaxed);
            return Some(Ok(decoded));
        }
    }
    let mut bytes = fetch(&request.url);
    if bytes.is_err()
        && let Some(fallback) = &request.fallback
    {
        if job.cancelled.load(Ordering::Relaxed) {
            return None;
        }
        bytes = fetch(fallback);
    }
    let bytes = match bytes {
        Ok(b) => b,
        Err(e) => return Some(Err(e)),
    };
    stats.fetched.fetch_add(1, Ordering::Relaxed);
    if job.cancelled.load(Ordering::Relaxed) {
        return None;
    }
    let decoded = decode::decode_near(&bytes, request.target);
    drop(bytes);
    if decoded.is_ok() {
        stats.decoded.fetch_add(1, Ordering::Relaxed);
    }
    if let (Ok(d), Some(disk)) = (&decoded, disk)
        && !job.cancelled.load(Ordering::Relaxed)
    {
        disk.write(&request.key, &d.image);
    }
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    fn request(key: &str) -> ArtRequest {
        ArtRequest {
            key: key.into(),
            url: format!("http://127.0.0.1:9/{key}"),
            fallback: None,
            target: Target {
                width: 64,
                height: 32,
                cover: true,
            },
        }
    }

    fn wait_until(mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "timed out");
            thread::sleep(Duration::from_millis(2));
        }
    }

    /// A fetcher that holds every request until the gate opens, and counts them.
    fn gated(gate: Arc<(Mutex<bool>, Condvar)>, calls: Arc<AtomicUsize>) -> FetchArt {
        let bytes = decode::tests::jpeg(256, 128);
        Arc::new(move |_url: &str| {
            calls.fetch_add(1, Ordering::SeqCst);
            let (lock, cv) = &*gate;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = cv.wait(open).unwrap();
            }
            Ok(bytes.clone())
        })
    }

    #[test]
    fn wanted_pictures_are_decoded_and_uploaded() {
        let ctx = egui::Context::default();
        let gate = Arc::new((Mutex::new(true), Condvar::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut art = ArtLoader::new(2, gated(gate, calls), None, DEFAULT_BUDGET, Arc::new(|| {}));
        let r = request("a");
        assert_eq!(art.request(&r), ArtState::Loading);
        art.end_frame();
        wait_until(|| {
            art.begin_frame(&ctx);
            let state = art.request(&r);
            art.end_frame();
            matches!(state, ArtState::Ready { .. })
        });
        match art.request(&r) {
            ArtState::Ready { size, .. } => assert_eq!(size, egui::vec2(64.0, 32.0)),
            other => panic!("{other:?}"),
        }
        assert_eq!(ArtStats::get(&art.stats.uploaded), 1);
        assert_eq!(art.textures().bytes(), 64 * 32 * 4);
    }

    #[test]
    fn cancelled_jobs_do_not_decode_or_upload() {
        let ctx = egui::Context::default();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        // One worker: the first job blocks in the fetch, the others wait in the queue.
        let mut art = ArtLoader::new(
            1,
            gated(Arc::clone(&gate), Arc::clone(&calls)),
            None,
            DEFAULT_BUDGET,
            Arc::new(|| {}),
        );
        for key in ["a", "b", "c", "d"] {
            art.request(&request(key));
        }
        wait_until(|| calls.load(Ordering::SeqCst) == 1);
        // The cards scrolled away: nothing is asked for in the next frame.
        art.end_frame();
        art.begin_frame(&ctx);
        art.end_frame();
        assert_eq!(art.pending(), 0);
        assert_eq!(ArtStats::get(&art.stats.cancelled_queued), 3, "three never started");
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        wait_until(|| ArtStats::get(&art.stats.cancelled_running) == 1);
        for _ in 0..5 {
            art.begin_frame(&ctx);
            art.end_frame();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "only the running job fetched");
        assert_eq!(ArtStats::get(&art.stats.decoded), 0, "nothing was decoded");
        assert_eq!(ArtStats::get(&art.stats.uploaded), 0, "nothing was uploaded");
        assert!(art.textures().is_empty());
    }

    #[test]
    fn lru_respects_its_budget_and_spares_what_is_on_screen() {
        let ctx = egui::Context::default();
        let gate = Arc::new((Mutex::new(true), Condvar::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        // Room for three 64x32 textures.
        let budget = 3 * 64 * 32 * 4;
        let mut art = ArtLoader::new(2, gated(gate, calls), None, budget, Arc::new(|| {}));
        let keys: Vec<String> = (0..6).map(|i| format!("k{i}")).collect();
        for key in &keys {
            // Each picture is on screen until it is ready, then scrolls away.
            let r = request(key);
            wait_until(|| {
                art.begin_frame(&ctx);
                let ready = matches!(art.request(&r), ArtState::Ready { .. });
                art.end_frame();
                assert!(art.textures().bytes() <= budget, "over budget");
                ready
            });
        }
        assert_eq!(art.textures().len(), 3);
        assert_eq!(ArtStats::get(&art.stats.evicted), 3);
        // The three newest stay; the oldest were released.
        art.begin_frame(&ctx);
        assert!(matches!(art.request(&request("k5")), ArtState::Ready { .. }));
        assert_eq!(art.request(&request("k0")), ArtState::Loading);
        // Everything on screen at once is kept even over budget (no thrashing within a frame).
        art.end_frame();
        let big = ArtLoader::new(1, Arc::new(|_: &str| Err("x".into())), None, 0, Arc::new(|| {}));
        assert_eq!(big.textures().budget(), 0);
    }

    #[test]
    fn failures_are_remembered_until_forgotten() {
        let ctx = egui::Context::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&calls);
        let fetch: FetchArt = Arc::new(move |_: &str| {
            c.fetch_add(1, Ordering::SeqCst);
            Err("404".into())
        });
        let mut art = ArtLoader::new(1, fetch, None, DEFAULT_BUDGET, Arc::new(|| {}));
        let r = request("x");
        wait_until(|| {
            art.begin_frame(&ctx);
            let failed = art.request(&r) == ArtState::Failed;
            art.end_frame();
            failed
        });
        for _ in 0..3 {
            art.begin_frame(&ctx);
            assert_eq!(art.request(&r), ArtState::Failed);
            art.end_frame();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no retry loop");
        art.forget_failures();
        assert_eq!(art.request(&r), ArtState::Loading);
    }

    #[test]
    fn the_disk_cache_serves_resized_thumbnails() {
        let dir = std::env::temp_dir().join(format!("wandur-thumbs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let gate = Arc::new((Mutex::new(true), Condvar::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        for round in 0..2 {
            let disk = ThumbCache::open(dir.clone(), 1024 * 1024);
            let mut art = ArtLoader::new(
                1,
                gated(Arc::clone(&gate), Arc::clone(&calls)),
                Some(disk),
                DEFAULT_BUDGET,
                Arc::new(|| {}),
            );
            let r = request("cached");
            wait_until(|| {
                art.begin_frame(&ctx);
                let ready = matches!(art.request(&r), ArtState::Ready { .. });
                art.end_frame();
                ready
            });
            assert_eq!(ArtStats::get(&art.stats.from_disk), round, "second run reads the disk");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "fetched once");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
