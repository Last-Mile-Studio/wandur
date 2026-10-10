//! One writer thread for the database. Callers queue jobs and return at once; the thread takes
//! the first job, then everything else already queued (up to [`MAX_BATCH`]), and applies them as
//! one batch: for SQLite one transaction, with a savepoint per job so a failing job does not undo
//! the others. A slow disk therefore makes batches bigger, never the caller slower.
//!
//! The thread is generic over a [`BatchSink`] so tests can put a slow fake behind it.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread;

use rusqlite::{Connection, TransactionBehavior};

use crate::l10n::{S, tf};

/// Most jobs applied in one batch.
pub const MAX_BATCH: usize = 512;

/// A database write, run on the writer thread inside the batch's transaction.
pub type Job = Box<dyn FnOnce(&Connection) -> Result<(), super::DbError> + Send>;

/// The SQLite writer.
pub type DbWriter = Writer<Job>;

/// Where a batch goes.
pub trait BatchSink: Send + 'static {
    type Job: Send + 'static;
    /// Apply a batch; returns one message per job that failed.
    fn apply(&mut self, batch: Vec<Self::Job>) -> Vec<String>;
}

/// What the writer has done, readable from any thread.
#[derive(Debug, Default)]
pub struct WriterStats {
    pub jobs: AtomicU64,
    pub batches: AtomicU64,
    pub failures: AtomicU64,
    /// The most jobs seen in one batch.
    pub largest_batch: AtomicU64,
    pub last_error: Mutex<Option<String>>,
    /// The writer thread's id, as text.
    pub thread: Mutex<Option<String>>,
}

enum Message<J> {
    Job(J),
    Flush(Sender<()>),
}

pub struct Writer<J: Send + 'static> {
    tx: Option<Sender<Message<J>>>,
    worker: Option<thread::JoinHandle<()>>,
    stats: Arc<WriterStats>,
}

impl<J: Send + 'static> Writer<J> {
    pub fn spawn<S: BatchSink<Job = J>>(name: &str, sink: S) -> std::io::Result<Self> {
        let (tx, rx) = channel();
        let stats = Arc::new(WriterStats::default());
        let thread_stats = Arc::clone(&stats);
        let worker = thread::Builder::new()
            .name(name.into())
            .spawn(move || run(sink, &rx, &thread_stats))?;
        Ok(Self {
            tx: Some(tx),
            worker: Some(worker),
            stats,
        })
    }

    /// Queue a job; never blocks.
    pub fn submit(&self, job: J) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Message::Job(job));
        }
    }

    /// Wait until everything queued so far is applied.
    pub fn flush(&self) {
        let (done, wait) = channel();
        if let Some(tx) = &self.tx
            && tx.send(Message::Flush(done)).is_ok()
        {
            let _ = wait.recv();
        }
    }

    pub fn stats(&self) -> &WriterStats {
        &self.stats
    }

    /// Apply what is queued and stop the thread (also done on drop).
    pub fn close(&mut self) {
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl<J: Send + 'static> Drop for Writer<J> {
    fn drop(&mut self) {
        self.close();
    }
}

fn run<S: BatchSink>(mut sink: S, rx: &Receiver<Message<S::Job>>, stats: &WriterStats) {
    if let Ok(mut id) = stats.thread.lock() {
        *id = Some(format!("{:?}", thread::current().id()));
    }
    let mut flushes = Vec::new();
    while let Ok(first) = rx.recv() {
        let mut batch = Vec::new();
        let mut open = true;
        let mut take = |m: Message<S::Job>, batch: &mut Vec<S::Job>| match m {
            Message::Job(j) => batch.push(j),
            Message::Flush(done) => flushes.push(done),
        };
        take(first, &mut batch);
        while batch.len() < MAX_BATCH {
            match rx.try_recv() {
                Ok(m) => take(m, &mut batch),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    open = false;
                    break;
                }
            }
        }
        if !batch.is_empty() {
            let n = batch.len() as u64;
            let failures = sink.apply(batch);
            stats.jobs.fetch_add(n, Ordering::Relaxed);
            stats.batches.fetch_add(1, Ordering::Relaxed);
            stats.largest_batch.fetch_max(n, Ordering::Relaxed);
            if !failures.is_empty() {
                stats.failures.fetch_add(failures.len() as u64, Ordering::Relaxed);
                for f in &failures {
                    eprintln!("wandur: database write failed: {f}");
                }
                if let Ok(mut last) = stats.last_error.lock() {
                    *last = failures.last().cloned();
                }
            }
        }
        for done in flushes.drain(..) {
            let _ = done.send(());
        }
        if !open {
            break;
        }
    }
}

/// Applies jobs to SQLite: one immediate transaction per batch, a savepoint per job.
pub struct SqliteSink {
    conn: Connection,
}

