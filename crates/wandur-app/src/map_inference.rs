//! Room terrain inference for one session's map (the C# `WorkspaceController.Inference`): rooms
//! without terrain go to a background worker, results come back between frames.
//!
//! The UI thread copies room text and compares revisions only; inference keys (SHA-256) and
//! the classifier run on the worker (see `wandur_core::classify::worker`). Nothing runs while
//! inference is off, before a package is ready, or during a map walk (applied results bump
//! room revisions, which a walk reads as a changed map); the map catches up afterwards.

use std::sync::Arc;
use std::time::{Duration, Instant};

use wandur_core::classify::{InferenceWorker, RoomClassificationService};
use wandur_core::map::MapSession;
use wandur_core::session::Waker;

pub struct SessionInference {
    service: Arc<RoomClassificationService>,
    worker: Option<InferenceWorker>,
    pub enabled: bool,
    pub threshold: f64,
    /// A full scan is due (load, enable, install, an edit, a walk's end, the worker's ask).
    scan: bool,
    /// What was last seen: the map generation, the map version, the service's status, the
    /// threshold.
    seen: (u64, u64, usize, u64),
    was_walking: bool,
    /// UI-thread time spent scheduling and applying (the measurement).
    pub ui_time: Duration,
    pub applied: usize,
}

impl std::fmt::Debug for SessionInference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionInference")
            .field("enabled", &self.enabled)
            .field("threshold", &self.threshold)
            .finish_non_exhaustive()
    }
}

impl SessionInference {
    pub fn new(service: Arc<RoomClassificationService>, enabled: bool, threshold: f64) -> Self {
        Self {
            service,
            worker: None,
            enabled,
            threshold,
            scan: true,
            seen: (u64::MAX, u64::MAX, usize::MAX, threshold.to_bits()),
            was_walking: false,
            ui_time: Duration::ZERO,
            applied: 0,
        }
    }

    pub fn service(&self) -> &Arc<RoomClassificationService> {
        &self.service
    }

    /// Settings > the map's inference switch and threshold.
    pub fn configure(&mut self, enabled: bool, threshold: f64) {
        if enabled != self.enabled || threshold != self.threshold {
            self.enabled = enabled;
            self.threshold = threshold;
            if let Some(worker) = &mut self.worker {
                worker.reset();
            }
            self.scan = true;
        }
    }

    /// Scan the whole map at the next chance (after a hand edit or an import).
    pub fn rescan(&mut self) {
        self.scan = true;
    }

    /// Rooms waiting on the worker.
    pub fn pending(&self) -> usize {
        self.worker.as_ref().map_or(0, InferenceWorker::pending)
    }

