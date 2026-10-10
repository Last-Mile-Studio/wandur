//! The C# `MapDockingTests`, ported: rooms seen without a known route are placed near the map
//! and dock once a move connects them; a move the server's exits contradict is a transport.

use std::collections::HashMap;

use super::RoomMapTracker;
use crate::map::model::*;
use crate::map::store::{MapStore, MapWorld};

const AREA: &str = "Coruscant";

fn at(id: &str, exits: &[(&str, Option<&str>)]) -> RoomObservation {
    RoomObservation::new(Some(id), &format!("Room {id}"), "", exits)
        .with_area(AREA)
        .with_source(RoomSource::Gmcp)
}

fn room(tracker: &RoomMapTracker, id: &str) -> MapRoom {
    tracker.room(id).cloned().unwrap_or_else(|| panic!("no room {id}"))
}

fn assert_at(room: &MapRoom, x: f64, y: f64) {
    assert!(
        (room.x - x).abs() < 1e-6 && (room.y - y).abs() < 1e-6 && room.z.abs() < 1e-6,
        "{} at ({}, {}, {}), expected ({x}, {y}, 0)",
        room.id,
        room.x,
        room.y,
        room.z
    );
}

fn positions(tracker: &RoomMapTracker) -> HashMap<String, (f64, f64, f64)> {
    tracker.rooms().map(|r| (r.id.clone(), (r.x, r.y, r.z))).collect()
}

/// 1 (0,0) east 2 (1,0) east 3 (2,0) north 4 (2,1) north 5 (2,2).
fn cluster() -> RoomMapTracker {
    let mut t = RoomMapTracker::new();
    t.observe(&at("1", &[]), None);
    t.observe(&at("2", &[]), Some("east"));
    t.observe(&at("3", &[]), Some("east"));
    t.observe(&at("4", &[]), Some("north"));
    t.observe(&at("5", &[]), Some("north"));
    t
}

fn saved(id: &str, x: f64, y: f64) -> MapRoom {
    MapRoom {
        revision: 5,
        ..MapRoom::new(
            &format!("s:{id}"),
            &format!("Room {id}"),
            "",
            Some(AREA),
            x,
            y,
            0.0,
            false,
        )
        .with_server_id(id)
    }
}

fn saved_link(from: &str, to: &str, direction: &str) -> MapLink {
    MapLink {
        revision: 5,
        ..MapLink::new(from, to, direction, true)
    }
}

#[test]
fn a_floating_room_docks_when_walked_into_through_a_northeast_door() {
    let mut t = cluster();
    t.observe(&at("9", &[]), None); // recall, teleport or a server-driven move: no known route
    assert!(room(&t, "s:9").x - 2.0 > 1.0, "beside the map, not on top of it");
    t.observe(&at("1", &[]), None);
    t.observe(&at("9", &[]), Some("northeast"));
    assert_at(&room(&t, "s:9"), 1.0, 1.0);
    assert_at(&room(&t, "s:1"), 0.0, 0.0);
    assert!(!t.can_undo(), "docking is observation, not a manual edit");
}

#[test]
fn a_floating_room_docks_when_its_server_exits_name_a_cluster_room() {
    let mut t = cluster();
    t.observe(&at("9", &[("sw", Some("1"))]), None);
    assert_at(&room(&t, "s:9"), 1.0, 1.0);
    assert_at(&room(&t, "s:1"), 0.0, 0.0);
}

#[test]
fn a_floating_cluster_docks_as_a_unit_and_keeps_its_shape() {
    let mut t = cluster();
    t.observe(&at("9", &[]), None);
    t.observe(&at("10", &[]), Some("west"));
    t.observe(&at("11", &[]), Some("north"));
    t.observe(&at("1", &[]), None);
    let mut before = positions(&t);
    before.retain(|id, _| !matches!(id.as_str(), "s:9" | "s:10" | "s:11"));
    t.observe(&at("9", &[]), Some("northwest"));
    assert_at(&room(&t, "s:9"), -1.0, 1.0);
    assert_at(&room(&t, "s:10"), -2.0, 1.0);
    assert_at(&room(&t, "s:11"), -2.0, 2.0);
    let mut after = positions(&t);
    after.retain(|id, _| before.contains_key(id));
    assert_eq!(before, after);
}

#[test]
fn a_docked_room_landing_on_an_occupied_spot_is_nudged() {
    let mut t = cluster();
    t.observe(&at("9", &[]), None);
    t.observe(&at("2", &[]), None);
    t.observe(&at("9", &[]), Some("northeast")); // (2,1) already holds room 4
    let docked = room(&t, "s:9");
    assert!((2.3..=2.4).contains(&docked.x), "{}", docked.x);
    assert!((docked.y - 1.0).abs() < 1e-6);
    assert_at(&room(&t, "s:4"), 2.0, 1.0);
}

#[test]
fn already_connected_rooms_keep_their_non_grid_geometry() {
    let mut t = RoomMapTracker::new();
    t.observe(&at("1", &[]), None);
    for id in ["2", "3", "4", "5"] {
        t.observe(&at(id, &[]), Some("east"));
    }
    let before = positions(&t);
    t.observe(&at("1", &[]), Some("west")); // a long winding road back to the start
    assert!(
        t.links()
            .any(|l| l.from_id == "s:5" && l.to_id == "s:1" && l.direction == "west")
    );
    assert_eq!(before, positions(&t));
}

