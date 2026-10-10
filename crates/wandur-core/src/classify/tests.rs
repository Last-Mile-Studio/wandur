//! The C# `RoomTextPreprocessorTests`, `ModelPackageTests`, `RoomClassificationServiceTests`,
//! `RoomInferenceTests` and the gated `OnnxRoomEnvironmentClassifierTests`, plus the worker.

#[cfg(feature = "classifier")]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::preprocess::{build_text, clean, inference_key};
use super::*;
use crate::map::{MapRoom, MapSnapshot, RoomMapTracker, RoomObservation, RoomSource};

const FIXTURE: &str = include_str!("../../tests/fixtures/room-classifier-parity.json");

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wandur-classify-{name}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The C# `CreateFakePackage`: a package whose manifest matches, with a fake encoder.
fn fake_package(version: &str, extra: &[(&str, &str)], tamper: Option<&str>) -> PathBuf {
    let dir = temp_dir("package");
    let files: Vec<(&str, String)> = vec![
        ("encoder.onnx", "not-a-real-model".into()),
        (
            "tokenizer.json",
            json!({"model": {"type": "WordPiece", "vocab": {"[PAD]": 0, "[UNK]": 1, "[CLS]": 2, "[SEP]": 3, "hall": 4, "##s": 5}}}).to_string(),
        ),
        (
            "head.json",
            json!({"classes": ["cave", "forest"], "coef": [vec![0.5; 384], vec![-0.5; 384]], "intercept": [0.1, -0.1]}).to_string(),
        ),
        (
            "preprocessing_spec.json",
            json!({"max_word_pieces": 256, "threshold": 0.8, "taxonomy_version": "1.0.0"}).to_string(),
        ),
        ("taxonomy.json", json!({"version": "1.0.0", "bases": ["cave", "forest"]}).to_string()),
    ];
    let mut manifest = serde_json::Map::new();
    for (name, content) in &files {
        std::fs::write(dir.join(name), content).unwrap();
        manifest.insert((*name).into(), Value::String(sha(content)));
    }
    for (name, content) in extra {
        manifest.insert((*name).into(), Value::String(sha(content)));
    }
    std::fs::write(
        dir.join("manifest.json"),
        json!({"version": version, "files": manifest}).to_string(),
    )
    .unwrap();
    if let Some(name) = tamper {
        std::fs::write(dir.join(name), "tampered").unwrap();
    }
    dir
}

fn sha(content: &str) -> String {
    preprocess::hex(&Sha256::digest(content.as_bytes()))
}

// ---- RoomTextPreprocessorTests ----------------------------------------------------------------

#[test]
fn clean_strips_color_codes_tildes_and_collapses_whitespace() {
    assert_eq!(clean("&RThe &GForest&x"), "The Forest");
    assert_eq!(clean("@rDark@n cave"), "Dark cave");
    assert_eq!(clean("{cMisty{x path"), "Misty path");
    assert_eq!(clean("\x1b[31mRed\x1b[0m room"), "Red room");
    assert_eq!(
        clean("A hall.~\r\n   Dust    hangs\n\nin the air.  "),
        "A hall. Dust hangs in the air."
    );
    assert_eq!(clean(""), "");
}

#[test]
fn build_text_joins_cleaned_name_and_description_with_newline() {
    assert_eq!(build_text("Temple", "A hall."), "Temple\nA hall.");
}

#[test]
fn every_parity_fixture_reproduces_its_text() {
    let fixtures: Value = serde_json::from_str(FIXTURE).unwrap();
    let list = fixtures["fixtures"].as_array().unwrap();
    assert_eq!(list.len(), 23);
    for f in list {
        assert_eq!(
            build_text(f["name"].as_str().unwrap(), f["description"].as_str().unwrap()),
            f["text"].as_str().unwrap()
        );
    }
}

#[test]
fn inference_key_is_stable_and_model_scoped() {
    let a = inference_key("Temple", "A hall.", "0.1.1");
    assert_eq!(a, inference_key("&RTemple", "A  hall.~", "0.1.1"));
    assert_ne!(a, inference_key("Temple", "A hall.", "0.2.0"));
    assert_eq!(a.len(), 64);
}

// ---- ModelPackageTests ------------------------------------------------------------------------

