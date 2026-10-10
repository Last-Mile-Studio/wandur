//! Classification on a background thread (the C# `WorkspaceController.Inference`, with the
//! hashing moved off the UI thread as its follow-up asked).
//!
//! The UI thread only copies the text of rooms without terrain into [`InferenceJob`]s
//! ([`InferenceWorker::schedule`]), skipping rooms already checked at their current revision;
//! the worker computes each room's inference key (SHA-256), skips rooms whose stored key is
//! current, loads the classifier on first use and classifies the rest. Results are applied on
//! the UI thread by [`InferenceWorker::apply`] through
//! [`RoomMapTracker::apply_inference_result`], which compares the room's text (not its hash)
//! to drop stale results.
//!
//! The queue holds at most 512 rooms, each once, the player's room first. When the queue runs
//! dry after doing something the worker asks for a rescan, which queues the next rooms.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use super::service::RoomClassificationService;
use super::{RoomEnvironmentPrediction, preprocess};
use crate::map::RoomMapTracker;

/// Most rooms waiting at once.
pub const QUEUE_LIMIT: usize = 512;

/// A room to classify: its id, its text as the map holds it, and the key stored with its last
/// inference.
#[derive(Clone, Debug, PartialEq)]
pub struct InferenceJob {
    pub id: String,
    pub name: String,
    pub description: String,
    pub inferred_key: Option<String>,
    /// The room's revision when it was read.
    pub revision: i64,
}

/// A classified room: the text it was classified on, its new key and the guess (`None` when
/// the classifier abstained).
impl InferenceJob {
    pub fn of(room: &crate::map::MapRoom) -> Self {
        Self {
            id: room.id.clone(),
            name: room.name.clone(),
            description: room.description.clone(),
            inferred_key: room.inferred_key.clone(),
            revision: room.revision,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct InferenceResult {
    pub id: String,
    pub name: String,
    pub description: String,
    pub key: String,
    pub model_version: String,
    pub prediction: Option<RoomEnvironmentPrediction>,
}

enum Event {
    Result(u64, InferenceResult),
    /// The room's stored inference is current (id, revision).
    Fresh(u64, String, i64),
    Rescan(u64),
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<InferenceJob>,
    queued: HashSet<String>,
    generation: u64,
    model_version: String,
    threshold: f64,
    progressed: bool,
    stop: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    signal: Condvar,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// What a drain gives the UI thread.
#[derive(Debug, Default)]
pub struct Drained {
    pub results: Vec<InferenceResult>,
    /// Rooms found current: (id, revision).
    pub fresh: Vec<(String, i64)>,
    /// The queue ran dry after classifying something: scan the map once more.
    pub rescan: bool,
}

pub struct InferenceWorker {
    shared: Arc<Shared>,
    events: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
    /// Rooms checked at a revision (UI side): a scan skips them until they change.
    checked: HashMap<String, i64>,
}

impl std::fmt::Debug for InferenceWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InferenceWorker").finish_non_exhaustive()
    }
}

impl InferenceWorker {
    /// Start the worker thread. `wake` is called (from the worker) when results are waiting.
    pub fn spawn(service: Arc<RoomClassificationService>, wake: Box<dyn Fn() + Send>) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            signal: Condvar::new(),
        });
        let (sender, events) = channel();
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("wandur-inference".into())
            .spawn(move || run(&worker, &service, &sender, &*wake))?;
        Ok(Self {
            shared,
            events,
            thread: Some(thread),
            checked: HashMap::new(),
        })
    }

    /// Queue rooms (deduplicated, bounded, `current` first) for `model_version` at
    /// `threshold`. Cheap: no hashing here.
    pub fn submit(&self, jobs: Vec<InferenceJob>, current: Option<&str>, model_version: &str, threshold: f64) {
        let mut queue = self.shared.lock();
        if queue.model_version != model_version {
            queue.jobs.clear();
            queue.queued.clear();
            queue.generation += 1;
            queue.model_version = model_version.to_string();
        }
        queue.threshold = threshold;
        for job in jobs {
            if queue.jobs.len() >= QUEUE_LIMIT {
                break;
            }
            if !queue.queued.insert(job.id.clone()) {
                continue;
            }
            if current == Some(job.id.as_str()) {
                queue.jobs.push_front(job);
            } else {
                queue.jobs.push_back(job);
            }
        }
        drop(queue);
        self.shared.signal.notify_one();
    }

    /// Forget waiting rooms and drop results still on their way (another map, inference off).
    pub fn reset(&mut self) {
        self.checked.clear();
        let mut queue = self.shared.lock();
        queue.jobs.clear();
        queue.queued.clear();
        queue.progressed = false;
        queue.generation += 1;
    }

    /// Rooms waiting.
    pub fn pending(&self) -> usize {
        self.shared.lock().jobs.len()
    }

    /// Results ready now, for the current generation only.
    pub fn drain(&self) -> Drained {
        let generation = self.shared.lock().generation;
        let mut drained = Drained::default();
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Result(g, result) if g == generation => drained.results.push(result),
                Event::Fresh(g, id, revision) if g == generation => drained.fresh.push((id, revision)),
                Event::Rescan(g) if g == generation => drained.rescan = true,
                _ => {}
            }
        }
        if !drained.results.is_empty() {
            // A room whose result arrived may be queued again by a later scan.
            let mut queue = self.shared.lock();
            for r in &drained.results {
                if !queue.jobs.iter().any(|j| j.id == r.id) {
                    queue.queued.remove(&r.id);
                }
            }
        }
        drained
    }

    /// Queue every room without terrain not yet checked at its revision (UI thread: copies
    /// text, compares revisions, no hashing). Returns how many were offered.
    pub fn schedule(&mut self, tracker: &RoomMapTracker, model_version: &str, threshold: f64) -> usize {
        let jobs: Vec<InferenceJob> = tracker
            .inference_candidates()
            .filter(|r| self.checked.get(&r.id) != Some(&r.revision))
            .take(QUEUE_LIMIT)
            .map(InferenceJob::of)
            .collect();
        let count = jobs.len();
        if count > 0 {
            self.submit(jobs, tracker.current_id(), model_version, threshold);
        }
        count
    }

    /// Queue one room (an observation's), unless it has terrain or was checked as it is.
    pub fn schedule_room(&mut self, tracker: &RoomMapTracker, id: &str, model_version: &str, threshold: f64) {
        if let Some(room) = tracker.room(id)
            && room.environment.as_deref().is_none_or(|e| e.trim().is_empty())
            && self.checked.get(id) != Some(&room.revision)
        {
            self.submit(vec![InferenceJob::of(room)], Some(id), model_version, threshold);
        }
    }

    /// Apply what the worker finished (UI thread). Returns whether the map changed and whether
    /// a rescan is due.
    pub fn apply(&mut self, tracker: &mut RoomMapTracker) -> (bool, bool) {
        let drained = self.drain();
        for (id, revision) in drained.fresh {
            self.checked.insert(id, revision);
        }
        let mut changed = false;
        for result in &drained.results {
            if tracker.apply_inference_result(result) {
                changed = true;
                if let Some(room) = tracker.room(&result.id) {
                    self.checked.insert(result.id.clone(), room.revision);
                }
            }
        }
        (changed, drained.rescan)
    }
}