    /// Between frames: apply finished results, queue new rooms. Returns whether the map
    /// changed.
    pub fn follow(&mut self, map: &mut MapSession, waker: Option<&Waker>) -> bool {
        let Some(version) = self.service.model_version().filter(|_| self.enabled) else {
            if let Some(worker) = &mut self.worker {
                worker.reset();
            }
            return false;
        };
        let walking = map.is_walking();
        if walking {
            self.was_walking = true;
            return false;
        }
        let started = Instant::now();
        if self.was_walking {
            self.was_walking = false;
            self.scan = true;
        }
        let worker = match &mut self.worker {
            Some(worker) => worker,
            None => {
                let wake = waker.cloned();
                let Ok(worker) = InferenceWorker::spawn(
                    Arc::clone(&self.service),
                    Box::new(move || {
                        if let Some(w) = &wake {
                            w();
                        }
                    }),
                ) else {
                    return false;
                };
                self.worker.insert(worker)
            }
        };
        let generation = map.generation();
        let status = self.service.generation();
        if generation != self.seen.0 || status != self.seen.2 || self.threshold.to_bits() != self.seen.3 {
            // Another map (a load), another package or threshold: start over.
            worker.reset();
            self.scan = true;
        }
        let (changed, rescan) = worker.apply(map.tracker_mut());
        self.applied += usize::from(changed);
        self.scan |= rescan;
        if self.scan {
            self.scan = false;
            worker.schedule(map.tracker(), &version, self.threshold);
        } else if map.version() != self.seen.1
            && let Some(id) = map.tracker().current_id()
        {
            // An observation: the room the player is standing in is the one whose colour they
            // are waiting for (a revision compare; unchanged rooms are not queued again).
            worker.schedule_room(map.tracker(), id, &version, self.threshold);
        }
        self.seen = (generation, map.version(), status, self.threshold.to_bits());
        self.ui_time += started.elapsed();
        changed
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use wandur_core::classify::RoomClassificationService;
    use wandur_core::map::{
        MapLink, MapRoom, MapSession, MapSnapshot, RoomObservation, RoomSource, WalkGate, find_route,
    };

    use super::*;
    use crate::scene::KeywordClassifier;

    fn session() -> MapSession {
        let room = |id: &str, name: &str, x: f64| {
            MapRoom::new(&format!("s:{id}"), name, "", None, x, 0.0, 0.0, false).with_server_id(id)
        };
        let mut map = MapSession::with_map(MapSnapshot::of(
            vec![
                room("1", "Old Road", 0.0),
                room("2", "Oak Forest", 1.0),
                room("3", "Grotto", 2.0),
            ],
            vec![
                MapLink::new("s:1", "s:2", "east", true),
                MapLink::new("s:2", "s:3", "east", true),
            ],
        ));
        let here = RoomObservation::new(Some("1"), "Old Road", "", &[]).with_source(RoomSource::Gmcp);
        map.observe_room(here, gate(), Instant::now());
        map
    }

    fn gate() -> WalkGate {
        WalkGate {
            connected: true,
            private: false,
            login: false,
            remote_echo: false,
        }
    }

    fn inference(enabled: bool) -> SessionInference {
        let service = Arc::new(RoomClassificationService::for_testing(Arc::new(KeywordClassifier)));
        SessionInference::new(service, enabled, 0.5)
    }

    fn inferred(map: &MapSession) -> Vec<Option<String>> {
        map.tracker().rooms().map(|r| r.inferred_environment.clone()).collect()
    }

    fn follow_until(i: &mut SessionInference, map: &mut MapSession, done: impl Fn(&MapSession) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(map) && Instant::now() < deadline {
            i.follow(map, None);
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn rooms_are_classified_on_the_worker_and_applied_between_frames() {
        let mut map = session();
        let mut i = inference(true);
        follow_until(&mut i, &mut map, |m| {
            m.tracker().rooms().all(|r| r.inferred_key.is_some())
        });
        assert_eq!(
            inferred(&map),
            [Some("road".into()), Some("forest".into()), Some("cave".into())]
        );
        assert!(i.applied > 0);
        assert!(!map.tracker().can_undo(), "inference is not an edit");
    }

    #[test]
    fn nothing_runs_while_inference_is_off() {
        let mut map = session();
        let mut i = inference(false);
        for _ in 0..20 {
            i.follow(&mut map, None);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(i.worker.is_none(), "no worker thread while off");
        assert!(map.tracker().rooms().all(|r| r.inferred_key.is_none()));
        i.configure(true, 0.5);
        follow_until(&mut i, &mut map, |m| {
            m.tracker().rooms().all(|r| r.inferred_key.is_some())
        });
        assert!(map.tracker().rooms().all(|r| r.inferred_key.is_some()));
    }

    #[test]
    fn results_wait_while_walking_and_arrive_after() {
        let mut map = session();
        let mut i = inference(true);
        let route = find_route(&map.tracker().snapshot(), "s:1", "s:3", false).unwrap();
        assert!(map.start_walk(&route, gate(), Instant::now()).is_some());
        for _ in 0..20 {
            i.follow(&mut map, None);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(map.is_walking(), "a walk is not disturbed by applied results");
        assert!(map.tracker().rooms().all(|r| r.inferred_key.is_none()));
        map.stop_walk(wandur_core::map::WalkStatus::Stopped);
        follow_until(&mut i, &mut map, |m| {
            m.tracker().rooms().all(|r| r.inferred_key.is_some())
        });
        assert!(map.tracker().rooms().all(|r| r.inferred_key.is_some()));
    }

    #[test]
    fn server_terrain_and_hand_edits_win_over_inference() {
        let mut map = session();
        let mut room = map.tracker().room("s:2").unwrap().clone();
        room.environment = Some("water".into());
        assert!(map.tracker_mut().upsert_room(room));
        let mut i = inference(true);
        follow_until(&mut i, &mut map, |m| {
            m.tracker().room("s:3").unwrap().inferred_key.is_some()
        });
        std::thread::sleep(Duration::from_millis(20));
        i.follow(&mut map, None);
        let forest = map.tracker().room("s:2").unwrap();
        assert!(forest.inferred_key.is_none(), "rooms with terrain are never classified");
        let (fill, _) = crate::map_palette::resolve(forest);
        assert_eq!(
            fill,
            egui::Color32::from_rgb(0x16, 0xA7, 0xD5),
            "the room's own terrain is drawn"
        );
    }

    /// With the `classifier` feature and the local 0.1.1 package: the service reads only the
    /// manifest; the model is verified and loaded on the worker the first time a room needs
    /// it, and not at all while inference is off.
    #[cfg(feature = "classifier")]
    #[test]
    fn the_onnx_package_loads_only_when_enabled_and_needed() {
        let dir = std::env::var_os("WANDUR_ROOM_MODEL_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.superpowers/models/room-classifier/0.1.1")
            });
        if !dir.join("manifest.json").is_file() {
            eprintln!("skipped: no classifier package (set WANDUR_ROOM_MODEL_DIR)");
            return;
        }
        let service = Arc::new(RoomClassificationService::new(None, Some(dir)));
        assert_eq!(service.model_version().as_deref(), Some("0.1.1"));
        let mut map = session();
        let mut i = SessionInference::new(Arc::clone(&service), false, 0.8);
        for _ in 0..10 {
            i.follow(&mut map, None);
        }
        assert_eq!(service.loads(), 0, "nothing loads while inference is off");
        i.configure(true, 0.5);
        let deadline = Instant::now() + Duration::from_secs(60);
        while map.tracker().rooms().any(|r| r.inferred_key.is_none()) && Instant::now() < deadline {
            i.follow(&mut map, None);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(service.loads(), 1);
        assert!(map.tracker().rooms().all(|r| r.inferred_key.is_some()));
    }

    #[test]
    fn a_description_read_from_the_text_is_classified() {
        // A LOTJ-like world: `Room.Info` without a description, the text after it.
        let mut map = MapSession::new();
        map.set_gmcp(wandur_core::map::OptionState::Enabled);
        let here = RoomObservation::new(Some("9"), "Narrow Path", "", &[("north", None)]).with_source(RoomSource::Gmcp);
        map.observe_room(here, gate(), Instant::now());
        let mut i = inference(true);
        follow_until(&mut i, &mut map, |m| {
            m.tracker().rooms().all(|r| r.inferred_key.is_some())
        });
        assert_eq!(inferred(&map), [None], "the name alone says nothing");
        map.track_output(
            "\x1b[1;32mNarrow Path [Trail]\x1b[0m\r\nTall oaks close over the trail.\r\nObvious exits:\r\nNorth - A Clearing\r\n\r\n",
            gate(),
            Instant::now(),
        );
        assert_eq!(
            map.tracker().room("s:9").unwrap().description,
            "Tall oaks close over the trail."
        );
        follow_until(&mut i, &mut map, |m| inferred(m) == [Some("forest".to_string())]);
        assert_eq!(inferred(&map), [Some("forest".into())]);
    }
}
