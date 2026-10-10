//! Three-way merges of an official map: upstream changes taken where the map is untouched,
//! the person's edits and deletions kept, upstream removals applied, labels, one undo step.

use super::*;
use crate::map::model::{DoorState, RoomObservation};

fn map(xml: &str) -> MapSnapshot {
    crate::map::mudlet::read(Some("xml"), xml.as_bytes(), &|_| {})
        .unwrap()
        .map
}

fn room(id: u32, name: &str, x: i32, y: i32, exits: &[(&str, u32)]) -> String {
    let exits: String = exits
        .iter()
        .map(|(d, to)| format!(r#"<exit direction="{d}" target="{to}"/>"#))
        .collect();
    format!(r#"<room id="{id}" area="1" title="{name}"><coord x="{x}" y="{y}" z="0"/>{exits}</room>"#)
}

fn file(rooms: &[String]) -> String {
    format!(
        r#"<map><areas><area id="1" name="Vale"/></areas><rooms>{}</rooms></map>"#,
        rooms.concat()
    )
}

/// Four rooms in a row: 1 - 2 - 3 - 4.
fn base() -> MapSnapshot {
    map(&file(&[
        room(1, "A", 0, 0, &[("east", 2)]),
        room(2, "B", 1, 0, &[("west", 1), ("east", 3)]),
        room(3, "C", 2, 0, &[("east", 4)]),
        room(4, "D", 3, 0, &[]),
    ]))
}

/// Upstream: 1 renamed, 2 moved, 3 gone, 4 renamed, 5 new east of 4.
fn theirs() -> MapSnapshot {
    map(&file(&[
        room(1, "A2", 0, 0, &[("east", 2)]),
        room(2, "B", 1, 1, &[("west", 1)]),
        room(4, "D2", 3, 0, &[("east", 5)]),
        room(5, "E", 4, 0, &[]),
    ]))
}

fn imported() -> RoomMapTracker {
    let mut tracker = RoomMapTracker::new();
    let report = merge(&mut tracker, None, &base()).unwrap();
    assert_eq!(
        report,
        MergeReport {
            added: 4 + 4,
            ..MergeReport::default()
        }
    );
    tracker
}

fn renamed(tracker: &RoomMapTracker, id: &str, name: &str) -> MapRoom {
    MapRoom {
        name: name.into(),
        ..tracker.room(id).unwrap().clone()
    }
}

#[test]
fn the_first_import_is_a_plain_import_and_the_same_file_again_changes_nothing() {
    let mut tracker = imported();
    let base = base();
    let edits = tracker.can_undo();
    let report = merge(&mut tracker, Some(&base), &base).unwrap();
    assert_eq!(report, MergeReport::default());
    assert!(!report.changed());
    assert_eq!(tracker.can_undo(), edits);
}

#[test]
fn untouched_items_take_the_new_version_and_edits_stay() {
    let mut tracker = imported();
    // The tracker walks into room 1: what it records is not an edit.
    tracker.observe(
        &RoomObservation::new(
            Some("1"),
            "A as the server says",
            "A long room.",
            &[("east", Some("2"))],
        ),
        None,
    );
    // The person renames room 4.
    assert!(tracker.upsert_room(renamed(&tracker, "s:4", "Dee")));
    let before = tracker.snapshot();
    let report = merge(&mut tracker, Some(&base()), &theirs()).unwrap();
    assert_eq!(
        report,
        MergeReport {
            added: 2,      // room 5 and the exit 4 east
            updated: 2,    // rooms 1 and 2
            kept_local: 1, // room 4, renamed here and upstream
            removed: 2,    // room 3 and the exit 2 east
        }
    );
    assert_eq!(tracker.room("s:1").unwrap().name, "A2");
    assert_eq!(
        tracker.room("s:1").unwrap().observed_name.as_deref(),
        Some("A as the server says"),
        "what the tracker observed stays"
    );
    assert_eq!(tracker.room("s:2").unwrap().y, 1.0);
    assert!(tracker.room("s:3").is_none());
    assert!(tracker.link("s:2", "east").is_none());
    assert_eq!(tracker.room("s:4").unwrap().name, "Dee");
    assert_eq!(tracker.room("s:5").unwrap().name, "E");
    assert_eq!(tracker.link("s:4", "east").unwrap().to_id, "s:5");
    // One undo step takes it all back.
    assert!(tracker.undo());
    let after_undo = tracker.snapshot();
    let ids = |m: &MapSnapshot| {
        let mut v: Vec<(String, String)> = m.rooms.iter().map(|r| (r.id.clone(), r.name.clone())).collect();
        v.sort();
        v
    };
    assert_eq!(ids(&after_undo), ids(&before));
}

#[test]
fn deleted_here_stays_deleted_and_edited_here_survives_an_upstream_removal() {
    let mut tracker = imported();
    // Room 2 deleted here (upstream moved it); room 3 moved here (upstream removed it).
    assert!(tracker.remove_room("s:2"));
    let moved = MapRoom {
        x: 9.0,
        ..tracker.room("s:3").unwrap().clone()
    };
    assert!(tracker.upsert_room(moved));
    let report = merge(&mut tracker, Some(&base()), &theirs()).unwrap();
    assert!(tracker.room("s:2").is_none(), "deleted here");
    assert!(
        tracker.link("s:1", "east").is_none(),
        "no exit into a deleted room comes back"
    );
    assert_eq!(tracker.room("s:3").unwrap().x, 9.0, "edited here");
    assert_eq!(report.kept_local, 2);
    // Room 3's exit east, untouched here, went with it upstream: items merge one by one.
    assert!(tracker.link("s:3", "east").is_none());
    assert_eq!(report.removed, 1);
}

#[test]
fn an_exit_edited_here_keeps_its_door() {
    let mut tracker = imported();
    let mut link = tracker.link("s:1", "east").unwrap().clone();
    link.door_state = DoorState::Locked;
    assert!(tracker.upsert_link(link));
    // Upstream points the exit somewhere else.
    let changed = map(&file(&[
        room(1, "A", 0, 0, &[("east", 4)]),
        room(2, "B", 1, 0, &[("west", 1), ("east", 3)]),
        room(3, "C", 2, 0, &[("east", 4)]),
        room(4, "D", 3, 0, &[]),
    ]));
    let report = merge(&mut tracker, Some(&base()), &changed).unwrap();
    let link = tracker.link("s:1", "east").unwrap();
    assert_eq!((link.to_id.as_str(), link.door_state), ("s:2", DoorState::Locked));
    assert_eq!(
        report,
        MergeReport {
            kept_local: 1,
            ..MergeReport::default()
        }
    );
}

#[test]
fn a_room_the_tracker_learned_takes_the_files_version() {
    let mut tracker = RoomMapTracker::new();
    tracker.observe(&RoomObservation::new(Some("5"), "E seen", "", &[]), None);
    let report = merge(&mut tracker, Some(&base()), &theirs()).unwrap();
    assert_eq!(tracker.room("s:5").unwrap().name, "E");
    assert!(report.updated >= 1);
}

#[test]
fn labels_merge_like_rooms() {
    let label = |id: &str, text: &str| MapLabel::text(id, Some("Vale"), 0.0, 5.0, 0.0, text);
    let mut base = base();
    base.labels = vec![label("l1", "Old"), label("l2", "Two"), label("l4", "Four")];
    let mut theirs = base.clone();
    theirs.labels = vec![label("l1", "New"), label("l3", "Three"), label("l4", "Four upstream")];
    let mut tracker = RoomMapTracker::new();
    merge(&mut tracker, None, &base).unwrap();
    // The person edits l4.
    let mut mine = tracker.label("l4").unwrap().clone();
    mine.text = "Mine".into();
    assert!(tracker.upsert_label(mine));
    let report = merge(&mut tracker, Some(&base), &theirs).unwrap();
    assert_eq!(tracker.label("l1").unwrap().text, "New");
    assert!(tracker.label("l2").is_none());
    assert_eq!(tracker.label("l3").unwrap().text, "Three");
    assert_eq!(tracker.label("l4").unwrap().text, "Mine");
    assert_eq!(
        report,
        MergeReport {
            added: 1,
            updated: 1,
            kept_local: 1,
            removed: 1,
        }
    );
}
