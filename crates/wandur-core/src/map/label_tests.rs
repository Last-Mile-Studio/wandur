//! Labels and their pictures: the tracker's editing calls and undo, merging, the map file and
//! the store; and bringing an imported map in ([`RoomMapTracker::import_map`]).

use super::format::{deserialize, serialize, validate};
use super::images::{self, MAX_IMAGE_BYTES, test_png};
use super::merge::combine;
use super::model::*;
use super::store::{MapStore, MapWorld};
use super::tracker::RoomMapTracker;

fn room(id: &str) -> MapRoom {
    MapRoom::new(id, id, "", Some("Keep"), 0.0, 0.0, 0.0, false)
}

fn picture(w: u32) -> MapImage {
    images::image(test_png(w, 2))
}

fn text_label(id: &str) -> MapLabel {
    MapLabel::text(id, Some("Keep"), 1.0, 2.0, 0.0, "Hello")
}

fn picture_label(id: &str, image: &MapImage) -> MapLabel {
    MapLabel {
        image: Some(image.hash.clone()),
        ..MapLabel::text(id, Some("Keep"), 0.0, 0.0, 0.0, "")
    }
}

/// Bytes that pass for a PNG without being decoded (the size checks do not decode).
fn fake_png(len: usize, seed: u8) -> MapImage {
    let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
    data.resize(len, seed);
    images::image(data)
}

#[test]
fn labels_are_edited_moved_deleted_and_undone_with_their_pictures() {
    let mut t = RoomMapTracker::from_snapshot(MapSnapshot::of(vec![room("a")], Vec::new()));
    assert!(t.upsert_label(text_label("note")));
    let image = picture(5);
    // A picture label needs its picture held first.
    assert!(!t.upsert_label(picture_label("pic", &image)));
    assert!(t.add_image(image.clone()));
    assert!(t.upsert_label(picture_label("pic", &image)));
    assert_eq!(t.label_count(), 2);
    assert!(t.label("pic").unwrap().is_manually_edited);
    let snapshot = t.snapshot();
    assert_eq!(snapshot.images, vec![image.clone()]);
    validate(&snapshot).unwrap();
    // Move, then delete: each one undo step; the tombstone stays.
    let moved = MapLabel {
        x: 7.0,
        ..t.label("pic").unwrap().clone()
    };
    assert!(t.upsert_label(moved));
    assert!(t.remove_label("pic"));
    assert!(t.label("pic").is_none());
    assert!(t.image(&image.hash).is_none(), "a picture no label shows is forgotten");
    assert!(t.snapshot().deleted_labels.iter().any(|d| d.id == "pic"));
    assert!(t.undo());
    assert_eq!(t.label("pic").unwrap().x, 7.0);
    assert_eq!(t.image(&image.hash), Some(&image), "undo brings the picture back");
    assert!(t.undo());
    assert_eq!(t.label("pic").unwrap().x, 0.0);
    assert!(t.redo() && t.redo());
    assert!(t.label("pic").is_none());
    // A blank text label is refused; a bad colour too.
    assert!(!t.upsert_label(MapLabel::text("blank", None, 0.0, 0.0, 0.0, "  ")));
    assert!(!t.upsert_label(MapLabel {
        color: Some("red".into()),
        ..text_label("bad")
    }));
}

#[test]
fn pictures_stay_within_the_maps_limit() {
    let mut t = RoomMapTracker::new();
    for i in 0..10u8 {
        let image = fake_png(MAX_IMAGE_BYTES, i);
        assert!(t.add_image(image.clone()));
        assert!(t.upsert_label(picture_label(&format!("l{i}"), &image)), "{i}");
    }
    assert_eq!(t.image_bytes(), 10 * MAX_IMAGE_BYTES);
    let one_more = fake_png(1024, 99);
    assert!(t.add_image(one_more.clone()));
    assert!(!t.upsert_label(picture_label("over", &one_more)));
    // The same picture again costs nothing.
    let again = fake_png(MAX_IMAGE_BYTES, 3);
    assert!(t.upsert_label(picture_label("again", &again)));
    assert!(!t.add_image(fake_png(MAX_IMAGE_BYTES + 1, 1)));
}