impl Drop for InferenceWorker {
    fn drop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.signal.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(shared: &Shared, service: &RoomClassificationService, events: &Sender<Event>, wake: &dyn Fn()) {
    loop {
        let (job, generation, version, threshold) = {
            let mut queue = shared.lock();
            loop {
                if queue.stop {
                    return;
                }
                if let Some(job) = queue.jobs.pop_front() {
                    break (job, queue.generation, queue.model_version.clone(), queue.threshold);
                }
                if queue.progressed {
                    queue.progressed = false;
                    let _ = events.send(Event::Rescan(queue.generation));
                    wake();
                }
                queue = shared
                    .signal
                    .wait(queue)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        let forget = |id: &str| {
            shared.lock().queued.remove(id);
        };
        // Creating the classifier loads the model, which is why it happens here.
        let Some(classifier) = service.try_get_classifier().filter(|c| c.model_version() == version) else {
            let mut queue = shared.lock();
            if queue.generation == generation {
                queue.jobs.clear();
                queue.queued.clear();
            }
            continue;
        };
        let key = preprocess::inference_key(&job.name, &job.description, &version);
        if job.inferred_key.as_deref() == Some(key.as_str()) {
            forget(&job.id);
            let mut queue = shared.lock();
            if queue.generation == generation {
                queue.progressed = true;
                let _ = events.send(Event::Fresh(generation, job.id, job.revision));
            }
            continue;
        }
        match classifier.classify(&job.name, &job.description, threshold) {
            Ok(prediction) => {
                let result = InferenceResult {
                    id: job.id,
                    name: job.name,
                    description: job.description,
                    key,
                    model_version: version,
                    prediction,
                };
                {
                    let mut queue = shared.lock();
                    if queue.generation != generation {
                        continue;
                    }
                    queue.progressed = true;
                }
                let _ = events.send(Event::Result(generation, result));
                wake();
            }
            // One bad room must not stop the worker or stay stuck as queued.
            Err(_) => forget(&job.id),
        }
    }
}