#[test]
fn loads_a_verified_package() {
    let package = ModelPackage::load(&fake_package("9.9.9", &[], None)).unwrap();
    assert_eq!(package.version, "9.9.9");
    assert_eq!(package.threshold, 0.8);
    assert_eq!(package.max_word_pieces, 256);
    assert_eq!(package.classes, ["cave", "forest"]);
    assert_eq!((package.coefficients.len(), package.width), (768, 384));
    assert_eq!(package.intercepts[0], 0.1);
    assert_eq!(
        package.vocabulary().unwrap(),
        ["[PAD]", "[UNK]", "[CLS]", "[SEP]", "hall", "##s"]
    );
    assert_eq!(ModelPackage::peek(&package.directory).as_deref(), Some("9.9.9"));
}

#[test]
fn rejects_tampered_and_incomplete_packages() {
    assert!(ModelPackage::load(&fake_package("9.9.9", &[], Some("head.json"))).is_err());
    let missing = fake_package("9.9.9", &[], None);
    std::fs::remove_file(missing.join("encoder.onnx")).unwrap();
    assert!(ModelPackage::load(&missing).is_err());
    let no_manifest = fake_package("9.9.9", &[], None);
    std::fs::remove_file(no_manifest.join("manifest.json")).unwrap();
    assert!(ModelPackage::load(&no_manifest).is_err());
}

#[test]
fn rejects_unsafe_versions_and_manifest_entries() {
    assert!(ModelPackage::load(&fake_package("../escape", &[], None)).is_err());
    assert!(ModelPackage::peek(&fake_package("../escape", &[], None)).is_none());
    assert!(ModelPackage::load(&fake_package("9.9.9", &[("../x", "y")], None)).is_err());
}

#[test]
fn malformed_manifest_or_head_is_refused() {
    let corrupt = fake_package("9.9.9", &[], None);
    std::fs::write(corrupt.join("manifest.json"), "{ not json").unwrap();
    assert!(ModelPackage::load(&corrupt).is_err());
    let missing_key = fake_package("9.9.9", &[], None);
    std::fs::write(missing_key.join("manifest.json"), "{\"version\":\"1\"}").unwrap();
    assert!(ModelPackage::load(&missing_key).is_err());
}

// ---- RoomClassificationServiceTests -----------------------------------------------------------

/// The C# test classifier: forest at 0.9 for every room.
struct Fake(String);

impl RoomClassifier for Fake {
    fn model_version(&self) -> &str {
        &self.0
    }
    fn default_threshold(&self) -> f64 {
        0.8
    }
    fn classify(&self, _: &str, _: &str, _: f64) -> Result<Option<RoomEnvironmentPrediction>, String> {
        Ok(Some(RoomEnvironmentPrediction {
            environment: "forest".into(),
            confidence: 0.9,
            model_version: self.0.clone(),
        }))
    }
}

fn counting_factory(count: Arc<AtomicUsize>) -> service::ClassifierFactory {
    Arc::new(move |package: ModelPackage| {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(Fake(package.version)) as Arc<dyn RoomClassifier>)
    })
}

#[test]
fn install_from_a_folder_makes_the_service_ready_and_loads_the_classifier_lazily() {
    let data = temp_dir("data");
    let loads = Arc::new(AtomicUsize::new(0));
    let service = RoomClassificationService::with_factory(Some(&data), None, Some(counting_factory(loads.clone())));
    assert_eq!(service.status().state, ClassificationState::NotInstalled);
    assert!(service.try_get_classifier().is_none());
    let generation = service.generation();
    service.install_from(&fake_package("9.9.9", &[], None));
    assert_eq!(service.status().state, ClassificationState::Ready);
    assert_eq!(service.status().version.as_deref(), Some("9.9.9"));
    assert!(service.generation() > generation);
    // Ready, but nothing loaded until a worker asks.
    assert_eq!(loads.load(Ordering::SeqCst), 0);
    let first = service.try_get_classifier().unwrap();
    assert_eq!(first.model_version(), "9.9.9");
    assert!(Arc::ptr_eq(&first, &service.try_get_classifier().unwrap()));
    assert_eq!(loads.load(Ordering::SeqCst), 1);
    assert!(data.join("models/room-classifier/9.9.9/manifest.json").is_file());
    // A new service finds the installed package by its manifest, and still loads nothing.
    let again = RoomClassificationService::with_factory(Some(&data), None, Some(counting_factory(loads.clone())));
    assert_eq!(again.model_version().as_deref(), Some("9.9.9"));
    assert_eq!(loads.load(Ordering::SeqCst), 1);
}