#[test]
fn merging_keeps_the_newer_label_and_its_tombstones() {
    let image = picture(4);
    let old = MapSnapshot {
        labels: vec![
            MapLabel {
                revision: 5,
                ..text_label("a")
            },
            MapLabel {
                revision: 5,
                ..picture_label("p", &image)
            },
        ],
        images: vec![image.clone()],
        ..MapSnapshot::default()
    };
    let newer = MapSnapshot {
        labels: vec![MapLabel {
            revision: 9,
            text: "Changed".into(),
            ..text_label("a")
        }],
        deleted_labels: vec![MapLabelDeletion {
            id: "p".into(),
            revision: 8,
        }],
        ..MapSnapshot::default()
    };
    let merged = combine(&old, &newer);
    assert_eq!(merged.labels.len(), 1);
    assert_eq!(merged.labels[0].text, "Changed");
    assert!(merged.images.is_empty(), "no merged label shows the picture");
    // A stale save cannot bring the deleted label back.
    let again = combine(&merged, &old);
    assert_eq!(again.labels.len(), 1);
    assert_eq!(again.labels[0].text, "Changed");
    // A picture is kept from whichever save holds it.
    let shown = combine(
        &MapSnapshot {
            images: vec![image.clone()],
            ..MapSnapshot::default()
        },
        &MapSnapshot {
            labels: vec![MapLabel {
                revision: 3,
                ..picture_label("q", &image)
            }],
            ..MapSnapshot::default()
        },
    );
    assert_eq!(shown.images, vec![image]);
    validate(&shown).unwrap();
}

#[test]
fn map_files_carry_labels_as_version_2_and_check_their_pictures() {
    let plain = MapSnapshot::of(vec![room("a")], Vec::new());
    let text = serialize(&plain).unwrap();
    assert!(
        text.starts_with("{\"Version\":1,") && !text.contains("Labels"),
        "{text}"
    );
    let image = picture(3);
    let labelled = MapSnapshot {
        labels: vec![text_label("t"), picture_label("p", &image)],
        images: vec![image.clone()],
        ..plain.clone()
    };
    let text = serialize(&labelled).unwrap();
    assert!(text.starts_with("{\"Version\":2,"));
    assert!(text.contains(&images::encode_base64(&image.data)));
    assert_eq!(deserialize(&text).unwrap(), labelled);
    // A picture whose bytes are not its hash's, a missing picture, a third version.
    let other = picture(9);
    let tampered = text.replace(&images::encode_base64(&image.data), &images::encode_base64(&other.data));
    assert!(deserialize(&tampered).is_err());
    let missing = MapSnapshot {
        images: Vec::new(),
        ..labelled.clone()
    };
    assert!(validate(&missing).is_err());
    assert!(deserialize(&text.replacen("\"Version\":2", "\"Version\":3", 1)).is_err());
}

fn temp_db(name: &str) -> (crate::db::Database, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("wandur-labels-{name}-{}", uuid::Uuid::new_v4().simple()));
    let (db, _) = crate::db::Database::open(&dir).unwrap();
    (db, dir)
}

