//! The import dialog's own parts: reading on a worker thread with progress, applying (merge for
//! Mudlet, replace for this client's file), and an import into a saved world's map.

use super::*;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wandur-core/tests/fixtures/mudlet/lantern-road-map.json")
}

fn dialog() -> MapImportDialog {
    let choices = vec![WorldChoice {
        label: "Lantern Road".into(),
        target: Target::Session(1),
    }];
    MapImportDialog::new(choices, 5, Arc::new(|| {}))
}

fn wait(dialog: &mut MapImportDialog) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while dialog.busy() {
        assert!(std::time::Instant::now() < deadline, "timed out");
        std::thread::sleep(std::time::Duration::from_millis(5));
        dialog.poll();
    }
}

#[test]
fn a_file_is_read_on_a_worker_thread_into_a_summary() {
    let mut d = dialog();
    assert_eq!(d.chosen, 0, "an out of range choice falls back");
    d.read_file(&fixture());
    assert!(d.busy());
    wait(&mut d);
    let prepared = d.prepared().expect("read");
    assert_eq!(prepared.summary.rooms, 8);
    assert!(!prepared.replaces());
    d.read_file(Path::new("/nonexistent/map.json"));
    wait(&mut d);
    assert!(d.prepared().is_none() && d.error().is_some());
}

#[test]
fn progress_reads_as_a_line_and_a_fraction() {
    wandur_core::l10n::override_thread(Some(wandur_core::l10n::Language::En));
    let (text, fraction) = progress_text(Some(Progress {
        stage: Stage::Rooms,
        done: 500,
        total: 2000,
    }));
    assert_eq!(text, "Reading rooms: 500 of 2000");
    assert_eq!(fraction, Some(0.25));
    assert_eq!(progress_text(None), ("Reading the map…".to_string(), None));
    wandur_core::l10n::override_thread(None);
}

#[test]
fn a_wandur_file_replaces_and_a_mudlet_file_merges() {
    let mudlet = read_path(&fixture(), &|_| {}).unwrap();
    let mut tracker = RoomMapTracker::from_snapshot(wandur_core::map::MapSnapshot::of(
        vec![wandur_core::map::MapRoom::new(
            "mine", "Mine", "", None, 0.0, 0.0, 0.0, false,
        )],
        Vec::new(),
    ));
    assert!(apply_to(&mut tracker, &mudlet).unwrap() > 0);
    assert_eq!(tracker.room_count(), 9, "merged beside the map's own room");
    let ours = wandur_core::map::format::serialize(&tracker.snapshot()).unwrap();
    let mut other = RoomMapTracker::from_snapshot(wandur_core::map::MapSnapshot::of(
        vec![wandur_core::map::MapRoom::new(
            "else", "Else", "", None, 0.0, 0.0, 0.0, false,
        )],
        Vec::new(),
    ));
    let replacing = wandur_core::map::mudlet::read(Some("json"), ours.as_bytes(), &|_| {}).unwrap();
    assert!(replacing.replaces());
    apply_to(&mut other, &replacing).unwrap();
    assert_eq!(other.room_count(), 9);
    assert!(other.room("else").is_none(), "replaced");
    assert_eq!(other.label_count(), 2);
    assert!(other.undo());
    assert!(other.room("else").is_some());
}

#[test]
fn an_import_into_a_saved_world_is_merged_and_saved() {
    let dir = std::env::temp_dir().join(format!("wandur-map-import-{}", uuid::Uuid::new_v4().simple()));
    let (db, _) = Database::open(&dir).unwrap();
    let store = MapStore::new(db);
    let world = MapWorld::Id(wandur_core::db::worlds::new_world_id());
    let prepared = read_path(&fixture(), &|_| {}).unwrap();
    let done = import_detached(&store, world.clone(), &prepared).unwrap();
    assert_eq!(done.changed, 8 + 12 + 2 + 1);
    assert!(done.tracker.can_undo());
    assert_eq!(store.load(&world).unwrap().unwrap().rooms.len(), 8);
    // Again: nothing changes and nothing is saved.
    let again = import_detached(&store, world.clone(), &prepared).unwrap();
    assert_eq!(again.changed, 0);
    let _ = std::fs::remove_dir_all(dir);
}
