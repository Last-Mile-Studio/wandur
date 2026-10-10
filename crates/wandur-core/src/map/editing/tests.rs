//! The editor's operations: direction inference, connecting rooms one-way and two-way, moves,
//! deletes and field edits as single undo steps, saved and reloaded through `wandur.db`, and
//! hand-placed rooms kept out of automatic docking.

use super::*;
use crate::map::model::{DoorState, MapSnapshot, RoomObservation, RoomSource};
use crate::map::store::{MapStore, MapWorld};

fn room(id: &str, x: f64, y: f64, z: f64) -> MapRoom {
    MapRoom::new(id, &format!("Room {id}"), "", Some("Keep"), x, y, z, false)
}

/// a (0,0,0), b (1,0,0), c (1,1,0), d (0,0,1); a and b joined both ways.
fn keep() -> RoomMapTracker {
    RoomMapTracker::from_snapshot(MapSnapshot::of(
        vec![
            room("a", 0.0, 0.0, 0.0),
            room("b", 1.0, 0.0, 0.0),
            room("c", 1.0, 1.0, 0.0),
            room("d", 0.0, 0.0, 1.0),
        ],
        vec![
            MapLink::new("a", "b", "east", true),
            MapLink::new("b", "a", "west", true),
        ],
    ))
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn directions_are_inferred_from_positions_with_diagonals_and_floors() {
    let o = (0.0, 0.0, 0.0);
    let cases = [
        ((0.0, 1.0, 0.0), "north"),
        ((0.0, -3.0, 0.0), "south"),
        ((2.0, 0.0, 0.0), "east"),
        ((-1.0, 0.0, 0.0), "west"),
        ((1.0, 1.0, 0.0), "northeast"),
        ((-1.0, 1.0, 0.0), "northwest"),
        ((1.0, -1.0, 0.0), "southeast"),
        ((-2.0, -2.0, 0.0), "southwest"),
        // The nearest compass: mostly east with a little north is east.
        ((3.0, 1.0, 0.0), "east"),
        ((1.0, 3.0, 0.0), "north"),
        // Another floor is up or down, wherever it lies on the floor.
        ((5.0, 5.0, 1.0), "up"),
        ((0.0, 0.0, -2.0), "down"),
    ];
    for (to, want) in cases {
        assert_eq!(infer_direction(o, to), Some(want), "{to:?}");
    }
    assert_eq!(infer_direction(o, o), None);
    assert_eq!(infer_direction(o, (f64::NAN, 0.0, 0.0)), None);
}

#[test]
fn connecting_two_way_adds_the_way_back_and_one_way_does_not_both_one_undo_step() {
    let mut t = keep();
    assert!(t.connect_rooms("b", "c", "north", true));
    let north = t.link("b", "north").unwrap();
    assert!(north.to_id == "c" && north.confirmed && north.is_manually_edited);
    assert_eq!(t.link("c", "south").map(|l| l.to_id.as_str()), Some("b"));
    assert!(t.undo());
    assert!(
        t.link("b", "north").is_none() && t.link("c", "south").is_none(),
        "one step"
    );
    assert!(t.redo());

    assert!(t.connect_rooms("a", "c", "northeast", false));
    assert!(t.link("a", "northeast").is_some());
    assert!(t.return_links("a", "northeast").is_empty(), "one-way");
    // A way between floors comes back the other way.
    assert!(t.connect_rooms("a", "d", "up", true));
    assert_eq!(t.link("d", "down").map(|l| l.to_id.as_str()), Some("a"));
    // A custom direction comes back by the direction the positions give.
    assert!(t.connect_rooms("c", "a", "portal", true));
    assert_eq!(t.link("a", "northeast").unwrap().to_id, "c");
    assert!(
        t.links().any(|l| l.from_id == "a" && l.to_id == "c"),
        "the way back exists"
    );
    assert!(!t.connect_rooms("a", "a", "north", true), "not to itself");
    assert!(!t.connect_rooms("a", "missing", "north", true));
}

#[test]
fn one_way_and_two_way_toggle_the_way_back() {
    let mut t = keep();
    assert_eq!(t.return_links("a", "east").len(), 1);
    assert!(t.set_two_way("a", "east", false));
    assert!(t.link("b", "west").is_none());
    assert!(t.set_two_way("a", "east", true));
    assert_eq!(t.link("b", "west").map(|l| l.to_id.as_str()), Some("a"));
    assert!(t.undo());
    assert!(t.link("b", "west").is_none());
}

#[test]
fn moving_several_rooms_is_one_undo_step_and_marks_them_edited() {
    let mut t = keep();
    assert_eq!(t.move_rooms(&ids(&["a", "b", "c"]), 2.0, -1.0, 0.0), 3);
    let a = t.room("a").unwrap();
    assert_eq!((a.x, a.y), (2.0, -1.0));
    assert!(a.is_manually_edited);
    assert_eq!(t.room("c").map(|r| (r.x, r.y)), Some((3.0, 0.0)));
    assert!(t.undo());
    assert_eq!(t.room("a").map(|r| (r.x, r.y)), Some((0.0, 0.0)));
    assert_eq!(t.room("c").map(|r| (r.x, r.y)), Some((1.0, 1.0)));
    assert!(!t.can_undo(), "one step for the whole move");
    // Moving by nothing records nothing.
    assert_eq!(t.move_rooms(&ids(&["a"]), 0.0, 0.0, 0.0), 0);
    assert!(!t.can_undo());
}

#[test]
fn deleting_a_selection_leaves_tombstones_and_undoes_in_one_step() {
    let mut t = keep();
    assert_eq!(t.remove_rooms(&ids(&["a", "c"])), 2);
    assert_eq!(t.room_count(), 2);
    assert!(t.link("b", "west").is_none(), "exits to a deleted room go too");
    let snapshot = t.snapshot();
    assert!(snapshot.deleted_rooms.iter().any(|d| d.id == "a"));
    assert!(
        snapshot
            .deleted_links
            .iter()
            .any(|d| d.from_id == "b" && d.direction == "west")
    );
    assert!(t.undo());
    assert_eq!(t.room_count(), 4);
    assert!(t.link("a", "east").is_some() && t.link("b", "west").is_some());
    assert!(!t.can_undo());
    assert_eq!(
        t.remove_links(&[("a".into(), "east".into()), ("b".into(), "west".into())]),
        2
    );
    assert!(t.undo());
    assert_eq!(t.link_count(), 2);
}

#[test]
fn field_edits_on_several_rooms_apply_together_and_refuse_bad_values() {
    let mut t = keep();
    let n = t.edit_rooms(&ids(&["a", "b", "c"]), |r| {
        r.area = Some("Garden".into());
        r.environment = Some("forest".into());
    });
    assert_eq!(n, 3);
    assert!(t.rooms().filter(|r| r.area.as_deref() == Some("Garden")).count() == 3);
    assert!(t.undo());
    assert!(t.rooms().all(|r| r.area.as_deref() == Some("Keep")));
    assert!(!t.can_undo());
    assert_eq!(
        t.edit_rooms(&ids(&["a"]), |r| r.name = " ".into()),
        0,
        "a room needs a name"
    );
    assert_eq!(t.edit_rooms(&ids(&["a"]), |r| r.color = Some("red".into())), 0);
    assert_eq!(t.edit_rooms(&ids(&["a"]), |r| r.name = "Room a".into()), 0, "unchanged");
    assert!(!t.can_undo());
}

#[test]
fn exit_edits_rename_change_and_refuse_a_clash() {
    let mut t = keep();
    assert!(t.edit_exit("a", "east", |l| {
        l.door_state = DoorState::Closed;
        l.weight = 4.0;
        l.command = Some("open door; east".into());
    }));
    let east = t.link("a", "east").unwrap();
    assert_eq!((east.door_state, east.weight), (DoorState::Closed, 4.0));
    assert!(t.edit_exit("a", "east", |l| l.direction = "gate".into()));
    assert!(t.link("a", "east").is_none() && t.link("a", "gate").is_some());
    assert!(
        t.snapshot()
            .deleted_links
            .iter()
            .any(|d| d.from_id == "a" && d.direction == "east")
    );
    assert!(t.connect_rooms("a", "c", "north", false));
    assert!(
        !t.edit_exit("a", "gate", |l| l.direction = "north".into()),
        "north is taken"
    );
    assert!(t.edit_exit("a", "gate", |l| l.to_id = "c".into()));
    assert_eq!(t.link("a", "gate").unwrap().to_id, "c");
    assert!(t.undo());
    assert_eq!(t.link("a", "gate").unwrap().to_id, "b");
}

#[test]
fn edits_inside_a_group_are_one_step_and_nested_groups_record_once() {
    let mut t = keep();
    t.edit_group(|t| {
        t.move_rooms(&ids(&["a"]), 5.0, 0.0, 0.0);
        t.edit_group(|t| t.connect_rooms("a", "c", "north", true));
        t.remove_rooms(&ids(&["d"]));
    });
    assert!(t.undo());
    assert!(!t.can_undo());
    assert_eq!(t.room_count(), 4);
    assert_eq!(t.room("a").map(|r| r.x), Some(0.0));
    assert!(t.link("a", "north").is_none());
    // A group that changes nothing records nothing and keeps the redo history.
    assert!(t.can_redo());
    t.edit_group(|t| t.remove_rooms(&ids(&["missing"])));
    assert!(t.can_redo());
}

#[test]
fn edits_survive_a_save_and_a_reload() {
    let dir = std::env::temp_dir().join(format!("wandur-map-editing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (db, _) = crate::db::Database::open(&dir).unwrap();
    let world = MapWorld::Endpoint {
        host: "host".into(),
        port: 4000,
    };
    let store = MapStore::new(db);
    let mut t = keep();
    let added = MapRoom {
        is_manually_edited: true,
        ..room("manual:1", 2.0, 0.0, 0.0)
    };
    assert!(t.upsert_room(added));
    assert!(t.connect_rooms("b", "manual:1", "east", true));
    assert_eq!(t.move_rooms(&ids(&["c"]), 0.0, 1.0, 0.0), 1);
    assert_eq!(t.edit_rooms(&ids(&["a"]), |r| r.name = "Gatehouse".into()), 1);
    assert!(t.edit_exit("a", "east", |l| l.door_state = DoorState::Locked));
    assert_eq!(t.remove_rooms(&ids(&["d"])), 1);
    store.save(&world, &t.snapshot()).unwrap();

    let saved = store.load(&world).unwrap().unwrap();
    let r = RoomMapTracker::from_snapshot(saved);
    assert_eq!(r.room("a").unwrap().name, "Gatehouse");
    assert_eq!(r.room("c").map(|c| (c.x, c.y)), Some((1.0, 2.0)));
    assert!(r.room("d").is_none());
    assert_eq!(r.link("b", "east").unwrap().to_id, "manual:1");
    assert_eq!(r.link("manual:1", "west").unwrap().to_id, "b");
    assert_eq!(r.link("a", "east").unwrap().door_state, DoorState::Locked);
    // An older copy of the map saved later cannot bring the deleted room back.
    store.save(&world, &keep().snapshot()).unwrap();
    let merged = store.load(&world).unwrap().unwrap();
    assert!(!merged.rooms.iter().any(|r| r.id == "d"), "the tombstone wins");
    assert!(merged.rooms.iter().any(|r| r.name == "Gatehouse"), "the edit wins");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn rooms_placed_by_hand_are_not_docked_by_later_moves() {
    let at = |id: &str| {
        RoomObservation::new(Some(id), &format!("Room {id}"), "", &[])
            .with_area("Keep")
            .with_source(RoomSource::Gmcp)
    };
    let mut t = RoomMapTracker::new();
    t.observe(&at("1"), None);
    t.observe(&at("2"), Some("east"));
    t.observe(&at("9"), None); // no known route: parked beside the map
    // The editor drags the parked room far away: a manual edit.
    assert_eq!(t.place_rooms(&[("s:9".into(), 30.0, 30.0, 0.0)]), 1);
    assert!(t.room("s:9").unwrap().is_manually_edited);
    t.observe(&at("1"), None);
    t.observe(&at("9"), Some("north"));
    assert_eq!(
        t.room("s:9").map(|r| (r.x, r.y)),
        Some((30.0, 30.0)),
        "the hand-placed room stays"
    );
}