#[test]
fn the_store_keeps_each_picture_once_and_drops_unused_ones() {
    let (db, dir) = temp_db("store");
    let store = MapStore::new(db.clone());
    let world = MapWorld::Id(crate::db::worlds::new_world_id());
    let image = picture(7);
    let map = MapSnapshot {
        labels: vec![
            MapLabel {
                revision: 1,
                ..picture_label("one", &image)
            },
            MapLabel {
                revision: 1,
                ..picture_label("two", &image)
            },
        ],
        images: vec![image.clone()],
        ..MapSnapshot::of(vec![room("a")], Vec::new())
    };
    store.save(&world, &map).unwrap();
    let count = || -> i64 {
        db.read(|c| Ok(c.query_row("SELECT COUNT(*) FROM map_images", [], |r| r.get(0))?))
            .unwrap()
    };
    assert_eq!(count(), 1);
    let loaded = store.load(&world).unwrap().unwrap();
    assert_eq!(loaded.labels.len(), 2);
    assert_eq!(loaded.images, vec![image.clone()]);
    // Both labels deleted (newer tombstones): the picture goes with them.
    let gone = MapSnapshot {
        labels: Vec::new(),
        deleted_labels: vec![
            MapLabelDeletion {
                id: "one".into(),
                revision: 5,
            },
            MapLabelDeletion {
                id: "two".into(),
                revision: 5,
            },
        ],
        images: Vec::new(),
        ..map
    };
    store.save(&world, &gone).unwrap();
    assert_eq!(count(), 0);
    assert!(store.load(&world).unwrap().unwrap().labels.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

fn imported() -> MapSnapshot {
    let image = picture(6);
    let mut a = MapRoom::new("mudlet:1", "Gate", "", Some("Town"), 0.0, 0.0, 0.0, false);
    a.color = Some("#800000".into());
    let b = MapRoom::new("mudlet:2", "Square", "", Some("Town"), 0.0, 1.0, 0.0, false);
    MapSnapshot {
        labels: vec![
            text_label("mudlet:label:1:0"),
            picture_label("mudlet:label:1:1", &image),
        ],
        images: vec![image],
        area_settings: vec![MapAreaSettings::new("Town", true)],
        ..MapSnapshot::of(
            vec![a, b],
            vec![
                MapLink::new("mudlet:1", "mudlet:2", "north", true),
                MapLink::new("mudlet:2", "mudlet:1", "south", true),
            ],
        )
    }
}

#[test]
fn an_import_merges_as_one_undo_step_and_again_changes_nothing() {
    let mut observed = MapRoom::new(
        "mudlet:1",
        "Old Gate",
        "Seen by the server.",
        None,
        0.0,
        0.0,
        0.0,
        false,
    );
    observed.inferred_environment = Some("city".into());
    let mut t = RoomMapTracker::from_snapshot(MapSnapshot::of(vec![observed, room("mine")], Vec::new()));
    let changed = t.import_map(&imported()).unwrap();
    assert_eq!(changed, 2 + 2 + 2 + 1);
    assert_eq!(t.room_count(), 3, "the map's own room stays");
    let gate = t.room("mudlet:1").unwrap();
    assert_eq!(gate.name, "Gate");
    assert_eq!(gate.description, "Seen by the server.");
    assert_eq!(gate.observed_name.as_deref(), Some("Old Gate"));
    assert_eq!(gate.inferred_environment.as_deref(), Some("city"));
    assert!(t.grid_mode("Town"));
    assert_eq!(t.label_count(), 2);
    let step = t.last_edit();
    // The same file again: nothing changes, no new undo step.
    assert_eq!(t.import_map(&imported()).unwrap(), 0);
    assert_eq!(t.last_edit(), step);
    // One undo takes the whole import back.
    assert!(t.undo());
    assert_eq!(t.room_count(), 2);
    assert_eq!(t.room("mudlet:1").unwrap().name, "Old Gate");
    assert_eq!(t.label_count(), 0);
    assert_eq!(t.link_count(), 0);
    assert!(!t.grid_mode("Town"));
}

#[test]
fn an_import_past_the_maps_limits_changes_nothing() {
    let rooms: Vec<MapRoom> = (0..super::format::MAX_ROOMS).map(|i| room(&format!("r{i}"))).collect();
    let mut t = RoomMapTracker::from_snapshot(MapSnapshot::of(rooms, Vec::new()));
    let error = t.import_map(&imported()).unwrap_err();
    assert!(error.0.contains("10002"), "{error}");
    assert!(t.room("mudlet:1").is_none());
    assert!(!t.can_undo());
}