impl SqliteSink {
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }
}

impl BatchSink for SqliteSink {
    type Job = Job;

    fn apply(&mut self, batch: Vec<Job>) -> Vec<String> {
        let n = batch.len();
        let mut failures = Vec::new();
        let mut tx = match self.conn.transaction_with_behavior(TransactionBehavior::Immediate) {
            Ok(tx) => tx,
            Err(e) => return vec![tf(S::DatabaseWritesLost, &[&n, &e])],
        };
        for job in batch {
            let result = tx.savepoint().map_err(super::DbError::from).and_then(|sp| {
                job(&sp)?;
                sp.commit()?;
                Ok(())
            });
            if let Err(e) = result {
                failures.push(e.to_string());
            }
        }
        if let Err(e) = tx.commit() {
            return vec![tf(S::DatabaseWritesLost, &[&n, &e])];
        }
        failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Batches applied, with the thread that applied each.
    type Applied = Arc<Mutex<Vec<(thread::ThreadId, Vec<u32>)>>>;

    /// A sink that takes 100 ms per batch and records which thread applied it.
    struct SlowFake {
        applied: Applied,
    }

    impl BatchSink for SlowFake {
        type Job = u32;
        fn apply(&mut self, batch: Vec<u32>) -> Vec<String> {
            thread::sleep(Duration::from_millis(100));
            self.applied.lock().unwrap().push((thread::current().id(), batch));
            Vec::new()
        }
    }

    #[test]
    fn a_slow_sink_never_slows_the_caller_and_gets_bigger_batches() {
        let applied = Arc::new(Mutex::new(Vec::new()));
        let mut writer = Writer::spawn(
            "test-writer",
            SlowFake {
                applied: Arc::clone(&applied),
            },
        )
        .unwrap();
        let started = Instant::now();
        for i in 0..200 {
            writer.submit(i);
            if i % 50 == 0 {
                thread::sleep(Duration::from_millis(5));
            }
        }
        let submitting = started.elapsed();
        // 200 submissions against a sink that takes 100 ms a batch: the caller only paid for its
        // own short sleeps.
        assert!(submitting < Duration::from_millis(80), "submitting took {submitting:?}");
        writer.flush();
        let applied = applied.lock().unwrap();
        let me = thread::current().id();
        assert!(applied.iter().all(|(t, _)| *t != me), "applied on the caller's thread");
        let jobs: Vec<u32> = applied.iter().flat_map(|(_, b)| b.iter().copied()).collect();
        assert_eq!(jobs, (0..200).collect::<Vec<_>>(), "every job once, in order");
        assert!(applied.len() < 10, "{} batches for 200 jobs", applied.len());
        assert!(writer.stats().largest_batch.load(Ordering::Relaxed) > 40);
        writer.close();
    }

    #[test]
    fn close_applies_what_is_queued() {
        let applied = Arc::new(Mutex::new(Vec::new()));
        let mut writer = Writer::spawn(
            "test-writer",
            SlowFake {
                applied: Arc::clone(&applied),
            },
        )
        .unwrap();
        for i in 0..10 {
            writer.submit(i);
        }
        writer.close();
        let n: usize = applied.lock().unwrap().iter().map(|(_, b)| b.len()).sum();
        assert_eq!(n, 10);
    }

    #[test]
    fn a_failing_sqlite_job_does_not_undo_the_rest_of_its_batch() {
        let dir = std::env::temp_dir().join(format!("wandur-db-writer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (db, _) = super::super::Database::open(&dir).unwrap();
        let writer = db.writer().unwrap();
        let caller = thread::current().id();
        let seen = Arc::new(Mutex::new(None));
        let seen2 = Arc::clone(&seen);
        writer.submit(Box::new(move |c: &Connection| {
            *seen2.lock().unwrap() = Some(thread::current().id());
            c.execute("INSERT INTO worlds(id) VALUES('a')", [])?;
            Ok(())
        }));
        writer.submit(Box::new(|c: &Connection| {
            // Fails: no such world.
            c.execute(
                "INSERT INTO endpoints(endpoint_key, world_id) VALUES('h:1', 'nope')",
                [],
            )?;
            Ok(())
        }));
        writer.submit(Box::new(|c: &Connection| {
            c.execute("INSERT INTO worlds(id) VALUES('b')", [])?;
            Ok(())
        }));
        writer.flush();
        assert_ne!(seen.lock().unwrap().unwrap(), caller);
        assert_eq!(writer.stats().failures.load(Ordering::Relaxed), 1);
        let n: i64 = db
            .read(|c| Ok(c.query_row("SELECT count(*) FROM worlds", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(n, 2);
        drop(writer);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
