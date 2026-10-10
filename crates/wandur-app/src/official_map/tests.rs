//! The offer's states with a fake server: asked for a new map, Not now for the session, Never
//! saved for the world, an unchanged imported map not asked again, the server asked at most once
//! a day unless the address changed, a changed map asked with the file already fetched,
//! download failures and the file kept as the next base.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::*;

const URL: &str = "https://maps.fixture.example/vale/map.xml";
const V1: &str = r#"<map><areas><area id="1" name="Vale"/></areas><rooms><room id="1" area="1" title="Gate"><coord x="0" y="0" z="0"/></room></rooms></map>"#;
const V2: &str = r#"<map><areas><area id="1" name="Vale"/></areas><rooms><room id="1" area="1" title="Gate"><coord x="0" y="0" z="0"/></room><room id="2" area="1" title="Well"><coord x="1" y="0" z="0"/></room></rooms></map>"#;

struct Server {
    calls: AtomicUsize,
    /// The validators sent, in order.
    sent: Mutex<Vec<Validators>>,
    answer: Mutex<Result<Downloaded, DownloadError>>,
}

fn server(answer: Result<Downloaded, DownloadError>) -> (Arc<Server>, Fetch) {
    let server = Arc::new(Server {
        calls: AtomicUsize::new(0),
        sent: Mutex::new(Vec::new()),
        answer: Mutex::new(answer),
    });
    let s = Arc::clone(&server);
    let fetch: Fetch = Arc::new(move |_: &str, known: &Validators| {
        s.calls.fetch_add(1, Ordering::SeqCst);
        s.sent.lock().unwrap().push(known.clone());
        s.answer.lock().unwrap().clone()
    });
    (server, fetch)
}

fn file(text: &str, etag: &str) -> Result<Downloaded, DownloadError> {
    Ok(Downloaded::File {
        bytes: text.as_bytes().to_vec(),
        etag: Some(etag.into()),
        last_modified: None,
    })
}

/// Make the last check of the world two days old.
fn age_check(store: &OfficialStore) {
    let record = store.load("world");
    let old = store::time_text(store::now_secs() - 2 * offer::CHECK_INTERVAL_SECS);
    store
        .save(
            "world",
            &Record {
                last_checked_at: Some(old),
                ..record
            },
        )
        .unwrap();
}

fn temp() -> (std::path::PathBuf, OfficialStore) {
    let dir = std::env::temp_dir().join(format!("wandur-official-app-{}", uuid::Uuid::new_v4().simple()));
    let store = OfficialStore::new(&dir);
    (dir, store)
}

fn settle(map: &mut OfficialMap) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        map.poll();
        if !map.busy() {
            return;
        }
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn offer(store: &OfficialStore, not_now: bool, fetch: &Fetch) -> OfficialMap {
    offer_at(URL, store, not_now, fetch)
}

fn offer_at(url: &str, store: &OfficialStore, not_now: bool, fetch: &Fetch) -> OfficialMap {
    let mut map = OfficialMap::new(url, "world", store.clone(), not_now, Arc::clone(fetch), Arc::new(|| {}));
    settle(&mut map);
    map
}

/// Download, merge into a fresh tracker over its base, keep the file: what the app does.
fn download_and_keep(map: &mut OfficialMap, tracker: &mut wandur_core::map::RoomMapTracker) -> MergeReport {
    map.download();
    settle(map);
    let ready = map.take_ready().expect("ready");
    let report = wandur_core::map::official::merge::merge(tracker, ready.base.as_ref(), &ready.prepared.map).unwrap();
    map.keep(ready, report);
    settle(map);
    report
}