#[test]
fn a_manually_edited_floating_room_stays_and_the_unanchored_cluster_moves_instead() {
    let mut t = cluster();
    t.observe(&at("9", &[]), None);
    let floating = room(&t, "s:9");
    assert!(t.upsert_room(MapRoom {
        x: 40.0,
        y: 40.0,
        ..floating
    }));
    t.observe(&at("1", &[]), None);
    t.observe(&at("9", &[]), Some("northeast"));
    assert_at(&room(&t, "s:9"), 40.0, 40.0);
    assert_at(&room(&t, "s:1"), 39.0, 39.0);
    assert_at(&room(&t, "s:2"), 40.0, 39.0);
    assert_at(&room(&t, "s:5"), 41.0, 41.0);
    // Only the manual edit is undoable; the docking itself is not.
    assert!(t.undo());
    assert!(!t.can_undo());
}

#[test]
fn nothing_moves_when_both_sides_hold_manual_edits_or_locks() {
    let mut t = cluster();
    t.observe(&at("9", &[]), None);
    let floating = room(&t, "s:9");
    assert!(t.upsert_room(MapRoom {
        x: 40.0,
        y: 40.0,
        ..floating
    }));
    let three = room(&t, "s:3");
    assert!(t.upsert_room(MapRoom {
        is_locked: true,
        ..three
    }));
    let before = positions(&t);
    t.observe(&at("1", &[]), None);
    t.observe(&at("9", &[]), Some("northeast"));
    assert_eq!(before, positions(&t));
    assert!(t.links().any(|l| l.from_id == "s:1" && l.to_id == "s:9"));
}

#[test]
fn rooms_placed_from_server_coordinates_are_never_moved() {
    let with = |o: RoomObservation, x: f64| RoomObservation {
        x: Some(x),
        y: Some(0.0),
        z: Some(0.0),
        ..o
    };
    let mut t = RoomMapTracker::new();
    t.observe(&with(at("1", &[]), 0.0), None);
    t.observe(&with(at("9", &[]), 50.0), None);
    t.observe(&with(at("1", &[]), 0.0), None);
    t.observe(&with(at("9", &[]), 50.0), Some("northeast"));
    assert_at(&room(&t, "s:9"), 50.0, 0.0);
    assert_at(&room(&t, "s:1"), 0.0, 0.0);
}