#[cfg(feature = "classifier")]
#[test]
fn install_from_a_zip_extracts_verifies_and_refuses_unsafe_entries() {
    use crate::mudlet::zip::writer::{Entry, build};
    let source = fake_package("9.9.9", &[], None);
    let mut entries = Vec::new();
    for item in std::fs::read_dir(&source).unwrap() {
        let path = item.unwrap().path();
        entries.push((
            path.file_name().unwrap().to_string_lossy().into_owned(),
            std::fs::read(&path).unwrap(),
        ));
    }
    let zip = |entries: &[(String, Vec<u8>)]| {
        build(
            &entries
                .iter()
                .map(|(n, d)| Entry {
                    name: n,
                    data: d.clone(),
                    deflate: true,
                    declared: None,
                })
                .collect::<Vec<_>>(),
        )
    };
    let dir = temp_dir("zip");
    let good = dir.join("package.zip");
    std::fs::write(&good, zip(&entries)).unwrap();
    let data = temp_dir("data");
    let service = RoomClassificationService::with_factory(Some(&data), None, Some(counting_factory(Arc::default())));
    service.install_from(&good);
    assert_eq!(service.status().state, ClassificationState::Ready);
    entries.push(("../evil".into(), b"x".to_vec()));
    let evil = dir.join("evil.zip");
    std::fs::write(&evil, zip(&entries)).unwrap();
    service.install_from(&evil);
    assert_eq!(service.status().state, ClassificationState::Failed);
    assert!(!data.join("models/evil").exists());
}

#[test]
fn a_failed_install_reports_failure() {
    let data = temp_dir("data");
    let service = RoomClassificationService::with_factory(Some(&data), None, Some(counting_factory(Arc::default())));
    let bad = data.join("bad.zip");
    std::fs::write(&bad, "not a zip").unwrap();
    service.install_from(&bad);
    let status = service.status();
    assert_eq!(status.state, ClassificationState::Failed);
    assert!(status.message.is_some());
    assert!(service.try_get_classifier().is_none());
    // A later install still runs.
    service.install_from(&fake_package("9.9.9", &[], None));
    assert_eq!(service.status().state, ClassificationState::Ready);
}

#[test]
fn a_factory_failure_is_reported_as_failed() {
    let data = temp_dir("data");
    let factory: service::ClassifierFactory = Arc::new(|_| Err("boom".into()));
    let service = RoomClassificationService::with_factory(Some(&data), None, Some(factory));
    service.install_from(&fake_package("9.9.9", &[], None));
    assert_eq!(service.status().state, ClassificationState::Ready);
    assert!(service.try_get_classifier().is_none());
    assert_eq!(service.status().state, ClassificationState::Failed);
    assert_eq!(service.status().message.as_deref(), Some("boom"));
}

#[test]
fn for_testing_is_ready_at_once_and_a_build_without_a_classifier_is_unavailable() {
    let service = RoomClassificationService::for_testing(Arc::new(Fake("t".into())));
    assert_eq!(service.status().state, ClassificationState::Ready);
    assert_eq!(service.try_get_classifier().unwrap().model_version(), "t");
    let none = RoomClassificationService::with_factory(Some(&temp_dir("data")), None, None);
    assert_eq!(none.status().state, ClassificationState::Unavailable);
    assert!(none.model_version().is_none());
}

#[test]
fn the_development_package_folder_is_used_as_it_is() {
    let package = fake_package("9.9.9", &[], None);
    let service = RoomClassificationService::with_factory(None, Some(package), Some(counting_factory(Arc::default())));
    assert_eq!(service.model_version().as_deref(), Some("9.9.9"));
    assert!(service.try_get_classifier().is_some());
}

// ---- RoomInferenceTests -----------------------------------------------------------------------

