//! What session history costs: the flood text (`AnsiGenerator`) through a recorder into a store
//! that keeps nothing (reading lines and the privacy rules), then into `wandur.db` (plus SQLite
//! and the full-text index). Process CPU time per megabyte, and the share of one core at 1 MB/s.
//!
//! `wandur-bench history [--mb N] [--rate BYTES]` (run with `--release`; the rate defaults to
//! 1 MB/s, the flood scenarios' rate).

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use wandur_app::sysstat;
use wandur_core::history::{
    HistoryEntry, HistoryError, HistoryFilter, HistoryHit, HistoryRecorder, HistorySession, HistoryStore,
    SqliteHistoryStore, system_clock,
};

use crate::generator::AnsiGenerator;

/// Keeps nothing, counts entries.
#[derive(Default)]
struct Null(std::sync::atomic::AtomicU64);

impl HistoryStore for Null {
    fn append(&self, _: &HistorySession, entries: &[HistoryEntry]) -> Result<(), HistoryError> {
        self.0
            .fetch_add(entries.len() as u64, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
    fn sessions(&self, _: &HistoryFilter, _: usize, _: usize) -> Result<Vec<HistorySession>, HistoryError> {
        Ok(Vec::new())
    }
    fn search(&self, _: &str, _: &HistoryFilter, _: usize, _: usize) -> Result<Vec<HistoryHit>, HistoryError> {
        Ok(Vec::new())
    }
    fn entries(&self, _: &str, _: i64, _: usize) -> Result<Vec<HistoryEntry>, HistoryError> {
        Ok(Vec::new())
    }
    fn delete(&self, _: &str) -> Result<(), HistoryError> {
        Ok(())
    }
    fn prune(&self, _: SystemTime) -> Result<(), HistoryError> {
        Ok(())
    }
}

fn feed(store: Arc<dyn HistoryStore>, chunks: &[String], rate: usize) -> (Duration, Duration, bool) {
    let session = HistorySession::start("127.0.0.1:4000", "Bench", "Odo", SystemTime::now());
    let mut recorder = HistoryRecorder::start(store, session, 30, system_clock()).expect("recorder");
    let cpu = sysstat::cpu_time();
    let mut caller = Duration::ZERO;
    let started = Instant::now();
    let mut sent = 0usize;
    for chunk in chunks {
        let t = Instant::now();
        recorder.received(chunk, false);
        caller += t.elapsed();
        sent += chunk.len();
        // Paced at `rate` bytes a second, in the app's 30 hand-overs a second.
        let due = Duration::from_secs_f64(sent as f64 / rate as f64);
        let ahead = due.saturating_sub(started.elapsed());
        if ahead > Duration::from_millis(30) {
            std::thread::sleep(ahead);
        }
    }
    let faulted = recorder.is_faulted();
    if let Some(handle) = recorder.complete() {
        let _ = handle.join();
    }
    (sysstat::cpu_time() - cpu, caller, faulted)
}

pub fn run(megabytes: usize, rate: usize) {
    let mut generator = AnsiGenerator::new(7, true);
    let mut text = String::new();
    while text.len() < megabytes << 20 {
        generator.next_line(&mut text);
    }
    let chunks: Vec<String> = text
        .as_bytes()
        .chunks((rate / 30).max(4096))
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect();
    let mb = text.len() as f64 / 1_048_576.0;
    let null = Arc::new(Null::default());
    let (cpu, caller, faulted) = feed(null.clone(), &chunks, rate);
    println!(
        "lines and privacy rules: {:.1} ms CPU per MB ({:.1}% of a core at 1 MB/s), caller {:.2} ms per MB, {} entries, faulted {faulted}",
        cpu.as_secs_f64() * 1000.0 / mb,
        cpu.as_secs_f64() / mb * 100.0,
        caller.as_secs_f64() * 1000.0 / mb,
        null.0.load(std::sync::atomic::Ordering::Relaxed)
    );
    let dir = std::env::var("WANDUR_BENCH_HISTORY_DIR").unwrap_or_else(|_| ".superpowers/perf/history-bench".into());
    let dir = std::path::Path::new(&dir);
    let _ = std::fs::remove_dir_all(dir);
    let (db, _) = wandur_core::db::Database::open(dir).expect("bench database");
    let (cpu, caller, faulted) = feed(Arc::new(SqliteHistoryStore::new(db)), &chunks, rate);
    println!(
        "into wandur.db with the index: {:.1} ms CPU per MB ({:.1}% of a core at 1 MB/s), caller {:.2} ms per MB, faulted {faulted}",
        cpu.as_secs_f64() * 1000.0 / mb,
        cpu.as_secs_f64() / mb * 100.0,
        caller.as_secs_f64() * 1000.0 / mb,
    );
    let size: u64 = std::fs::read_dir(dir)
        .map(|d| d.filter_map(|e| e.ok()?.metadata().ok()).map(|m| m.len()).sum())
        .unwrap_or(0);
    println!(
        "database size {:.1} MB for {mb:.1} MB of server text",
        size as f64 / 1_048_576.0
    );
    let _ = std::fs::remove_dir_all(dir);
}