#[test]
fn a_saved_far_parked_room_repairs_when_the_link_is_walked_and_the_save_keeps_it() {
    let dir = std::env::temp_dir().join(format!("wandur-map-docking-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (db, _) = crate::db::Database::open(&dir).unwrap();
    let store = MapStore::new(db);
    let world = MapWorld::Endpoint {
        host: "mud.example".into(),
        port: 4000,
    };
    let snapshot = MapSnapshot {
        source: RoomSource::Gmcp,
        ..MapSnapshot::of(
            vec![
                saved("1", 0.0, 0.0),
                saved("2", 1.0, 0.0),
                saved("9", 200.0, 0.0),
                saved("10", 201.0, 0.0),
            ],
            vec![
                saved_link("s:1", "s:2", "east"),
                saved_link("s:2", "s:9", "northeast"),
                saved_link("s:9", "s:10", "east"),
            ],
        )
    };
    store.save(&world, &snapshot).unwrap();
    let mut t = RoomMapTracker::from_snapshot(store.load(&world).unwrap().unwrap());
    t.observe(&at("2", &[]), None);
    t.observe(&at("9", &[]), Some("northeast"));
    assert_at(&room(&t, "s:9"), 2.0, 1.0);
    assert_at(&room(&t, "s:10"), 3.0, 1.0);
    assert!(!t.can_undo());
    store.save(&world, &t.snapshot()).unwrap();
    let reloaded = store.load(&world).unwrap().unwrap();
    assert_at(reloaded.room("s:9").unwrap(), 2.0, 1.0);
    assert_at(reloaded.room("s:10").unwrap(), 3.0, 1.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_move_the_server_exits_contradict_is_a_transport_and_adds_no_link() {
    let mut t = RoomMapTracker::new();
    t.observe(&at("1", &[("north", Some("2"))]), None);
    t.observe(&at("2", &[("south", Some("1"))]), Some("north"));
    t.observe(&at("1", &[("north", Some("2"))]), Some("south"));
    // "north" again, but the game moves the player to a shuttle bay instead.
    t.observe(&at("50", &[]), Some("north"));
    assert!(
        t.links()
            .any(|l| l.from_id == "s:1" && l.direction == "north" && l.to_id == "s:2")
    );
    assert!(!t.links().any(|l| l.to_id == "s:50"));
    let bay = room(&t, "s:50");
    assert!(!(bay.x == 0.0 && bay.y == 1.0), "not placed where room 2 belongs");
}

#[test]
fn a_move_through_an_unlisted_exit_still_links_because_doors_can_be_hidden() {
    let mut t = RoomMapTracker::new();
    t.observe(&at("1", &[("south", Some("0"))]), None);
    t.observe(&at("7", &[]), Some("northeast"));
    assert!(
        t.links()
            .any(|l| l.from_id == "s:1" && l.direction == "northeast" && l.to_id == "s:7")
    );
    assert_at(&room(&t, "s:7"), 1.0, 1.0);
}

/// The cluster 1 (0,0) east 2 (1,0) east 3 (2,0), and an island 9 (200,0) east 10 (201,0)
/// parked far east but joined to it by three long links. All three fit once the island moves
/// by (-198, 1).
fn parked_island_with_several_links(conflicting: bool) -> RoomMapTracker {
    let mut links = vec![
        saved_link("s:1", "s:2", "east"),
        saved_link("s:2", "s:3", "east"),
        saved_link("s:9", "s:10", "east"),
        saved_link("s:2", "s:9", "northeast"),
        saved_link("s:3", "s:9", "north"),
        saved_link("s:9", "s:3", "south"),
    ];
    // A link that cannot fit after the same move: the island is real, non-grid geometry.
    if conflicting {
        links.push(saved_link("s:1", "s:10", "north"));
    }
    RoomMapTracker::from_snapshot(MapSnapshot {
        source: RoomSource::Gmcp,
        ..MapSnapshot::of(
            vec![
                saved("1", 0.0, 0.0),
                saved("2", 1.0, 0.0),
                saved("3", 2.0, 0.0),
                saved("9", 200.0, 0.0),
                saved("10", 201.0, 0.0),
            ],
            links,
        )
    })
}

#[test]
fn a_parked_island_joined_by_several_long_links_docks_when_they_all_fit_after_one_move() {
    let mut t = parked_island_with_several_links(false);
    t.observe(&at("2", &[]), None);
    t.observe(&at("9", &[]), Some("northeast"));
    assert_at(&room(&t, "s:9"), 2.0, 1.0);
    assert_at(&room(&t, "s:10"), 3.0, 1.0);
    assert_at(&room(&t, "s:3"), 2.0, 0.0);
}

#[test]
fn a_parked_island_stays_when_another_link_would_still_not_fit() {
    let mut t = parked_island_with_several_links(true);
    t.observe(&at("2", &[]), None);
    t.observe(&at("9", &[]), Some("northeast"));
    assert_at(&room(&t, "s:9"), 200.0, 0.0);
    assert_at(&room(&t, "s:10"), 201.0, 0.0);
}

#[test]
fn a_saved_far_parked_text_room_repairs_when_the_existing_link_is_walked() {
    let text = |name: &str| {
        RoomObservation::new(None, name, &format!("{name} prose for recognition"), &[]).with_source(RoomSource::Text)
    };
    let mut t = RoomMapTracker::from_snapshot(MapSnapshot {
        source: RoomSource::Text,
        ..MapSnapshot::of(
            vec![
                MapRoom::new(
                    "t:hall",
                    "Hall",
                    "Hall prose for recognition",
                    None,
                    0.0,
                    0.0,
                    0.0,
                    true,
                ),
                MapRoom::new(
                    "t:garden",
                    "Garden",
                    "Garden prose for recognition",
                    None,
                    200.0,
                    0.0,
                    0.0,
                    true,
                ),
            ],
            vec![MapLink::new("t:hall", "t:garden", "northeast", false)],
        )
    });
    t.observe(&text("Hall"), None);
    assert_eq!(t.current_id(), Some("t:hall"));
    t.observe(&text("Garden"), Some("northeast"));
    assert_eq!(t.current_id(), Some("t:garden"));
    assert_at(&room(&t, "t:garden"), 1.0, 1.0);
    assert!(room(&t, "t:garden").revision > 0);
}

#[test]
fn a_room_with_no_known_route_is_placed_beside_the_map_not_by_room_count() {
    let mut t = RoomMapTracker::new();
    t.observe(&at("0", &[]), None);
    for i in 1..100 {
        let direction = if i % 10 == 0 {
            "north"
        } else if (i / 10) % 2 == 0 {
            "east"
        } else {
            "west"
        };
        t.observe(&at(&i.to_string(), &[]), Some(direction));
    }
    let max_x = t.rooms().map(|r| r.x).fold(f64::NEG_INFINITY, f64::max);
    let current = t.current().cloned().unwrap();
    t.observe(&at("500", &[]), None);
    let placed = room(&t, "s:500");
    assert!(
        (max_x + 1.0..=max_x + 3.0).contains(&placed.x),
        "{} vs {max_x}",
        placed.x
    );
    assert!((placed.y - current.y).abs() < 1e-6 && (placed.z - current.z).abs() < 1e-6);
    assert!(
        !t.rooms()
            .any(|r| r.id != placed.id && r.x == placed.x && r.y == placed.y && r.z == placed.z)
    );
}

#[test]
fn the_first_room_of_an_empty_map_starts_at_the_origin() {
    let mut t = RoomMapTracker::new();
    t.observe(&at("1", &[]), None);
    assert_at(&room(&t, "s:1"), 0.0, 0.0);
}