fn at(id: &str, description: &str, environment: Option<&str>) -> RoomObservation {
    let mut o = RoomObservation::new(Some(id), "Room", description, &[]).with_source(RoomSource::Gmcp);
    o.environment = environment.map(str::to_string);
    o
}

const PINES: &str = "Tall pines crowd the trail.";

fn prediction(environment: &str, confidence: f64) -> RoomEnvironmentPrediction {
    RoomEnvironmentPrediction {
        environment: environment.into(),
        confidence,
        model_version: "0.1.1".into(),
    }
}

#[test]
fn rooms_needing_inference_skip_server_terrain_and_fresh_keys() {
    let mut tracker = RoomMapTracker::new();
    tracker.observe(&at("1", PINES, None), None);
    tracker.observe(&at("2", PINES, Some("forest")), Some("north"));
    tracker.observe(&at("3", PINES, None), Some("north"));
    let ids = |t: &RoomMapTracker, v: &str| -> Vec<String> {
        t.rooms_needing_inference(v).iter().map(|r| r.id.clone()).collect()
    };
    assert_eq!(ids(&tracker, "0.1.1"), ["s:3", "s:1"]); // the current room first
    let key = inference_key("Room", PINES, "0.1.1");
    assert!(tracker.apply_inference("s:3", &key, "0.1.1", Some(&prediction("forest", 0.91))));
    assert_eq!(ids(&tracker, "0.1.1"), ["s:1"]);
    assert_eq!(tracker.rooms_needing_inference("0.2.0").len(), 2); // a new model version
}

#[test]
fn apply_inference_sets_fields_without_the_manual_flag_and_rejects_stale_keys() {
    let mut tracker = RoomMapTracker::new();
    tracker.observe(&at("1", PINES, None), None);
    let version = tracker.version();
    let key = inference_key("Room", PINES, "0.1.1");
    assert!(tracker.apply_inference("s:1", &key, "0.1.1", Some(&prediction("forest", 0.91))));
    let room = tracker.room("s:1").unwrap().clone();
    assert_eq!(room.inferred_environment.as_deref(), Some("forest"));
    assert_eq!(room.inferred_confidence, Some(0.91));
    assert_eq!(room.inferred_key.as_deref(), Some(key.as_str()));
    assert!(!room.is_manually_edited);
    assert!(room.environment.is_none());
    assert_eq!(tracker.version(), version + 1);
    assert!(!tracker.apply_inference("s:1", "stale", "0.1.1", Some(&prediction("cave", 0.95))));
    assert!(!tracker.apply_inference("missing", &key, "0.1.1", Some(&prediction("cave", 0.95))));
    assert_eq!(
        tracker.room("s:1").unwrap().inferred_environment.as_deref(),
        Some("forest")
    );
    assert_eq!(tracker.version(), version + 1);
    assert!(!tracker.can_undo(), "inference is not an edit");
}

#[test]
fn an_abstention_records_the_key_and_clears_the_terrain() {
    let mut tracker = RoomMapTracker::new();
    tracker.observe(&at("1", PINES, None), None);
    let key = inference_key("Room", PINES, "0.1.1");
    tracker.apply_inference("s:1", &key, "0.1.1", Some(&prediction("forest", 0.91)));
    assert!(tracker.apply_inference("s:1", &key, "0.1.1", None));
    let room = tracker.room("s:1").unwrap();
    assert!(room.inferred_environment.is_none() && room.inferred_confidence.is_none());
    assert_eq!(room.inferred_key.as_deref(), Some(key.as_str()));
    assert!(tracker.rooms_needing_inference("0.1.1").is_empty());
}

#[test]
fn reobservation_with_new_text_keeps_the_hint_but_needs_inference_again() {
    let mut tracker = RoomMapTracker::new();
    tracker.observe(&at("1", PINES, None), None);
    let key = inference_key("Room", PINES, "0.1.1");
    tracker.apply_inference("s:1", &key, "0.1.1", Some(&prediction("forest", 0.91)));
    tracker.observe(&at("1", "Endless dunes roll away.", None), None);
    assert_eq!(
        tracker.room("s:1").unwrap().inferred_environment.as_deref(),
        Some("forest")
    );
    let needing: Vec<&str> = tracker
        .rooms_needing_inference("0.1.1")
        .iter()
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(needing, ["s:1"]);
}