#[test]
fn a_new_map_is_asked_for_and_not_now_lasts_the_session() {
    let (dir, store) = temp();
    let (server, fetch) = server(file(V1, "\"v1\""));
    let mut map = offer(&store, false, &fetch);
    assert!(matches!(map.phase, Phase::Asking) && map.visible());
    assert_eq!(
        server.calls.load(Ordering::SeqCst),
        0,
        "nothing is fetched before Download"
    );
    map.not_now();
    assert!(!map.visible() && map.not_now);
    // The server names it again in the same session: still silent.
    let again = offer(&store, map.not_now, &fetch);
    assert!(!again.visible());
    // A new session asks again.
    assert!(offer(&store, false, &fetch).visible());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn never_is_saved_for_the_world() {
    let (dir, store) = temp();
    let (_, fetch) = server(file(V1, "\"v1\""));
    let mut map = offer(&store, false, &fetch);
    map.never().unwrap();
    assert!(!map.visible());
    assert!(store.load("world").never);
    assert!(!offer(&store, false, &fetch).visible(), "a later session stays silent");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn download_imports_and_keeps_the_file_and_an_unchanged_map_does_not_nag() {
    let (dir, store) = temp();
    let (server, fetch) = server(file(V1, "\"v1\""));
    let mut tracker = wandur_core::map::RoomMapTracker::new();
    let mut map = offer(&store, false, &fetch);
    let report = download_and_keep(&mut map, &mut tracker);
    assert_eq!(report.added, 1);
    assert_eq!(tracker.room("s:1").unwrap().name, "Gate");
    let record = store.load("world");
    assert_eq!(record.url.as_deref(), Some(URL));
    assert_eq!(record.etag.as_deref(), Some("\"v1\""));
    assert_eq!(record.file.as_deref(), Some("map.xml"));
    assert_eq!(record.report, Some(report));
    assert!(dir.join("official-maps/world/map.xml").exists());
    // A session the next day: the server says 304.
    age_check(&store);
    *server.answer.lock().unwrap() = Ok(Downloaded::NotModified);
    let quiet = offer(&store, false, &fetch);
    assert!(!quiet.visible());
    assert_eq!(
        server.sent.lock().unwrap().last().unwrap().etag.as_deref(),
        Some("\"v1\"")
    );
    // Or the same bytes again, without a 304.
    age_check(&store);
    *server.answer.lock().unwrap() = file(V1, "\"v1b\"");
    assert!(!offer(&store, false, &fetch).visible());
    assert_eq!(
        store.load("world").etag.as_deref(),
        Some("\"v1b\""),
        "the new ETag is kept"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_changed_map_is_asked_for_with_the_file_already_fetched_and_merged_over_its_base() {
    let (dir, store) = temp();
    let (server, fetch) = server(file(V1, "\"v1\""));
    let mut tracker = wandur_core::map::RoomMapTracker::new();
    download_and_keep(&mut offer(&store, false, &fetch), &mut tracker);
    // The person renames the gate.
    let mut gate = tracker.room("s:1").unwrap().clone();
    gate.name = "My gate".into();
    assert!(tracker.upsert_room(gate));
    *server.answer.lock().unwrap() = file(V2, "\"v2\"");
    age_check(&store);
    let mut map = offer(&store, false, &fetch);
    assert!(matches!(map.phase, Phase::Asking));
    let calls = server.calls.load(Ordering::SeqCst);
    map.download();
    settle(&mut map);
    assert_eq!(server.calls.load(Ordering::SeqCst), calls, "the checked file is used");
    let ready = map.take_ready().unwrap();
    assert!(ready.base.is_some(), "the file imported before is the base");
    let report =
        wandur_core::map::official::merge::merge(&mut tracker, ready.base.as_ref(), &ready.prepared.map).unwrap();
    assert_eq!(report.added, 1);
    assert_eq!(tracker.room("s:1").unwrap().name, "My gate", "the edit stays");
    assert_eq!(tracker.room("s:2").unwrap().name, "Well");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_server_is_asked_at_most_once_a_day_unless_the_address_changes() {
    let (dir, store) = temp();
    let (server, fetch) = server(file(V1, "\"v1\""));
    let mut tracker = wandur_core::map::RoomMapTracker::new();
    download_and_keep(&mut offer(&store, false, &fetch), &mut tracker);
    let calls = server.calls.load(Ordering::SeqCst);
    // Sessions within the day: no request at all.
    *server.answer.lock().unwrap() = file(V2, "\"v2\"");
    assert!(!offer(&store, false, &fetch).visible());
    assert!(!offer(&store, false, &fetch).visible());
    assert_eq!(server.calls.load(Ordering::SeqCst), calls, "no request inside 24 hours");
    // The game names a new address: asked at once, with nothing to validate against there.
    let moved = "https://maps.fixture.example/vale/map-2.xml";
    *server.answer.lock().unwrap() = file(V1, "\"m1\"");
    assert!(
        !offer_at(moved, &store, false, &fetch).visible(),
        "the same file, moved"
    );
    assert_eq!(server.calls.load(Ordering::SeqCst), calls + 1);
    assert_eq!(server.sent.lock().unwrap().last().unwrap(), &Validators::default());
    let record = store.load("world");
    assert_eq!(
        record.url.as_deref(),
        Some(moved),
        "the file is known at its new address"
    );
    assert_eq!(record.last_checked_url.as_deref(), Some(moved));
    // A change found by a check is still offered within the day, without asking again.
    age_check(&store);
    *server.answer.lock().unwrap() = file(V2, "\"m2\"");
    let mut first = offer_at(moved, &store, false, &fetch);
    assert!(first.visible());
    first.not_now();
    let calls = server.calls.load(Ordering::SeqCst);
    let later = offer_at(moved, &store, false, &fetch);
    assert!(matches!(later.phase, Phase::Asking));
    assert_eq!(server.calls.load(Ordering::SeqCst), calls);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_failed_or_needless_download_says_why() {
    wandur_core::l10n::override_thread(Some(wandur_core::l10n::Language::En));
    let (dir, store) = temp();
    let (server, fetch) = server(Err(DownloadError::Status(404)));
    let mut map = offer(&store, false, &fetch);
    map.download();
    settle(&mut map);
    let Phase::Message(text) = &map.phase else {
        panic!("a message");
    };
    assert_eq!(text, "The official map could not be imported: The server answered 404.");
    assert!(map.visible());
    map.dismiss();
    assert!(!map.visible());
    // Not a map at all.
    *server.answer.lock().unwrap() = file("<html/>", "\"x\"");
    let mut map = offer(&store, false, &fetch);
    map.download();
    settle(&mut map);
    assert!(matches!(&map.phase, Phase::Message(t) if t.starts_with("The official map could not be imported")));
    wandur_core::l10n::override_thread(None);
    let _ = std::fs::remove_dir_all(dir);
}
