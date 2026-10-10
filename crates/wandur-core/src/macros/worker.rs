//! Trigger matching off the UI thread. A session hands each batch of complete public lines to
//! its worker, which matches them against its own copy of the trigger rules and hands back the
//! rules that fired (and the line buffers, for reuse). The session then sends their commands on
//! its own thread, with the rate limit and the privacy check at that moment, as the C# client's
//! script worker does. A frame costs the session only taking the lines.
//!
//! Every batch carries the session's rules generation and comes back with it, so the session can
//! drop results for older rules (after a reload or the session switch); batches are matched in
//! order with the rules last loaded before them. The queue is bounded: when the worker falls this far behind,
//! triggers stop with the C# "too many events were waiting" message.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, channel, sync_channel};
use std::thread::{self, JoinHandle};

use super::rules::RuleSet;
use super::runtime::SavedMacro;

/// Batches that may wait for the worker (one per frame with output: several seconds).
pub const MAX_QUEUED_BATCHES: usize = 256;

/// Wakes the UI when results are ready.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

enum Job {
    Rules(Vec<SavedMacro>),
    Lines { generation: u64, lines: Vec<String> },
}

/// What one batch set off.
pub struct Matched {
    pub generation: u64,
    /// Trigger rule indices (as in a [`RuleSet`] built from the same macros), in line order;
    /// each line's hits in library order.
    pub hits: Vec<u32>,
    /// The batch's line buffers, to give back to the terminal for reuse.
    pub lines: Vec<String>,
}

/// Why a batch was not queued.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// The worker is too far behind.
    Full(Vec<String>),
    /// The worker has stopped.
    Gone(Vec<String>),
}

pub struct TriggerWorker {
    jobs: Option<SyncSender<Job>>,
    done: Receiver<Matched>,
    thread: Option<JoinHandle<()>>,
}

impl TriggerWorker {
    pub fn spawn(name: &str, wake: Wake) -> std::io::Result<Self> {
        let (jobs, inbox) = sync_channel::<Job>(MAX_QUEUED_BATCHES);
        let (results, done) = channel();
        let thread = thread::Builder::new().name(name.into()).spawn(move || {
            let mut rules = RuleSet::default();
            while let Ok(job) = inbox.recv() {
                match job {
                    Job::Rules(macros) => {
                        rules = RuleSet::build(
                            macros
                                .iter()
                                .filter(|m| m.enabled)
                                .map(|m| (m.id.as_str(), &m.definition)),
                        );
                    }
                    Job::Lines { generation, lines } => {
                        let mut hits = Vec::new();
                        for line in &lines {
                            rules.match_line(line, &mut hits);
                        }
                        if results
                            .send(Matched {
                                generation,
                                hits,
                                lines,
                            })
                            .is_err()
                        {
                            break;
                        }
                        wake();
                    }
                }
            }
        })?;
        Ok(Self {
            jobs: Some(jobs),
            done,
            thread: Some(thread),
        })
    }

    /// Match the batches queued from now on with these macros (the enabled ones).
    pub fn load(&self, macros: Vec<SavedMacro>) {
        if let Some(jobs) = &self.jobs {
            // Blocking here is fine: rules change rarely and the worker drains quickly.
            let _ = jobs.send(Job::Rules(macros));
        }
    }

    /// Queue a batch of lines. Never blocks.
    pub fn submit(&self, generation: u64, lines: Vec<String>) -> Result<(), Refused> {
        let Some(jobs) = &self.jobs else {
            return Err(Refused::Gone(lines));
        };
        match jobs.try_send(Job::Lines { generation, lines }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(Job::Lines { lines, .. })) => Err(Refused::Full(lines)),
            Err(TrySendError::Disconnected(Job::Lines { lines, .. })) => Err(Refused::Gone(lines)),
            Err(_) => Err(Refused::Gone(Vec::new())),
        }
    }

    /// The next finished batch, if any.
    pub fn poll(&self) -> Option<Matched> {
        self.done.try_recv().ok()
    }
}

impl Drop for TriggerWorker {
    fn drop(&mut self) {
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::macros::{MacroDefinition, MacroKind};
    use std::time::{Duration, Instant};

    fn wait(worker: &TriggerWorker) -> Matched {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(m) = worker.poll() {
                return m;
            }
            assert!(Instant::now() < deadline, "no result");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn matches_batches_off_thread_in_order_with_the_rules_loaded_before_them() {
        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = Arc::clone(&woken);
        let worker = TriggerWorker::spawn(
            "test-triggers",
            Arc::new(move || {
                w.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }),
        )
        .unwrap();
        let macros = vec![
            SavedMacro {
                id: "a".into(),
                name: "a".into(),
                enabled: true,
                definition: MacroDefinition::new(MacroKind::Trigger, "hungry", "eat"),
            },
            SavedMacro {
                id: "b".into(),
                name: "b".into(),
                enabled: true,
                definition: MacroDefinition::new(MacroKind::Trigger, "thirsty", "drink").ignoring_case(),
            },
        ];
        worker.load(macros.clone());
        worker
            .submit(
                1,
                vec!["You are hungry.".into(), "Quiet.".into(), "THIRSTY and hungry".into()],
            )
            .unwrap();
        let m = wait(&worker);
        assert_eq!((m.generation, m.hits.as_slice()), (1, &[0, 0, 1][..]));
        assert_eq!(m.lines.len(), 3, "buffers come back");
        assert!(woken.load(std::sync::atomic::Ordering::Relaxed) >= 1);
        worker.load(macros[1..].to_vec());
        worker.submit(2, vec!["thirsty".into()]).unwrap();
        let m = wait(&worker);
        assert_eq!(
            (m.generation, m.hits.as_slice()),
            (2, &[0][..]),
            "the new rules' numbering"
        );
    }
}