#[test]
fn inferred_fields_survive_the_map_file_and_are_validated() {
    use crate::map::format;
    let mut room = MapRoom::new("r", "Room", "Desc", None, 0.0, 0.0, 0.0, false);
    room.inferred_environment = Some("cave".into());
    room.inferred_confidence = Some(0.5);
    room.inferred_key = Some("k".into());
    let snapshot = |r: MapRoom| MapSnapshot::of(vec![r], Vec::new());
    let back = format::deserialize(&format::serialize(&snapshot(room.clone())).unwrap()).unwrap();
    assert_eq!(back.rooms[0], room);
    let mut bad = room.clone();
    bad.inferred_confidence = Some(1.5);
    assert!(format::serialize(&snapshot(bad)).is_err());
    let mut bad = room.clone();
    bad.inferred_key = Some("k".repeat(129));
    assert!(format::serialize(&snapshot(bad)).is_err());
    let mut bad = room;
    bad.inferred_environment = Some("e".repeat(129));
    assert!(format::serialize(&snapshot(bad)).is_err());
}

#[test]
fn settings_validate_the_threshold() {
    use crate::settings::Settings;
    let settings = Settings::default();
    assert!(settings.classify_rooms_locally);
    assert_eq!(settings.room_classification_threshold, 0.8);
    assert!(settings.validate_preferences().is_ok());
    for bad in [0.2, f64::NAN, 1.0] {
        let s = Settings {
            room_classification_threshold: bad,
            ..Settings::default()
        };
        assert!(s.validate_preferences().is_err(), "{bad}");
    }
    let s = Settings {
        room_classification_threshold: 0.99,
        ..Settings::default()
    };
    assert!(s.validate_preferences().is_ok());
}

// ---- the worker -------------------------------------------------------------------------------

/// A keyword classifier (the C# reference capture's stand-in) that records the threads it ran
/// on and counts its calls.
struct Keywords {
    calls: AtomicUsize,
    threads: std::sync::Mutex<Vec<std::thread::ThreadId>>,
}

impl Keywords {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            threads: std::sync::Mutex::new(Vec::new()),
        })
    }
}

impl RoomClassifier for Keywords {
    fn model_version(&self) -> &str {
        "fixture-1"
    }
    fn default_threshold(&self) -> f64 {
        0.5
    }
    fn classify(
        &self,
        name: &str,
        description: &str,
        threshold: f64,
    ) -> Result<Option<RoomEnvironmentPrediction>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.threads.lock().unwrap().push(std::thread::current().id());
        let text = format!("{name} {description}").to_lowercase();
        let pick = if text.contains("cave") || text.contains("grotto") {
            Some(("cave", 0.91))
        } else if text.contains("forest") || text.contains("oak") {
            Some(("forest", 0.87))
        } else if text.contains("road") {
            Some(("road", 0.74))
        } else {
            None
        };
        Ok(pick
            .filter(|p| p.1 >= threshold)
            .map(|(e, c)| RoomEnvironmentPrediction {
                environment: e.into(),
                confidence: c,
                model_version: "fixture-1".into(),
            }))
    }
}

fn worker(classifier: Arc<Keywords>) -> InferenceWorker {
    let service = Arc::new(RoomClassificationService::for_testing(classifier));
    InferenceWorker::spawn(service, Box::new(|| {})).unwrap()
}

/// Apply results until `done` holds (or two seconds pass), rescanning as the worker asks.
fn settle(
    worker: &mut InferenceWorker,
    tracker: &mut RoomMapTracker,
    threshold: f64,
    done: impl Fn(&RoomMapTracker) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(tracker) && Instant::now() < deadline {
        let (_, rescan) = worker.apply(tracker);
        if rescan {
            worker.schedule(tracker, "fixture-1", threshold);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn text_map(rooms: &[(&str, &str, &str, Option<&str>)]) -> RoomMapTracker {
    let rooms = rooms
        .iter()
        .enumerate()
        .map(|(i, (id, name, description, environment))| {
            let mut r = MapRoom::new(id, name, description, None, i as f64, 0.0, 0.0, false);
            r.environment = environment.map(str::to_string);
            r
        })
        .collect();
    RoomMapTracker::from_snapshot(MapSnapshot::of(rooms, Vec::new()))
}

#[test]
fn the_worker_classifies_off_the_calling_thread_and_server_terrain_wins() {
    let classifier = Keywords::new();
    let mut worker = worker(classifier.clone());
    let mut tracker = text_map(&[
        ("r1", "Lantern Crossroads", "Four roads meet.", None),
        ("r2", "Edge of Hollowwood", "Tall oaks crowd the road.", None),
        ("r3", "Hidden Grotto", "A damp cave.", Some("water")),
        ("r4", "Void", "Nothing.", None),
    ]);
    assert_eq!(worker.schedule(&tracker, "fixture-1", 0.5), 3);
    settle(&mut worker, &mut tracker, 0.5, |t| {
        t.rooms().filter(|r| r.inferred_key.is_some()).count() == 3
    });
    let inferred = |id: &str| tracker.room(id).unwrap().inferred_environment.clone();
    assert_eq!(inferred("r1").as_deref(), Some("road"));
    assert_eq!(inferred("r2").as_deref(), Some("forest"));
    assert_eq!(inferred("r3"), None, "server terrain is never classified");
    assert_eq!(inferred("r4"), None, "an abstention");
    assert!(tracker.room("r4").unwrap().inferred_key.is_some());
    let caller = std::thread::current().id();
    assert!(classifier.threads.lock().unwrap().iter().all(|t| *t != caller));
    // Everything is checked now: another scan offers nothing.
    assert_eq!(worker.schedule(&tracker, "fixture-1", 0.5), 0);
    assert_eq!(classifier.calls.load(Ordering::SeqCst), 3);
}

#[test]
fn a_result_for_text_that_changed_meanwhile_is_dropped() {
    let mut tracker = text_map(&[("r1", "Old Road", "A road.", None)]);
    let stale = InferenceResult {
        id: "r1".into(),
        name: "Old Road".into(),
        description: "A road.".into(),
        key: "k".into(),
        model_version: "fixture-1".into(),
        prediction: Some(RoomEnvironmentPrediction {
            environment: "road".into(),
            confidence: 0.74,
            model_version: "fixture-1".into(),
        }),
    };
    let mut renamed = tracker.room("r1").unwrap().clone();
    renamed.description = "A cave now.".into();
    assert!(tracker.upsert_room(renamed));
    assert!(!tracker.apply_inference_result(&stale));
    assert!(tracker.room("r1").unwrap().inferred_key.is_none());
}

#[test]
fn rooms_already_inferred_are_not_classified_again_after_a_reload() {
    let classifier = Keywords::new();
    let mut worker = worker(classifier.clone());
    let mut tracker = text_map(&[("r1", "Old Road", "A road.", None), ("r2", "Grotto", "Wet.", None)]);
    worker.schedule(&tracker, "fixture-1", 0.5);
    settle(&mut worker, &mut tracker, 0.5, |t| {
        t.rooms().all(|r| r.inferred_key.is_some())
    });
    assert_eq!(classifier.calls.load(Ordering::SeqCst), 2);
    // The saved map comes back in a new session: keys are checked on the worker, nothing runs.
    let mut reloaded = RoomMapTracker::from_snapshot(tracker.snapshot());
    let mut fresh_worker = self::worker(classifier.clone());
    assert_eq!(fresh_worker.schedule(&reloaded, "fixture-1", 0.5), 2);
    let deadline = Instant::now() + Duration::from_secs(2);
    while fresh_worker.schedule(&reloaded, "fixture-1", 0.5) > 0 && Instant::now() < deadline {
        fresh_worker.apply(&mut reloaded);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(classifier.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn more_rooms_than_the_queue_holds_are_all_classified_through_rescans() {
    let classifier = Keywords::new();
    let mut worker = worker(classifier.clone());
    let rooms: Vec<(String, String)> = (0..600).map(|i| (format!("r{i}"), format!("Road {i}"))).collect();
    let list: Vec<(&str, &str, &str, Option<&str>)> = rooms
        .iter()
        .map(|(id, name)| (id.as_str(), name.as_str(), "A road.", None))
        .collect();
    let mut tracker = text_map(&list);
    assert_eq!(worker.schedule(&tracker, "fixture-1", 0.5), worker::QUEUE_LIMIT);
    settle(&mut worker, &mut tracker, 0.5, |t| {
        t.rooms().all(|r| r.inferred_key.is_some())
    });
    assert!(
        tracker
            .rooms()
            .all(|r| r.inferred_environment.as_deref() == Some("road"))
    );
    assert_eq!(classifier.calls.load(Ordering::SeqCst), 600);
}

#[test]
fn the_current_room_goes_first_and_reset_drops_waiting_work() {
    let classifier = Keywords::new();
    let mut worker = worker(classifier);
    let mut tracker = text_map(&[("a", "Road", "", None), ("b", "Grotto", "", None)]);
    tracker.set_current_room("b");
    let jobs: Vec<InferenceJob> = tracker.inference_candidates().map(InferenceJob::of).collect();
    assert_eq!(jobs[0].id, "b");
    worker.reset();
    assert_eq!(worker.pending(), 0);
    assert!(worker.drain().results.is_empty());
}

// ---- the package's encoder (gated on a local copy of the 0.1.1 package) -----------------------

/// `WANDUR_ROOM_MODEL_DIR`, else the copy under `.superpowers/models` (not in git).
#[cfg(feature = "classifier")]
fn model_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("WANDUR_ROOM_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.superpowers/models/room-classifier/0.1.1")
        });
    dir.join("manifest.json").is_file().then_some(dir)
}

#[cfg(feature = "classifier")]
#[test]
fn the_onnx_classifier_reproduces_every_parity_fixture() {
    let Some(dir) = model_dir() else {
        eprintln!("skipped: no classifier package (set WANDUR_ROOM_MODEL_DIR)");
        return;
    };
    let started = Instant::now();
    let classifier = onnx::OnnxRoomClassifier::new(ModelPackage::load(&dir).unwrap()).unwrap();
    eprintln!("package verified and encoder loaded in {:?}", started.elapsed());
    let fixtures: Value = serde_json::from_str(FIXTURE).unwrap();
    let atol_probs = fixtures["atol_probs"].as_f64().unwrap();
    let atol_embedding = fixtures["atol_embedding"].as_f64().unwrap();
    let started = Instant::now();
    let mut worst = (0f64, 0f64);
    for f in fixtures["fixtures"].as_array().unwrap() {
        let text = f["text"].as_str().unwrap();
        let ids: Vec<u32> = f["token_ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as u32)
            .collect();
        assert_eq!(classifier.tokenize(text), ids, "{}", f["id"]);
        let embedding = classifier.embed(text).unwrap();
        for (a, b) in embedding.iter().zip(f["embedding"].as_array().unwrap()) {
            let d = (f64::from(*a) - b.as_f64().unwrap()).abs();
            worst.0 = worst.0.max(d);
            assert!(d <= atol_embedding, "{} embedding off by {d}", f["id"]);
        }
        let probabilities = classifier.probabilities(text).unwrap();
        for (p, e) in probabilities.iter().zip(f["probs"].as_array().unwrap()) {
            let d = (f64::from(*p) - e.as_f64().unwrap()).abs();
            worst.1 = worst.1.max(d);
            assert!(d <= atol_probs, "{} probability off by {d}", f["id"]);
        }
        let prediction = classifier
            .classify(f["name"].as_str().unwrap(), f["description"].as_str().unwrap(), 0.0)
            .unwrap()
            .unwrap();
        assert_eq!(prediction.environment, f["predicted"].as_str().unwrap());
    }
    eprintln!(
        "23 fixtures in {:?}; largest differences: embedding {:.2e}, probability {:.2e}",
        started.elapsed(),
        worst.0,
        worst.1
    );
}

#[cfg(feature = "classifier")]
#[test]
fn the_onnx_classifier_abstains_below_the_threshold() {
    let Some(dir) = model_dir() else {
        return;
    };
    let classifier = onnx::OnnxRoomClassifier::new(ModelPackage::load(&dir).unwrap()).unwrap();
    assert!(classifier.classify("Void", "Nothing.", 1.0).unwrap().is_none());
    assert!(classifier.classify("Void", "Nothing.", 0.0).unwrap().is_some());
    assert_eq!(classifier.model_version(), "0.1.1");
    assert_eq!(classifier.default_threshold(), 0.8);
}
