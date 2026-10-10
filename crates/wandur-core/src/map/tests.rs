//! The C# mapping tests, ported: `RoomMapTests`, `MapIdentityTests`, `ExpandedMapTests`,
//! `RoomSearchTests`, `TextRoomObserverTests`, the decoder half of `RoomProtocolTests` and
//! `SqliteMapStoreTests` (on `wandur.db`), plus the session's walking rules.

use std::time::{Duration, Instant};

use super::decode::{from_gmcp, from_msdp};
use super::format::{deserialize, serialize};
use super::model::*;
use super::route::find_route;
use super::search::{parse_terms, search};
use super::session::*;
use super::store::{MapStore, MapWorld};
use super::text::{TextRoomObserver, is_movement_failure};
use super::tracker::RoomMapTracker;
use crate::protocol::parse_gmcp;

fn exits(list: &[&str]) -> Vec<(&'static str, Option<&'static str>)> {
    list.iter()
        .map(|e| (Box::leak(e.to_string().into_boxed_str()) as &'static str, None))
        .collect()
}

/// A text room with the C# fixture's description.
fn text_room(name: &str, exit_list: &[&str]) -> RoomObservation {
    RoomObservation::new(None, name, "Stone walls and a worn floor.", &exits(exit_list))
}

fn id_room(id: &str, name: &str, exit_list: &[(&str, Option<&str>)]) -> RoomObservation {
    RoomObservation::new(Some(id), name, "", exit_list).with_source(RoomSource::Gmcp)
}

fn room(id: &str) -> MapRoom {
    MapRoom::new(id, id, "", Some("Keep"), 0.0, 0.0, 0.0, false)
}

fn map(rooms: Vec<MapRoom>) -> MapSnapshot {
    MapSnapshot {
        source: RoomSource::Gmcp,
        ..MapSnapshot::of(rooms, Vec::new())
    }
}

fn corridor_map() -> MapSnapshot {
    let r = |id: &str, name: &str, desc: &str, x: f64, y: f64| MapRoom::new(id, name, desc, None, x, y, 0.0, true);
    let walls = "Stone walls and a worn floor.";
    MapSnapshot {
        source: RoomSource::Text,
        ..MapSnapshot::of(
            vec![
                r("a", "Corridor", walls, 0.0, 0.0),
                r("b", "Corridor", walls, 0.0, 1.0),
                r("c", "Corridor", walls, 0.0, 2.0),
                r("d", "Corridor", walls, 3.0, 0.0),
                r("shrine", "Shrine", "A marble altar.", 0.0, 3.0),
            ],
            vec![
                MapLink::new("a", "b", "north", false),
                MapLink::new("b", "c", "north", false),
                MapLink::new("c", "shrine", "north", false),
                MapLink::new("d", "shrine", "east", false),
            ],
        )
    }
}

fn temp_db(name: &str) -> (crate::db::Database, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("wandur-map-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (db, _) = crate::db::Database::open(&dir).unwrap();
    (db, dir)
}

fn world() -> MapWorld {
    MapWorld::Endpoint {
        host: "host".into(),
        port: 4000,
    }
}

// ---- RoomMapTests ------------------------------------------------------------------------

#[test]
fn uncommanded_room_change_starts_a_separate_component_and_movement_continues_there() {
    for source in [RoomSource::Gmcp, RoomSource::Msdp] {
        let at = |id: &str| RoomObservation::new(Some(id), &format!("Room {id}"), "", &[]).with_source(source);
        let mut t = RoomMapTracker::new();
        t.observe(&at("1"), None);
        t.observe(&at("2"), Some("east"));
        let original = t.snapshot().links;
        t.observe(&at("3"), None); // a teleport, a transport, a follow
        assert_eq!(t.current_id(), Some("s:3"));
        assert_eq!(t.state(), TrackingState::Confirmed);
        assert_eq!(t.snapshot().links, original);
        let destination = t.room("s:3").unwrap().clone();
        for r in t.rooms().filter(|r| r.id != "s:3") {
            assert!((r.x - destination.x).abs().max((r.y - destination.y).abs()) > 1.0);
        }
        t.observe(&at("4"), Some("north"));
        let s = t.snapshot();
        assert!(find_route(&s, "s:3", "s:4", false).is_some());
        assert!(find_route(&s, "s:1", "s:4", false).is_none());
        t.observe(&at("1"), None); // revisiting a known room keeps its topology
        assert_eq!(t.current_id(), Some("s:1"));
        assert_eq!(t.room_count(), 4);
        assert_eq!(t.link_count(), 2);
    }
}

#[test]
fn text_guess_before_metadata_relocalizes_to_the_saved_room() {
    let mut t = RoomMapTracker::new();
    t.observe(&text_room("Passenger Bunks", &["north"]), None);
    let saved = t.snapshot();
    let actual = saved.current_room_id.clone();
    let mut t = RoomMapTracker::from_snapshot(saved);
    t.observe(&text_room("Welcome tutorial", &["north"]), None);
    let guess = t.current_id().unwrap().to_string();
    let count = t.observation_count();
    assert!(t.refine_provisional_room(
        &guess,
        count,
        &text_room("Passenger Bunks", &["north"]).with_source(RoomSource::Gmcp)
    ));
    assert_eq!(t.room_count(), 1);
    assert_eq!(t.current_id().map(str::to_string), actual);
    assert!(t.snapshot().deleted_rooms.iter().any(|d| d.id == guess));
    assert_eq!(t.link_count(), 0);
}

#[test]
fn provisional_refinement_rejects_stale_or_invalid_evidence_and_preserves_manual_edits() {
    let mut t = RoomMapTracker::new();
    t.observe(&text_room("Guessed heading", &["north"]), None);
    let id = t.current_id().unwrap().to_string();
    let observation = RoomObservation::new(None, "Actual room", "", &[]).with_source(RoomSource::Gmcp);
    assert!(!t.refine_provisional_room(&id, 0, &observation));
    let long = RoomObservation {
        name: "x".repeat(513),
        ..observation.clone()
    };
    assert!(!t.refine_provisional_room(&id, 1, &long));
    assert_eq!(t.rooms().next().unwrap().name, "Guessed heading");
    let bad_area = RoomObservation {
        area: Some("bad\0area".into()),
        ..observation.clone()
    };
    assert!(t.refine_provisional_room(&id, 1, &bad_area));
    assert_eq!(t.rooms().next().unwrap().area, None);
    assert_eq!(t.current_id(), Some(id.as_str()));
    serialize(&t.snapshot()).unwrap();
    let edited = MapRoom {
        name: "My room name".into(),
        ..t.rooms().next().unwrap().clone()
    };
    t.upsert_room(edited);
    let count = t.observation_count();
    assert!(!t.refine_provisional_room(&id, count, &observation));
    assert_eq!(t.rooms().next().unwrap().name, "My room name");
}

#[test]
fn observed_return_to_a_distinctive_room_records_the_return_exit() {
    let mut t = RoomMapTracker::new();
    t.observe(&text_room("Passenger Bunks", &["north"]), None);
    let bunks = t.current_id().map(str::to_string);
    t.observe(&text_room("Cockpit", &["south"]), Some("north"));
    let cockpit = t.current_id().map(str::to_string);
    assert_eq!(t.link_count(), 1, "no return exit until the return is seen");
    for _ in 0..3 {
        t.observe(&text_room("Passenger Bunks", &["north"]), Some("south"));
        assert_eq!(t.current_id().map(str::to_string), bunks);
        t.observe(&text_room("Cockpit", &["south"]), Some("north"));
        assert_eq!(t.current_id().map(str::to_string), cockpit);
    }
    assert_eq!(t.room_count(), 2);
    assert_eq!(t.link_count(), 2);
    assert!(t.links().all(|l| !l.confirmed));
}

#[test]
fn identical_rooms_or_lost_position_cannot_prove_a_return_exit() {
    let mut t = RoomMapTracker::new();
    t.observe(&text_room("Corridor", &["north", "south"]), None);
    t.observe(&text_room("Corridor", &["north", "south"]), Some("north"));
    t.observe(&text_room("Corridor", &["north", "south"]), Some("south"));
    assert_eq!(t.current_id(), None);
    assert_eq!(t.link_count(), 1);
    for manual in [false, true] {
        let mut t = RoomMapTracker::new();
        t.observe(&text_room("Bunks", &["north"]), None);
        let bunks = t.current_id().unwrap().to_string();
        t.observe(&text_room("Cockpit", &["south"]), Some("north"));
        if manual {
            let cockpit = t.current_id().unwrap().to_string();
            t.set_current_room(&bunks);
            t.set_current_room(&cockpit);
        } else {
            t.lose_position();
            t.observe(&text_room("Cockpit", &["south"]), None);
        }
        t.observe(&text_room("Bunks", &["north"]), Some("south"));
        assert_eq!(t.current_id(), None, "manual {manual}");
        assert_eq!(t.link_count(), 1);
    }
}

#[test]
fn consecutive_rooms_narrow_identical_candidates_without_changing_the_graph() {
    let mut t = RoomMapTracker::from_snapshot(corridor_map());
    t.observe(&text_room("Corridor", &[]), None);
    assert_eq!(t.candidates().len(), 4);
    assert_eq!(t.current_id(), None);
    t.observe(&text_room("Corridor", &[]), Some("north"));
    let mut c = t.candidates();
    c.sort();
    assert_eq!(c, ["b", "c"]);
    t.observe(&text_room("Corridor", &[]), Some("north"));
    assert_eq!(t.current_id(), Some("c"));
    assert_eq!(t.state(), TrackingState::Inferred);
    assert_eq!(t.room_count(), 5);
    assert_eq!(t.link_count(), 4);
}

#[test]
fn identical_adjacent_new_rooms_are_not_collapsed_and_failed_moves_do_not_advance() {
    let mut t = RoomMapTracker::new();
    t.observe(&text_room("Corridor", &["north", "south"]), None);
    let first = t.current_id().unwrap().to_string();
    t.movement_failed();
    assert_eq!(t.current_id(), Some(first.as_str()));
    t.observe(&text_room("Corridor", &["north", "south"]), Some("north"));
    assert_eq!(t.room_count(), 2);
    assert_ne!(t.current_id(), Some(first.as_str()));
    let link = t.links().next().unwrap();
    assert_eq!(t.link_count(), 1);
    assert_eq!(link.direction, "north");
    assert_eq!(link.from_id, first);
    assert!(!link.confirmed);
}

#[test]
fn server_identifiers_win_over_identical_descriptions_and_do_not_assume_reverse_exits() {
    let mut t = RoomMapTracker::new();
    let forest = |id: &str, exits: &[(&str, Option<&str>)]| {
        RoomObservation::new(Some(id), "Forest", "Trees.", exits).with_source(RoomSource::Gmcp)
    };
    t.observe(&forest("101", &[("e", Some("102"))]), None);
    t.observe(&forest("102", &[]), Some("e"));
    assert_eq!(t.room_count(), 2);
    assert_eq!(t.current_id(), Some("s:102"));
    assert_eq!(t.state(), TrackingState::Confirmed);
    assert_eq!(t.link_count(), 1);
    assert_eq!(t.links().next().unwrap().direction, "east");
    t.observe(
        &RoomObservation::new(Some("101"), "Changed forest", "Night falls.", &[("e", Some("102"))])
            .with_source(RoomSource::Gmcp),
        None,
    );
    assert_eq!(t.room_count(), 2);
    assert_eq!(t.current_id(), Some("s:101"));
}

#[test]
fn unknown_room_after_teleport_does_not_create_an_invented_connection() {
    let mut t = RoomMapTracker::from_snapshot(corridor_map());
    t.observe(&RoomObservation::new(None, "Shrine", "A marble altar.", &[]), None);
    t.lose_position();
    t.observe(&text_room("Desert", &[]), None);
    assert_eq!(t.room_count(), 6);
    assert_eq!(t.link_count(), 4);
}

#[test]
fn lotj_tags_refine_the_same_room_instead_of_creating_disconnected_duplicates() {
    let mut t = RoomMapTracker::new();
    t.observe(
        &RoomObservation::new(None, "The Cockpit | Academy Transport Shuttle", "", &[("south", None)])
            .with_source(RoomSource::Gmcp),
        None,
    );
    t.observe(
        &RoomObservation::new(
            None,
            "The Cockpit | Academy Transport Shuttle  [Hotel]",
            "Panels glow.",
            &[("south", None)],
        ),
        None,
    );
    assert_eq!(t.room_count(), 1);
    let cockpit = t.current_id().unwrap().to_string();
    t.observe(
        &RoomObservation::new(
            None,
            "Passenger Bunks | Academy Transport Shuttle",
            "",
            &[("north", None)],
        )
        .with_source(RoomSource::Gmcp),
        Some("south"),
    );
    t.observe(
        &RoomObservation::new(
            None,
            "Passenger Bunks | Academy Transport Shuttle [HOSPITAL][Hotel][ENGINE]",
            "Rows of beds.",
            &[("north", None)],
        ),
        None,
    );
    assert_eq!(t.room_count(), 2);
    assert_eq!(t.link_count(), 1);
    let north = t.room(&cockpit).unwrap();
    let south = t.current().unwrap();
    assert_eq!(north.x, south.x);
    assert!(south.y < north.y);
    assert_eq!(t.links().next().unwrap().direction, "south");
}

#[test]
fn bracketed_room_identifiers_remain_distinct() {
    let mut t = RoomMapTracker::new();
    t.observe(&text_room("Cabin [101]", &["north"]), None);
    t.observe(&text_room("Cabin [102]", &["north"]), None);
    assert_eq!(t.room_count(), 2);
}

#[test]
fn exit_evidence_rejects_a_contradictory_lookalike() {
    let mut m = corridor_map();
    for r in &mut m.rooms {
        r.known_exits = vec![if r.id == "a" { "east" } else { "north" }.into()];
    }
    let mut t = RoomMapTracker::from_snapshot(m);
    t.observe(&text_room("Corridor", &["east"]), None);
    assert_eq!(t.current_id(), Some("a"));
}

#[test]
fn too_many_candidates_remain_unknown_instead_of_choosing_from_a_truncated_set() {
    let rooms = (0..300)
        .map(|i| {
            MapRoom::new(
                &i.to_string(),
                "Corridor",
                "Stone walls and a worn floor.",
                None,
                f64::from(i),
                0.0,
                0.0,
                true,
            )
        })
        .collect();
    let mut t = RoomMapTracker::from_snapshot(MapSnapshot {
        source: RoomSource::Text,
        ..MapSnapshot::of(rooms, Vec::new())
    });
    t.observe(&text_room("Corridor", &[]), None);
    assert_eq!(t.state(), TrackingState::Unknown);
    assert_eq!(t.current_id(), None);
    assert!(t.candidates().is_empty());
}

#[test]
fn two_session_saves_keep_both_discovered_branches() {
    let (db, dir) = temp_db("branches");
    let store = MapStore::new(db);
    let mut first = RoomMapTracker::new();
    let mut second = RoomMapTracker::new();
    first.observe(&id_room("1", "Hall", &[]), None);
    second.observe(&id_room("2", "Tower", &[]), None);
    store.save(&world(), &first.snapshot()).unwrap();
    store.save(&world(), &second.snapshot()).unwrap();
    assert_eq!(store.load(&world()).unwrap().unwrap().rooms.len(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn map_cache_is_scoped_to_the_world_and_restores_the_graph_without_assuming_position() {
    let (db, dir) = temp_db("scoped");
    let store = MapStore::new(db);
    let at = |host: &str, port| MapWorld::Endpoint {
        host: host.into(),
        port,
    };
    store.save(&at("mud.example", 4000), &corridor_map()).unwrap();
    let loaded = store.load(&at("MUD.EXAMPLE", 4000)).unwrap().unwrap();
    assert_eq!(loaded.rooms.len(), 5);
    assert!(store.load(&at("mud.example", 4001)).unwrap().is_none());
    assert_eq!(RoomMapTracker::from_snapshot(loaded).current_id(), None);
    let _ = std::fs::remove_dir_all(dir);
}

// ---- MapIdentityTests --------------------------------------------------------------------

fn kafrene(id: &str, name: &str, exit: &str) -> RoomObservation {
    RoomObservation::new(Some(id), name, "", &[(exit, None)])
        .with_area("Ring of Kafrene")
        .with_source(RoomSource::Gmcp)
}

#[test]
fn server_identity_upgrades_legacy_rooms_and_witnessed_links_without_losing_edits() {
    for duplicates in [false, true] {
        let mut cockpit = MapRoom::new(
            "t:cockpit",
            "My cockpit",
            "Saved description",
            None,
            0.0,
            1.0,
            0.0,
            true,
        );
        cockpit.observed_name = Some("The Cockpit [Hotel]".into());
        cockpit.known_exits = vec!["south".into()];
        cockpit.notes = "Keep this".into();
        cockpit.is_manually_edited = true;
        cockpit.revision = 1;
        let mut bunks = MapRoom::new("t:bunks", "Passenger Bunks", "", None, 0.0, 0.0, 0.0, true);
        bunks.known_exits = vec!["north".into()];
        bunks.is_manually_edited = true;
        bunks.revision = 1;
        let mut rooms = vec![cockpit, bunks];
        let edited = |l: MapLink| MapLink {
            is_manually_edited: true,
            ..l
        };
        let mut links = vec![
            edited(MapLink::new("t:cockpit", "t:bunks", "south", false)),
            edited(MapLink::new("t:bunks", "t:cockpit", "north", false)),
        ];
        if duplicates {
            let mut a = MapRoom::new(
                "s:561",
                "The Cockpit",
                "",
                Some("Ring of Kafrene"),
                4.0,
                0.0,
                0.0,
                false,
            )
            .with_server_id("561");
            a.known_exits = vec!["south".into()];
            let mut b = MapRoom::new(
                "s:562",
                "Passenger Bunks",
                "",
                Some("Ring of Kafrene"),
                4.0,
                -1.0,
                0.0,
                false,
            )
            .with_server_id("562");
            b.known_exits = vec!["north".into()];
            rooms.extend([a, b]);
            links.push(MapLink::new("s:561", "s:562", "south", true));
        }
        let stale = MapSnapshot {
            source: RoomSource::Gmcp,
            ..MapSnapshot::of(rooms, links)
        };
        let mut t = RoomMapTracker::from_snapshot(stale.clone());
        t.observe(&kafrene("561", "The Cockpit", "south"), None);
        t.observe(&kafrene("562", "Passenger Bunks", "north"), Some("south"));
        t.observe(&kafrene("561", "The Cockpit", "south"), Some("north"));
        let m = t.snapshot();
        assert_eq!(m.rooms.len(), 2, "duplicates {duplicates}");
        assert!(m.rooms.iter().all(|r| !r.provisional));
        let c = m.rooms.iter().find(|r| r.server_id.as_deref() == Some("561")).unwrap();
        assert_eq!(c.notes, "Keep this");
        assert_eq!(c.name, "My cockpit");
        assert_eq!(c.y, 1.0);
        assert!(find_route(&m, "s:561", "s:562", false).is_some());
        assert!(find_route(&m, "s:562", "s:561", false).is_some());
        assert_eq!(m.links.len(), 2);
        serialize(&m).unwrap();
        let (db, dir) = temp_db(&format!("identity-{duplicates}"));
        let store = MapStore::new(db);
        store.save(&world(), &stale).unwrap();
        store.save(&world(), &m).unwrap();
        store.save(&world(), &stale).unwrap();
        let restored = store.load(&world()).unwrap().unwrap();
        assert_eq!(restored.rooms.len(), 2);
        assert_eq!(restored.links.len(), 2);
        assert!(find_route(&restored, "s:561", "s:562", false).is_some());
        assert!(find_route(&restored, "s:562", "s:561", false).is_some());
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[test]
fn ambiguous_legacy_rooms_are_not_merged_by_title() {
    let mut one = MapRoom::new("t:one", "Corridor", "", None, 0.0, 0.0, 0.0, true);
    one.known_exits = vec!["north".into()];
    let mut two = MapRoom::new("t:two", "Corridor", "", None, 1.0, 0.0, 0.0, true);
    two.known_exits = vec!["north".into()];
    let mut t = RoomMapTracker::from_snapshot(MapSnapshot::of(vec![one, two], Vec::new()));
    t.observe(&kafrene("123", "Corridor", "north"), None);
    assert_eq!(t.room_count(), 3);
    assert!(t.snapshot().room_aliases.is_empty());
}

#[test]
fn movement_to_an_identically_named_room_does_not_adopt_the_origin_identity() {
    let mut t = RoomMapTracker::new();
    let mut first = kafrene("123", "Corridor", "north");
    first.server_id = None;
    t.observe(&first, None);
    let origin = t.current_id().unwrap().to_string();
    t.observe(&kafrene("124", "Corridor", "north"), Some("north"));
    assert_eq!(t.room_count(), 2);
    assert_eq!(t.links().next().unwrap().from_id, origin);
    assert_eq!(t.link_count(), 1);
    assert!(t.snapshot().room_aliases.is_empty());
}

#[test]
fn confirming_a_manual_edge_preserves_restrictions_and_does_not_redirect_it() {
    let mut t = RoomMapTracker::new();
    t.observe(&kafrene("561", "The Cockpit", "south"), None);
    t.observe(&kafrene("562", "Passenger Bunks", "north"), Some("south"));
    t.upsert_link(MapLink {
        is_locked: true,
        door_state: DoorState::Closed,
        command: Some("enter hatch".into()),
        weight: 5.0,
        ..MapLink::new("s:561", "s:562", "south", false)
    });
    t.observe(&kafrene("561", "The Cockpit", "south"), None);
    t.observe(&kafrene("562", "Passenger Bunks", "north"), Some("south"));
    assert_eq!(t.link_count(), 1);
    let link = t.links().next().unwrap().clone();
    assert!(link.confirmed && link.is_locked);
    assert_eq!(link.door_state, DoorState::Closed);
    assert_eq!(link.command.as_deref(), Some("enter hatch"));
    assert_eq!(link.weight, 5.0);
    assert!(find_route(&t.snapshot(), "s:561", "s:562", false).is_none());
    t.observe(&kafrene("561", "The Cockpit", "south"), None);
    t.observe(&kafrene("563", "Elsewhere", "north"), Some("south"));
    assert_eq!(t.links().next().unwrap().to_id, "s:562");
    assert_eq!(t.link_count(), 1);
}

#[test]
fn identity_merge_keeps_edited_exit_restrictions_over_automatic_duplicate_edges() {
    let mut old = MapRoom::new("t:old", "The Cockpit", "", None, 0.0, 1.0, 0.0, true);
    old.known_exits = vec!["south".into()];
    let mut a = MapRoom::new("s:561", "The Cockpit", "", None, 4.0, 0.0, 0.0, false).with_server_id("561");
    a.known_exits = vec!["south".into()];
    let mut b = MapRoom::new("s:562", "Passenger Bunks", "", None, 4.0, -1.0, 0.0, false).with_server_id("562");
    b.known_exits = vec!["north".into()];
    let links = vec![
        MapLink {
            is_manually_edited: true,
            is_locked: true,
            weight: 9.0,
            ..MapLink::new("t:old", "s:562", "south", false)
        },
        MapLink::new("s:561", "s:562", "south", true),
    ];
    let mut t = RoomMapTracker::from_snapshot(MapSnapshot::of(vec![old, a, b], links));
    t.observe(&kafrene("561", "The Cockpit", "south"), None);
    assert_eq!(t.link_count(), 1);
    let link = t.links().next().unwrap();
    assert!(link.is_locked);
    assert_eq!(link.weight, 9.0);
    assert!(find_route(&t.snapshot(), "s:561", "s:562", false).is_none());
}

// ---- ExpandedMapTests --------------------------------------------------------------------

#[test]
fn renaming_a_text_room_preserves_recognition_evidence_after_reload() {
    let mut t = RoomMapTracker::new();
    let observed = RoomObservation::new(None, "Hall", "Stone walls.", &[]);
    t.observe(&observed, None);
    let id = t.current_id().map(str::to_string);
    let edited = MapRoom {
        name: "My camp".into(),
        description: "Rest here".into(),
        area: Some("My area".into()),
        ..t.rooms().next().unwrap().clone()
    };
    t.upsert_room(edited);
    let mut t = RoomMapTracker::from_snapshot(deserialize(&serialize(&t.snapshot()).unwrap()).unwrap());
    t.observe(&observed, None);
    assert_eq!(t.current_id().map(str::to_string), id);
    assert_eq!(t.room_count(), 1);
    assert_eq!(t.rooms().next().unwrap().name, "My camp");
}

#[test]
fn oversized_optional_protocol_fields_cannot_poison_the_cache() {
    let mut t = RoomMapTracker::new();
    let mut o = RoomObservation::new(Some("1"), "Hall", "", &[]);
    o.exits = (0..100).map(|i| (format!("exit{i}"), None)).collect();
    o.area = Some("a".repeat(1000));
    o.environment = Some("a".repeat(2000));
    o.symbol = Some("a".repeat(100));
    o.x = Some(f64::INFINITY);
    t.observe(&o, None);
    let loaded = deserialize(&serialize(&t.snapshot()).unwrap()).unwrap();
    assert_eq!(loaded.rooms.len(), 1);
    assert_eq!(loaded.rooms[0].environment, None);
    assert!(loaded.rooms[0].known_exits.len() <= 64);
}

#[test]
fn undo_exit_deletion_allows_later_observed_destination_changes() {
    let mut t = RoomMapTracker::new();
    t.observe(&id_room("1", "Hall", &[("north", Some("2"))]), None);
    assert!(t.remove_link("s:1", "north"));
    assert!(t.undo());
    t.observe(&id_room("1", "Hall", &[("north", Some("3"))]), None);
    assert_eq!(t.link_count(), 1);
    assert_eq!(t.links().next().unwrap().to_id, "s:3");
}

#[test]
fn unassigned_area_grid_setting_round_trips() {
    let mut t = RoomMapTracker::new();
    t.set_area_settings(MapAreaSettings::new("", true));
    let loaded = deserialize(&serialize(&t.snapshot()).unwrap()).unwrap();
    assert_eq!(loaded.area_settings.len(), 1);
    assert_eq!(loaded.area_settings[0].area, "");
    assert!(loaded.area_settings[0].grid_mode);
}

#[test]
fn legacy_cache_without_new_fields_retains_safe_defaults() {
    let m = deserialize(
        r#"{"Rooms":[{"Id":"a","Name":"Hall","Description":"","Area":null,"X":0,"Y":0,"Z":0,"Provisional":true}],
            "Links":[],"CandidateRoomIds":[],"CurrentRoomId":null,"State":4,"Source":0,"ObservationCount":0}"#,
    )
    .unwrap();
    assert_eq!(m.rooms[0].weight, 1.0);
    assert_eq!(m.rooms[0].notes, "");
    assert!(m.area_settings.is_empty() && m.deleted_rooms.is_empty());
}

#[test]
fn stale_room_observations_cannot_overwrite_saved_manual_edits() {
    let (db, dir) = temp_db("stale-edits");
    let store = MapStore::new(db.clone());
    store.save(&world(), &map(vec![room("s:1")])).unwrap();
    let mut edited = RoomMapTracker::from_snapshot(store.load(&world()).unwrap().unwrap());
    let mut stale = RoomMapTracker::from_snapshot(store.load(&world()).unwrap().unwrap());
    let changed = MapRoom {
        name: "My camp".into(),
        color: Some("#112233".into()),
        ..edited.rooms().next().unwrap().clone()
    };
    edited.upsert_room(changed);
    store.save(&world(), &edited.snapshot()).unwrap();
    stale.observe(&RoomObservation::new(Some("1"), "Changed server hall", "", &[]), None);
    MapStore::new(db).save(&world(), &stale.snapshot()).unwrap();
    let saved = store.load(&world()).unwrap().unwrap();
    assert_eq!(saved.rooms.len(), 1);
    assert_eq!(saved.rooms[0].name, "My camp");
    assert_eq!(saved.rooms[0].color.as_deref(), Some("#112233"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn manual_room_and_exit_edits_survive_observation() {
    let mut t = RoomMapTracker::new();
    t.observe(&id_room("1", "Hall", &[("n", Some("2"))]), None);
    let hall = t.rooms().next().unwrap().clone();
    assert!(t.upsert_room(MapRoom {
        name: "My hall".into(),
        x: 12.0,
        environment: Some("forest".into()),
        notes: "home".into(),
        ..hall
    }));
    let exit = t.links().next().unwrap().clone();
    assert!(t.upsert_link(MapLink {
        command: Some("enter portal".into()),
        is_locked: true,
        ..exit
    }));
    let mut later = RoomObservation::new(Some("1"), "Server hall", "New prose", &[("n", Some("3"))]);
    later.x = Some(99.0);
    later.environment = Some("city".into());
    t.observe(&later, None);
    let r = t.rooms().next().unwrap();
    assert_eq!(t.room_count(), 1);
    assert_eq!(r.name, "My hall");
    assert_eq!(r.x, 12.0);
    assert_eq!(r.environment.as_deref(), Some("forest"));
    assert_eq!(r.notes, "home");
    let l = t.links().next().unwrap();
    assert_eq!(t.link_count(), 1);
    assert_eq!(l.to_id, "s:2");
    assert_eq!(l.command.as_deref(), Some("enter portal"));
    assert!(l.is_locked);
}

fn abc() -> MapSnapshot {
    MapSnapshot {
        links: vec![
            MapLink::new("a", "b", "north", true),
            MapLink::new("b", "c", "east", true),
        ],
        ..map(vec![room("a"), room("b"), room("c")])
    }
}

#[test]
fn delete_undo_redo_and_merge_maintain_the_graph() {
    let mut t = RoomMapTracker::from_snapshot(abc());
    assert!(t.set_current_room("b"));
    assert!(t.merge_rooms("b", "a"));
    assert_eq!(t.current_id(), Some("a"));
    assert_eq!(t.link_count(), 1);
    assert_eq!(t.links().next().unwrap().to_id, "c");
    assert!(t.undo());
    assert_eq!(t.room_count(), 3);
    assert!(t.redo());
    assert_eq!(t.room_count(), 2);
    assert!(t.remove_room("a"));
    assert_eq!(t.link_count(), 0);
    assert_eq!(t.current_id(), None);
}

/// Each undo step has its own number: the undo toast undoes only while its delete is the step
/// Undo would take back (a later edit, or an Undo from the Edit menu, moves it on).
#[test]
fn every_undo_step_has_a_number_that_follows_undo_and_redo() {
    let mut t = RoomMapTracker::from_snapshot(abc());
    assert_eq!(t.last_edit(), None);
    assert_eq!(t.remove_rooms(&["a".into(), "b".into()]), 2);
    let delete = t.last_edit().expect("a step");
    assert!(t.upsert_room(room("d")));
    let later = t.last_edit().unwrap();
    assert!(later > delete);
    assert!(t.undo());
    assert_eq!(t.last_edit(), Some(delete), "the later step undone, the delete is next");
    assert!(t.undo());
    assert_eq!(t.room_count(), 3);
    assert_ne!(t.last_edit(), Some(delete));
    assert!(t.redo());
    assert_eq!(t.last_edit(), Some(delete), "redo brings the same number back");
}

#[test]
fn deleted_rooms_and_exits_cannot_be_resurrected_by_stale_sessions() {
    let (db, dir) = temp_db("tombstones");
    let store = MapStore::new(db);
    store.save(&world(), &abc()).unwrap();
    let mut active = RoomMapTracker::from_snapshot(store.load(&world()).unwrap().unwrap());
    let stale = RoomMapTracker::from_snapshot(store.load(&world()).unwrap().unwrap());
    assert!(active.remove_link("a", "north"));
    assert!(active.remove_room("c"));
    store.save(&world(), &active.snapshot()).unwrap();
    store.save(&world(), &stale.snapshot()).unwrap();
    let saved = store.load(&world()).unwrap().unwrap();
    assert_eq!(saved.rooms.len(), 2);
    assert!(saved.links.is_empty());
    assert!(active.undo());
    store.save(&world(), &active.snapshot()).unwrap();
    assert_eq!(store.load(&world()).unwrap().unwrap().rooms.len(), 3);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn route_uses_directed_weighted_safe_exits() {
    let mut m = map(vec![
        room("a"),
        MapRoom {
            weight: 8.0,
            ..room("b")
        },
        room("c"),
        room("d"),
    ]);
    m.links = vec![
        MapLink::new("a", "b", "north", true),
        MapLink::new("b", "d", "east", true),
        MapLink::new("a", "c", "east", true),
        MapLink::new("c", "d", "north", true),
    ];
    let route = find_route(&m, "a", "d", false).unwrap();
    assert_eq!(route.cost, 2.0);
    assert_eq!(route.steps[0].to_id, "c");
    assert!(find_route(&m, "d", "a", false).is_none());
    for l in &mut m.links {
        if l.from_id == "c" {
            l.door_state = DoorState::Locked;
        }
    }
    assert_eq!(find_route(&m, "a", "d", false).unwrap().cost, 9.0);
    for l in &mut m.links {
        if l.from_id == "c" {
            l.door_state = DoorState::Closed;
        }
    }
    assert_eq!(
        find_route(&m, "a", "d", false).unwrap().cost,
        9.0,
        "closed doors are avoided too"
    );
    for r in &mut m.rooms {
        if r.id == "b" {
            r.is_locked = true;
        }
    }
    assert!(find_route(&m, "a", "d", false).is_none());
}

#[test]
fn exit_weights_override_room_weights_and_locked_exits_are_avoided() {
    let mut m = map(vec![room("a"), room("b"), room("c")]);
    m.links = vec![
        MapLink {
            weight: 5.0,
            ..MapLink::new("a", "c", "east", true)
        },
        MapLink::new("a", "b", "north", true),
        MapLink::new("b", "c", "south", true),
    ];
    let route = find_route(&m, "a", "c", false).unwrap();
    assert_eq!((route.cost, route.steps.len()), (2.0, 2));
    m.links[1].is_locked = true;
    assert_eq!(find_route(&m, "a", "c", false).unwrap().cost, 5.0);
    assert_eq!(find_route(&m, "a", "c", false).unwrap().commands(), "east");
}

#[test]
fn inferred_routes_require_explicit_opt_in() {
    let mut m = map(vec![room("a"), room("b")]);
    m.links = vec![MapLink::new("a", "b", "east", false)];
    assert!(find_route(&m, "a", "b", false).is_none());
    assert!(find_route(&m, "a", "b", true).is_some());
    // Provisional rooms too.
    let mut m = map(vec![
        room("a"),
        MapRoom {
            provisional: true,
            ..room("b")
        },
    ]);
    m.links = vec![MapLink::new("a", "b", "east", true)];
    assert!(find_route(&m, "a", "b", false).is_none());
    assert!(find_route(&m, "a", "b", true).is_some());
}

#[test]
fn map_files_round_trip_and_accept_legacy_snapshots() {
    let mut m = map(vec![MapRoom {
        environment: Some("forest".into()),
        symbol: Some("♣".into()),
        notes: "camp".into(),
        ..room("a")
    }]);
    m.area_settings = vec![MapAreaSettings::new("Keep", true)];
    let loaded = deserialize(&serialize(&m).unwrap()).unwrap();
    assert_eq!(loaded.rooms[0].notes, "camp");
    assert!(loaded.area_settings[0].grid_mode);
    assert_eq!(deserialize(&serde_json::to_string(&m).unwrap()).unwrap().rooms.len(), 1);
    assert!(deserialize(r#"{"Version":999,"Map":{}}"#).is_err());
    let mut bad = m.clone();
    bad.rooms[0].weight = -1.0;
    assert!(deserialize(&serde_json::to_string(&bad).unwrap()).is_err());
}

#[test]
fn renaming_an_exit_and_adding_its_return_is_atomic_and_undoable() {
    let mut m = map(vec![room("a"), room("b")]);
    m.links = vec![MapLink::new("a", "b", "east", true)];
    let mut t = RoomMapTracker::from_snapshot(m);
    let bad_return = MapLink {
        weight: -1.0,
        ..MapLink::new("b", "a", "south", true)
    };
    assert!(!t.edit_link(MapLink::new("a", "b", "north", true), Some("east"), Some(bad_return)));
    assert_eq!(t.links().next().unwrap().direction, "east");
    assert!(t.edit_link(
        MapLink::new("a", "b", "north", true),
        Some("east"),
        Some(MapLink::new("b", "a", "south", true))
    ));
    assert_eq!(t.link_count(), 2);
    assert!(t.undo());
    assert_eq!(t.link_count(), 1);
    assert_eq!(t.links().next().unwrap().direction, "east");
    assert!(t.redo());
    assert_eq!(t.link_count(), 2);
}

#[test]
fn import_is_one_undoable_replacement_and_tombstones_old_data() {
    let mut t = RoomMapTracker::from_snapshot(map(vec![room("a"), room("b")]));
    assert!(t.replace_map(&map(vec![room("c")])));
    assert_eq!(t.rooms().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["c"]);
    assert_eq!(t.snapshot().deleted_rooms.len(), 2);
    assert!(t.undo());
    let mut ids: Vec<_> = t.rooms().map(|r| r.id.clone()).collect();
    ids.sort();
    assert_eq!(ids, ["a", "b"]);
    assert!(t.redo());
    assert_eq!(t.rooms().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["c"]);
}

#[test]
fn undo_preserves_rooms_discovered_after_the_manual_edit() {
    let mut t = RoomMapTracker::from_snapshot(map(vec![room("a")]));
    t.upsert_room(MapRoom {
        notes: "camp".into(),
        ..room("a")
    });
    t.observe(&RoomObservation::new(Some("2"), "New discovery", "", &[]), None);
    assert!(t.undo());
    assert_eq!(t.room_count(), 2);
    assert_eq!(t.room("a").unwrap().notes, "");
}

#[test]
fn merge_remaps_subsequent_authoritative_observations_after_reload() {
    let mut t = RoomMapTracker::from_snapshot(map(vec![room("s:1"), room("s:2")]));
    assert!(t.merge_rooms("s:1", "s:2"));
    let mut t = RoomMapTracker::from_snapshot(deserialize(&serialize(&t.snapshot()).unwrap()).unwrap());
    t.observe(&RoomObservation::new(Some("1"), "Hall", "", &[]), None);
    assert_eq!(t.room_count(), 1);
    assert_eq!(t.current_id(), Some("s:2"));
}

#[test]
fn malformed_map_imports_are_rejected() {
    for json in [
        r#"{"Rooms":[null],"Links":[],"CandidateRoomIds":[]}"#,
        r#"{"Rooms":[],"Links":[],"CandidateRoomIds":[],"AreaSettings":null}"#,
        "[]",
        "null",
    ] {
        assert!(deserialize(json).is_err(), "{json}");
    }
}

#[test]
fn msdp_coordinate_and_terrain_updates_never_stitch_across_rooms() {
    let r = from_msdp(b"\x01ROOM_VNUM\x0242\x01ROOM_NAME\x02Hall\x01ROOM_TERRAIN\x02forest\x01ROOM_X\x0212\x01ROOM_Y\x02-4\x01ROOM_Z\x022").unwrap();
    assert_eq!(r.environment.as_deref(), Some("forest"));
    assert_eq!((r.x, r.y), (Some(12.0), Some(-4.0)));
    assert!(from_msdp(b"\x01ROOM_X\x0299").is_none());
    let r = from_msdp(b"\x01ROOM_VNUM\x0243\x01ROOM_NAME\x02Other").unwrap();
    assert!(r.x.is_none() && r.environment.is_none());
}

fn gmcp(text: &str) -> crate::protocol::GmcpMessage {
    parse_gmcp(text.as_bytes()).unwrap()
}

#[test]
fn protocol_supplied_coordinates_and_terrain_are_coherent_and_finite() {
    let r = from_gmcp(&gmcp(
        r#"Room.Info {"num":1,"name":"Hall","environment":"forest","coord":{"x":12,"y":-3,"z":2}}"#,
    ))
    .unwrap();
    assert_eq!(r.environment.as_deref(), Some("forest"));
    assert_eq!((r.x, r.y, r.z), (Some(12.0), Some(-3.0), Some(2.0)));
    let malformed = from_gmcp(&gmcp(r#"Room.Info {"num":2,"name":"Hall","x":"NaN","y":1e999}"#)).unwrap();
    assert!(malformed.x.is_none() && malformed.y.is_none() && malformed.environment.is_none());
    let mut t = RoomMapTracker::new();
    t.observe(&r, None);
    assert_eq!(t.rooms().next().unwrap().x, 12.0);
}

// ---- RoomProtocolTests (the decoder) -----------------------------------------------------

#[test]
fn lotj_vnum_identifies_the_room_and_exit_flags_do_not_become_room_ids() {
    let r = from_gmcp(&gmcp(
        r#"Room.Info {"name":"The Cockpit | Academy Transport Shuttle ","vnum":561,"exits":{"south":"O","up":"C"},"planet":"Ring of Kafrene"}"#,
    ))
    .unwrap();
    assert_eq!(r.server_id.as_deref(), Some("561"));
    assert_eq!(r.area.as_deref(), Some("Ring of Kafrene"));
    assert_eq!(r.name, "The Cockpit | Academy Transport Shuttle");
    assert!(r.exits_provided);
    assert_eq!(r.exits["south"], None);
    assert_eq!(r.exits["up"], None);
    let mut t = RoomMapTracker::new();
    t.observe(&r, None);
    assert_eq!(t.state(), TrackingState::Confirmed);
    assert_eq!(t.current_id(), Some("s:561"));
    assert_eq!(t.link_count(), 0);
}

#[test]
fn vnum_alias_preserves_destination_ids_and_prefers_the_area() {
    for id in ["561", "\"561\""] {
        let r = from_gmcp(&gmcp(&format!(
            r#"Room.Info {{"vnum":{id},"name":"Hall","planet":"Planet","area":"Ship","exits":{{"north":562}}}}"#
        )))
        .unwrap();
        assert_eq!(r.server_id.as_deref(), Some("561"));
        assert_eq!(r.exits["north"].as_deref(), Some("562"));
        assert_eq!(r.area.as_deref(), Some("Ship"));
    }
}

#[test]
fn gmcp_room_normalizes_identity_description_and_exit_ids() {
    let r = from_gmcp(&gmcp(
        r#"room.info {"num":42,"name":"Hall","desc":"Marble","area":"Keep","exits":{"n":43,"s":-1}}"#,
    ))
    .unwrap();
    assert_eq!(r.server_id.as_deref(), Some("42"));
    assert_eq!((r.name.as_str(), r.description.as_str()), ("Hall", "Marble"));
    assert_eq!(r.area.as_deref(), Some("Keep"));
    assert_eq!(r.exits["north"].as_deref(), Some("43"));
    assert_eq!(r.exits["south"], None);
    assert_eq!(r.source, RoomSource::Gmcp);
}

#[test]
fn sentinel_ids_never_become_authoritative() {
    for id in [
        "null",
        "-1",
        "0",
        "false",
        "{}",
        "\"unknown\"",
        "\"false\"",
        "\"NaN\"",
        "\"Infinity\"",
        "\"-2\"",
        "\"1.5\"",
        "\"\"",
    ] {
        let r = from_gmcp(&gmcp(&format!(r#"Room.Info {{"id":{id},"name":"Hall"}}"#))).unwrap();
        assert_eq!(r.server_id, None, "{id}");
    }
    let r = from_gmcp(&gmcp(r#"Room.Info {"id":"1e3","name":"Hall"}"#)).unwrap();
    assert_eq!(r.server_id.as_deref(), Some("1000"));
}

#[test]
fn gmcp_alternate_names_and_opaque_ids_are_accepted() {
    let r = from_gmcp(&gmcp(
        r#"Room.Info {"id":"zone:42","name":"Hall","description":"Stone","exits":["e","up"]}"#,
    ))
    .unwrap();
    assert_eq!(r.server_id.as_deref(), Some("zone:42"));
    assert_eq!(r.description, "Stone");
    assert!(r.exits.contains_key("east"));
    assert_eq!(r.exits["up"], None);
}

#[test]
fn malformed_or_unrelated_gmcp_is_ignored() {
    for text in [
        "Room.Info {",
        r#"Char.Vitals {"name":"Hall"}"#,
        "Room.Info []",
        r#"Room.Info {"num":42}"#,
    ] {
        assert!(from_gmcp(&gmcp(text)).is_none(), "{text}");
    }
    let huge = format!(r#"Room.Info {{"name":"{}"}}"#, "a".repeat(20_000));
    assert!(from_gmcp(&gmcp(&huge)).is_none());
}

#[test]
fn msdp_decodes_a_coherent_room_and_nested_exits() {
    let r = from_msdp(
        b"\x01ROOM_VNUM\x0242\x01ROOM_NAME\x02Hall\x01AREA_NAME\x02Keep\x01ROOM_EXITS\x02\x03\x01n\x0243\x01s\x020\x04",
    )
    .unwrap();
    assert_eq!(r.server_id.as_deref(), Some("42"));
    assert_eq!(r.name, "Hall");
    assert_eq!(r.area.as_deref(), Some("Keep"));
    assert_eq!(r.exits["north"].as_deref(), Some("43"));
    assert_eq!(r.exits["south"], None);
    assert_eq!(r.source, RoomSource::Msdp);
}

#[test]
fn msdp_partial_updates_never_reuse_an_old_name_for_a_new_id() {
    assert!(from_msdp(b"\x01ROOM_NAME\x02Old hall").is_none());
    assert!(from_msdp(b"\x01ROOM_VNUM\x0299").is_none());
    assert!(from_msdp(b"\x01ROOM_VNUM\x0299\x01ROOM_EXITS\x02\x03\x01n\x024").is_none());
}

#[test]
fn compound_msdp_room_provides_a_coherent_observation() {
    let r = from_msdp(
        b"\x01ROOM\x02\x03\x01VNUM\x0242\x01NAME\x02Hall\x01AREA\x02Keep\x01EXITS\x02\x03\x01n\x0243\x04\x04",
    )
    .unwrap();
    assert_eq!(r.server_id.as_deref(), Some("42"));
    assert_eq!(r.exits["north"].as_deref(), Some("43"));
    assert_eq!(r.area.as_deref(), Some("Keep"));
}

#[test]
fn msdp_bounds_malformed_tables_and_deep_nesting() {
    let mut deep = b"\x01ROOM\x02".to_vec();
    for _ in 0..30 {
        deep.extend_from_slice(b"\x03\x01ROOM\x02");
    }
    deep.push(b'x');
    assert!(from_msdp(&deep).is_none());
    assert!(from_msdp(&[0u8; 20_000]).is_none());
    assert!(from_msdp(b"\x01ROOM_VNUM\x0242\x01ROOM_NAME\x02Hall\x04").is_none());
    assert!(from_msdp(b"\x01ROOM_VNUM\x0242\x01ROOM_NAME\x02Hall\x01ROOM_NAME\x02Other").is_none());
}

// ---- RoomSearchTests ---------------------------------------------------------------------

fn seen(id: &str, name: &str, description: &str) -> MapRoom {
    MapRoom::new(id, name, description, None, 0.0, 0.0, 0.0, false)
}

fn edited_room(id: &str, label: &str, name: Option<&str>, description: Option<&str>) -> MapRoom {
    MapRoom {
        is_manually_edited: true,
        observed_name: name.map(str::to_string),
        observed_description: description.map(str::to_string),
        ..MapRoom::new(id, label, "", None, 0.0, 0.0, 0.0, false)
    }
}

fn ids(rooms: Vec<&MapRoom>) -> Vec<&str> {
    let mut ids: Vec<&str> = rooms.iter().map(|r| r.id.as_str()).collect();
    ids.sort();
    ids
}

#[test]
fn every_term_must_match_the_observed_name_or_description() {
    let rooms = [
        seen("a", "Ancient Temple", "A crumbling temple beside a quiet garden."),
        seen("b", "Temple Gate", "Guarded stone gate."),
        seen("c", "Garden Path", "A gravel path through a garden."),
    ];
    assert_eq!(ids(search(&rooms, "temple garden")), ["a"]);
}

#[test]
fn a_quoted_phrase_matches_only_as_a_contiguous_run() {
    let rooms = [
        seen("a", "Ancient Temple", "A crumbling temple beside a quiet garden."),
        seen("b", "Temple of the Ancient Kings", "Old stonework."),
    ];
    assert_eq!(ids(search(&rooms, "\"ancient temple\"")), ["a"]);
    assert_eq!(ids(search(&rooms, "ancient temple")), ["a", "b"]);
    assert_eq!(parse_terms("  \"a b\" c\"d"), ["a b", "c", "d"]);
}

#[test]
fn matching_is_case_insensitive() {
    let rooms = [seen("a", "Ancient Temple", "A crumbling TEMPLE beside a quiet garden.")];
    assert_eq!(ids(search(&rooms, "TEMPLE")), ["a"]);
    assert_eq!(ids(search(&rooms, "temple")), ["a"]);
}

#[test]
fn rooms_without_any_observed_text_are_never_matched() {
    let rooms = [
        edited_room("a", "Temple (guess)", None, None),
        seen("b", "Temple", ""),
        edited_room("c", "The Old Shrine", Some("Sunken Temple"), Some("A temple in ruins.")),
    ];
    assert_eq!(ids(search(&rooms, "temple")), ["b", "c"]);
}

#[test]
fn an_empty_query_matches_nothing() {
    for q in ["", "   "] {
        assert!(search(&[seen("a", "Temple", "A temple.")], q).is_empty());
    }
}

// ---- TextRoomObserverTests ---------------------------------------------------------------

#[test]
fn fragmented_colored_room_waits_for_completed_exits_and_ignores_occupants() {
    let mut p = TextRoomObserver::new();
    assert!(
        p.feed("\x1b[1mThe Hall\x1b[0m\r\nStone arches rise above you.\r\nA guard is standing here.\r\n\r\nExits: nor")
            .is_empty()
    );
    let rooms = p.feed("th east\r\n> ");
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0].name, "The Hall");
    assert_eq!(rooms[0].description, "Stone arches rise above you.");
    let mut exits: Vec<&str> = rooms[0].exits.keys().map(String::as_str).collect();
    exits.sort();
    assert_eq!(exits, ["east", "north"]);
    assert!(p.feed("Somebody says: hello!\r\n").is_empty());
}

#[test]
fn lotj_directional_exit_lines_are_one_observation() {
    let mut p = TextRoomObserver::new();
    assert!(
        p.feed("\x1b[1mThe Cockpit | Academy Transport Shuttle\x1b[0m\nPanels glow in the dark.\n\nObvious exits:\nNorth - A passage\n")
            .is_empty()
    );
    let rooms = p.feed("South - The cabin\n\n*((==HP==((||||\n");
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0].name, "The Cockpit | Academy Transport Shuttle");
    assert_eq!(rooms[0].exits.len(), 2);
}

#[test]
fn multiline_exits_finish_at_a_recognizable_unterminated_prompt() {
    for prompt in ["HP: 100 MV: 80> ", "> "] {
        let mut p = TextRoomObserver::new();
        let rooms = p.feed(&format!(
            "Hall\nStone arches.\nExits:\nNorth - The stairs\nSouth - The door\n{prompt}"
        ));
        assert_eq!(rooms.len(), 1, "{prompt}");
        assert_eq!(rooms[0].name, "Hall");
        assert_eq!(rooms[0].exits.len(), 2);
    }
}

#[test]
fn login_and_chat_do_not_create_rooms_and_failed_movement_is_detected() {
    let mut p = TextRoomObserver::new();
    p.watch_failures = true;
    assert!(
        p.feed("Welcome!\nUsername: \n(P)assword:\nSomeone says: Exits: north\n")
            .is_empty()
    );
    assert!(is_movement_failure("You can't go that way."));
    assert!(is_movement_failure("The door is closed."));
    assert!(!is_movement_failure("You open the door."));
    p.feed("The door is closed.\n");
    assert!(p.movement_failed);
}

// ---- SqliteMapStoreTests -----------------------------------------------------------------

fn rich_room(id: &str) -> MapRoom {
    MapRoom {
        environment: Some("forest".into()),
        symbol: Some("♣".into()),
        color: Some("#123456".into()),
        notes: "home".into(),
        weight: 3.0,
        is_manually_edited: true,
        revision: 42,
        known_exits: vec!["north".into()],
        observed_name: Some("Observed".into()),
        ..MapRoom::new(id, id, "Description", Some("Keep"), 1.0, 2.0, 3.0, false).with_server_id(id)
    }
}

fn rich_map() -> MapSnapshot {
    MapSnapshot {
        rooms: vec![rich_room("a"), rich_room("b")],
        links: vec![MapLink {
            command: Some("enter portal".into()),
            weight: 4.0,
            door_state: DoorState::Closed,
            is_locked: true,
            is_manually_edited: true,
            line_points: vec![MapLinePoint { x: 2.0, y: 4.0 }],
            revision: 43,
            ..MapLink::new("a", "b", "north", true)
        }],
        candidate_room_ids: Vec::new(),
        current_room_id: Some("a".into()),
        state: TrackingState::Confirmed,
        source: RoomSource::Gmcp,
        observation_count: 17,
        area_settings: vec![MapAreaSettings {
            revision: 44,
            ..MapAreaSettings::new("Keep", true)
        }],
        room_aliases: vec![MapRoomAlias {
            source_id: "old".into(),
            target_id: "a".into(),
            revision: 45,
        }],
        deleted_rooms: vec![MapRoomDeletion {
            id: "gone".into(),
            revision: 46,
        }],
        deleted_links: vec![MapLinkDeletion {
            from_id: "a".into(),
            direction: "south".into(),
            revision: 47,
        }],
        labels: vec![
            MapLabel {
                color: Some("#FFEEDD".into()),
                background: Some("#102030".into()),
                above_rooms: true,
                is_manually_edited: true,
                revision: 48,
                ..MapLabel::text("note", Some("Keep"), 1.0, 2.0, 3.0, "The Keep\nNorth wing")
            },
            MapLabel {
                image: Some(rich_image().hash),
                opacity: 0.5,
                revision: 49,
                ..MapLabel::text("picture", None, -4.0, 5.0, 0.0, "")
            },
        ],
        deleted_labels: vec![MapLabelDeletion {
            id: "old-label".into(),
            revision: 50,
        }],
        images: vec![rich_image()],
    }
}

fn rich_image() -> MapImage {
    super::images::image(super::images::test_png(6, 3))
}

#[test]
fn normalized_rows_round_trip_all_map_metadata_without_player_position() {
    let (db, dir) = temp_db("rows");
    let store = MapStore::new(db.clone());
    let at = |host: &str, port| MapWorld::Endpoint {
        host: host.into(),
        port,
    };
    let m = rich_map();
    store.save(&at("mud.example", 4000), &m).unwrap();
    let loaded = store.load(&at("MUD.EXAMPLE", 4000)).unwrap().unwrap();
    let expected = MapSnapshot {
        current_room_id: None,
        state: TrackingState::Unknown,
        ..m
    };
    assert_eq!(serialize(&expected).unwrap(), serialize(&loaded).unwrap());
    let n: i64 = db
        .read(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM map_rooms WHERE area='Keep' AND x=1 AND y=2 AND z=3",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(n, 2);
    assert!(store.load(&at("mud.example", 4001)).unwrap().is_none());
    // A saved world's id works the same, also before any endpoint names it.
    let id = MapWorld::Id(crate::db::worlds::new_world_id());
    store.save(&id, &MapSnapshot::of(vec![room("x")], Vec::new())).unwrap();
    assert_eq!(store.load(&id).unwrap().unwrap().rooms.len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn search_rooms_matches_observed_text_of_the_stored_world_only() {
    let (db, dir) = temp_db("search");
    let store = MapStore::new(db);
    let host = |h: &str| MapWorld::Endpoint {
        host: h.into(),
        port: 4,
    };
    let rooms = vec![
        MapRoom {
            observed_name: Some("Ancient Temple".into()),
            observed_description: Some("A crumbling temple.".into()),
            ..rich_room("a")
        },
        MapRoom {
            observed_name: Some("Market Square".into()),
            observed_description: Some("A busy square.".into()),
            ..rich_room("b")
        },
        MapRoom {
            observed_name: None,
            observed_description: None,
            ..rich_room("c")
        },
    ];
    store.save(&host("host"), &MapSnapshot::of(rooms, Vec::new())).unwrap();
    store
        .save(
            &host("other"),
            &MapSnapshot::of(
                vec![MapRoom {
                    observed_name: Some("Temple".into()),
                    ..rich_room("d")
                }],
                Vec::new(),
            ),
        )
        .unwrap();
    let found = store.search_rooms(&host("host"), "temple").unwrap();
    assert_eq!(found.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["a"]);
    assert!(store.search_rooms(&host("host"), "cathedral").unwrap().is_empty());
    assert!(
        store
            .search_rooms(&host("missing.example"), "temple")
            .unwrap()
            .is_empty()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn concurrent_stale_sessions_cannot_revive_deleted_rooms_or_exits() {
    let (db, dir) = temp_db("concurrent");
    let store = MapStore::new(db.clone());
    store.save(&world(), &rich_map()).unwrap();
    let mut first = RoomMapTracker::from_snapshot(store.load(&world()).unwrap().unwrap());
    let stale = RoomMapTracker::from_snapshot(store.load(&world()).unwrap().unwrap());
    first.remove_room("b");
    store.save(&world(), &first.snapshot()).unwrap();
    MapStore::new(db).save(&world(), &stale.snapshot()).unwrap();
    let loaded = store.load(&world()).unwrap().unwrap();
    assert_eq!(loaded.rooms.len(), 1);
    assert!(loaded.links.is_empty());
    first.undo();
    store.save(&world(), &first.snapshot()).unwrap();
    assert_eq!(store.load(&world()).unwrap().unwrap().rooms.len(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn simultaneous_store_instances_serialize_merges() {
    let (db, dir) = temp_db("simultaneous");
    let store = MapStore::new(db.clone());
    store.save(&world(), &rich_map()).unwrap();
    let mut edited = RoomMapTracker::from_snapshot(store.load(&world()).unwrap().unwrap());
    let mut stale = RoomMapTracker::from_snapshot(store.load(&world()).unwrap().unwrap());
    edited.remove_room("b");
    stale.upsert_room(rich_room("branch"));
    let (a, b) = (edited.snapshot(), stale.snapshot());
    let (db1, db2) = (db.clone(), db);
    let t1 = std::thread::spawn(move || MapStore::new(db1).save(&world(), &a).unwrap());
    let t2 = std::thread::spawn(move || MapStore::new(db2).save(&world(), &b).unwrap());
    t1.join().unwrap();
    t2.join().unwrap();
    let mut ids: Vec<String> = store
        .load(&world())
        .unwrap()
        .unwrap()
        .rooms
        .into_iter()
        .map(|r| r.id)
        .collect();
    ids.sort();
    assert_eq!(ids, ["a", "branch"]);
    let _ = std::fs::remove_dir_all(dir);
}

// ---- The session: tracking commands and walking -------------------------------------------

const OPEN: WalkGate = WalkGate {
    connected: true,
    private: false,
    login: false,
    remote_echo: false,
};

/// The C# MapNavigationTests world: Room 1, north to Room 2, east to Room 3, all confirmed.
fn nav_session() -> (MapSession, super::route::MapRoute) {
    let rooms = vec![
        MapRoom::new("s:1", "Room 1", "", None, 0.0, 0.0, 0.0, false).with_server_id("1"),
        MapRoom::new("s:2", "Room 2", "", None, 0.0, 1.0, 0.0, false).with_server_id("2"),
        MapRoom::new("s:3", "Room 3", "", None, 1.0, 1.0, 0.0, false).with_server_id("3"),
    ];
    let links = vec![
        MapLink::new("s:1", "s:2", "north", true),
        MapLink::new("s:2", "s:3", "east", true),
    ];
    let mut s = MapSession::with_map(MapSnapshot::of(rooms, links.clone()));
    s.observe_room(id_room("1", "Room 1", &[]), OPEN, Instant::now());
    assert_eq!(s.tracker().state(), TrackingState::Confirmed);
    (
        s,
        super::route::MapRoute {
            steps: links,
            cost: 2.0,
        },
    )
}

/// Start a walk and send what it asks for, as the session tab does.
fn start(s: &mut MapSession, route: &super::route::MapRoute, now: Instant) -> Option<String> {
    let command = s.start_walk(route, OPEN, now);
    if let Some(c) = &command {
        s.track_command(c, false, now);
    }
    command
}

/// Advance a walk and send what it asks for.
fn advance(s: &mut MapSession, now: Instant) -> Option<String> {
    let command = s.advance_walk(OPEN, now);
    if let Some(c) = &command {
        s.track_command(c, false, now);
    }
    command
}

#[test]
fn a_walk_sends_one_step_and_waits_for_the_expected_room() {
    let (mut s, route) = nav_session();
    let now = Instant::now();
    let first = start(&mut s, &route, now);
    assert_eq!(first.as_deref(), Some("north"));
    assert_eq!(s.walk_status(), WalkStatus::Progress { step: 1, of: 2 });
    assert_eq!(s.advance_walk(OPEN, now), None, "nothing more until the room arrives");
    s.observe_room(id_room("2", "Room 2", &[]), OPEN, now);
    let next = advance(&mut s, now);
    assert_eq!(next.as_deref(), Some("east"));
    s.observe_room(id_room("3", "Room 3", &[]), OPEN, now);
    assert_eq!(s.advance_walk(OPEN, now), None);
    assert!(!s.is_walking());
    assert_eq!(s.walk_status(), WalkStatus::Complete);
    assert_eq!(s.tracker().current_id(), Some("s:3"));
}

#[test]
fn a_wrong_room_or_two_arrivals_in_one_burst_stop_the_walk() {
    let (mut s, route) = nav_session();
    let now = Instant::now();
    start(&mut s, &route, now);
    s.observe_room(id_room("2", "Room 2", &[]), OPEN, now);
    s.observe_room(id_room("1", "Room 1", &[]), OPEN, now);
    assert_eq!(s.advance_walk(OPEN, now), None);
    assert!(!s.is_walking());
    assert_eq!(s.walk_status(), WalkStatus::WrongRoom);
}

#[test]
fn privacy_a_blocked_move_a_changed_map_a_timeout_or_a_disconnect_stop_the_walk() {
    let now = Instant::now();
    let cases: [(&str, WalkStatus); 6] = [
        ("private", WalkStatus::Private),
        ("login", WalkStatus::Private),
        ("blocked", WalkStatus::Blocked),
        ("edit", WalkStatus::GraphChanged),
        ("timeout", WalkStatus::Timeout),
        ("disconnect", WalkStatus::Disconnected),
    ];
    for (case, expected) in cases {
        let (mut s, route) = nav_session();
        start(&mut s, &route, now);
        let mut gate = OPEN;
        let mut later = now;
        match case {
            "private" => gate.private = true,
            "login" => gate.login = true,
            "blocked" => s.track_output("The door is closed.\n> ", OPEN, now),
            "edit" => assert!(s.tracker_mut().remove_link("s:2", "east")),
            "timeout" => later = now + STEP_TIMEOUT,
            _ => gate.connected = false,
        }
        assert_eq!(s.advance_walk(gate, later), None, "{case}");
        assert!(!s.is_walking(), "{case}");
        assert_eq!(s.walk_status(), expected, "{case}");
    }
}

#[test]
fn a_walk_needs_a_confirmed_position_no_pending_move_and_direction_commands() {
    let now = Instant::now();
    let (mut s, route) = nav_session();
    s.track_command("north", false, now);
    assert_eq!(s.start_walk(&route, OPEN, now), None);
    assert_eq!(s.walk_status(), WalkStatus::Unavailable);
    let (mut s, route) = nav_session();
    assert_eq!(s.start_walk(&route, WalkGate { login: true, ..OPEN }, now), None);
    assert_eq!(s.walk_status(), WalkStatus::Unavailable);
    let (mut s, mut route) = nav_session();
    route.steps[1].command = Some("drop all".into());
    assert!(s.tracker_mut().upsert_link(route.steps[1].clone()));
    let route = find_route(&s.tracker().snapshot(), "s:1", "s:3", false).unwrap();
    assert_eq!(s.start_walk(&route, OPEN, now), None);
    assert_eq!(s.walk_status(), WalkStatus::CustomCommand);
    // A route planned before an edit no longer matches the map.
    let (mut s, stale) = nav_session();
    let edited = MapLink {
        weight: 3.0,
        ..stale.steps[1].clone()
    };
    assert!(s.tracker_mut().upsert_link(edited));
    assert_eq!(s.start_walk(&stale, OPEN, now), None);
    assert_eq!(s.walk_status(), WalkStatus::InvalidRoute);
}

#[test]
fn rejected_room_data_cannot_complete_the_last_step() {
    let (mut s, route) = nav_session();
    let now = Instant::now();
    let one = super::route::MapRoute {
        steps: vec![route.steps[0].clone()],
        cost: 1.0,
    };
    start(&mut s, &one, now);
    s.observe_room(id_room("2", &"x".repeat(513), &[]), OPEN, now);
    assert_eq!(s.advance_walk(OPEN, now + Duration::from_secs(11)), None);
    assert!(!s.is_walking());
    assert_eq!(s.tracker().current_id(), Some("s:1"));
    assert_ne!(s.walk_status(), WalkStatus::Complete);
}

#[test]
fn a_repeated_origin_neither_moves_nor_uses_up_the_direction() {
    let (mut s, _) = nav_session();
    let now = Instant::now();
    s.track_command("north", false, now);
    s.observe_room(id_room("1", "Room 1", &[]), OPEN, now);
    s.observe_room(id_room("2", "Room 2", &[]), OPEN, now);
    let link = s.tracker().link("s:1", "north").unwrap();
    assert!(link.confirmed);
    assert_eq!(s.tracker().current_id(), Some("s:2"));
}

#[test]
fn a_later_command_clears_an_older_direction_and_two_moves_lose_the_position() {
    let now = Instant::now();
    let mut s = MapSession::new();
    s.observe_room(id_room("1", "Hall", &[]), OPEN, now);
    s.track_command("north", false, now);
    s.track_command("enter portal", false, now);
    s.observe_room(id_room("9", "Portal room", &[]), OPEN, now);
    assert_eq!(s.tracker().link_count(), 0, "no exit is inferred from the portal");
    s.track_command("north", false, now);
    s.track_command("south", false, now);
    assert_eq!(s.tracker().current_id(), None);
    // A private command is never a move.
    s.observe_room(id_room("1", "Hall", &[]), OPEN, now);
    s.track_command("north", true, now);
    s.observe_room(id_room("2", "Other", &[]), OPEN, now);
    assert!(s.tracker().link("s:1", "north").is_none());
}

#[test]
fn rooms_wait_for_the_stored_map_and_saves_wait_for_changes() {
    let now = Instant::now();
    let mut s = MapSession::new();
    s.begin_load();
    s.observe_room(id_room("2", "Tower", &[]), OPEN, now);
    assert_eq!(s.tracker().room_count(), 0);
    assert!(
        s.take_save(now, true).is_none(),
        "nothing is saved before the map arrives"
    );
    s.finish_load(Some(map(vec![room("s:1")])), OPEN, now);
    assert_eq!(s.tracker().room_count(), 2);
    let saved = s.take_save(now, false).unwrap();
    assert_eq!(saved.rooms.len(), 2);
    assert!(s.take_save(now, true).is_none(), "nothing changed since");
    s.observe_room(id_room("3", "Gate", &[]), OPEN, now);
    assert!(
        s.take_save(now + Duration::from_millis(500), false).is_none(),
        "two seconds apart"
    );
    assert!(
        s.take_save(now + Duration::from_millis(500), true).is_some(),
        "unless forced"
    );
}

#[test]
fn evidence_distinguishes_negotiation_from_received_fields() {
    let now = Instant::now();
    let mut s = MapSession::new();
    s.set_gmcp(OptionState::Enabled);
    s.observe_room(id_room("1", "Room 1", &[]), OPEN, now);
    let e = &s.evidence;
    assert_eq!((e.gmcp, e.msdp), (OptionState::Enabled, OptionState::Unknown));
    assert!(e.received_room_id && !e.received_exits && !e.received_terrain && !e.received_coordinates);
    assert_eq!(e.summary(), "GMCP: Supported · MSDP: Not negotiated");
    let r = from_gmcp(&gmcp(
        r#"Room.Info {"num":1,"name":"Room 1","exits":{},"environment":"forest","coords":{"x":2,"y":3,"z":0}}"#,
    ))
    .unwrap();
    s.observe_room(r, OPEN, now);
    let e = &s.evidence;
    assert!(e.received_exits && e.received_terrain && e.received_coordinates);
    assert_eq!(e.last_room_source, Some(RoomSource::Gmcp));
    assert!(e.room_fields().contains("terrain"), "{}", e.room_fields());
    s.set_msdp(OptionState::Disabled);
    assert_eq!(s.evidence.summary(), "GMCP: Supported · MSDP: Declined");
}

#[test]
fn text_rooms_map_a_world_without_room_data_until_structured_rooms_arrive() {
    let now = Instant::now();
    let mut s = MapSession::new();
    s.track_output("\x1b[1mThe Hall\x1b[0m\r\nStone arches.\r\nExits: north\r\n", OPEN, now);
    assert_eq!(s.tracker().room_count(), 1);
    s.track_command("north", false, now);
    s.track_output("Tower Stairs\r\nA spiral of worn steps.\r\nExits: south\r\n", OPEN, now);
    assert_eq!(s.tracker().room_count(), 2);
    assert!(
        s.tracker()
            .link(s.tracker().rooms().next().unwrap().id.as_str(), "north")
            .is_some()
    );
    // Structured data replaces a guess made from the same answer.
    s.track_command("look", false, now);
    s.track_output("Login banner\r\nWelcome traveller.\r\nExits: north\r\n", OPEN, now);
    let before = s.tracker().room_count();
    s.observe_room(
        RoomObservation::new(None, "Tower Stairs", "", &[("south", None)]).with_source(RoomSource::Gmcp),
        OPEN,
        now,
    );
    assert_eq!(s.tracker().room_count(), before - 1, "the banner guess is withdrawn");
    s.track_output("Elsewhere\r\nA room.\r\nExits: east\r\n", OPEN, now);
    assert_eq!(s.tracker().room_count(), before - 1, "text is no longer read");
}

// ---- t13: map files, limits and undo across many edits -----------------------------------------

#[test]
fn map_file_limits_are_enforced_on_import_and_export() {
    use super::format::{MAX_BYTES, MAX_LINKS, MAX_ROOMS};
    let many = |count: usize| -> Vec<MapRoom> {
        (0..count)
            .map(|i| MapRoom::new(&format!("r{i}"), "R", "", None, i as f64, 0.0, 0.0, false))
            .collect()
    };
    // 10,000 rooms is the limit; one more is refused both ways.
    assert!(serialize(&map(many(MAX_ROOMS))).is_ok());
    let over = map(many(MAX_ROOMS + 1));
    assert!(serialize(&over).is_err());
    assert!(deserialize(&serde_json::to_string(&over).unwrap()).is_err());
    // 60,000 exits is the limit.
    let links = |count: usize| -> Vec<MapLink> {
        (0..count)
            .map(|i| MapLink::new(&format!("r{}", i % 100), "r0", &format!("d{i}"), true))
            .collect()
    };
    let at_limit = MapSnapshot {
        links: links(MAX_LINKS),
        ..map(many(100))
    };
    assert!(super::format::validate(&at_limit).is_ok());
    let over = MapSnapshot {
        links: links(MAX_LINKS + 1),
        ..map(many(100))
    };
    assert!(super::format::validate(&over).is_err());
    assert!(deserialize(&serde_json::to_string(&over).unwrap()).is_err());
    // The file limit holds before any parsing.
    let huge = format!(
        "{{\"Version\":1,\"Map\":{{\"Rooms\":[],\"Pad\":\"{}\"}}}}",
        "x".repeat(MAX_BYTES)
    );
    assert_eq!(
        deserialize(&huge).unwrap_err().to_string(),
        "Map file exceeds the size limit."
    );
    // A map whose text would pass every field check but not the 16 MiB limit (pictures aside)
    // cannot be exported.
    let big: Vec<MapRoom> = (0..2_000)
        .map(|i| MapRoom {
            notes: "n".repeat(9_000),
            ..MapRoom::new(&format!("r{i}"), "R", "", None, 0.0, 0.0, 0.0, false)
        })
        .collect();
    assert!(serialize(&map(big)).is_err());
}

#[test]
fn the_tracker_refuses_rooms_and_exits_past_the_limits() {
    use super::format::{MAX_LINKS, MAX_ROOMS};
    let rooms: Vec<MapRoom> = (0..MAX_ROOMS)
        .map(|i| MapRoom::new(&format!("r{i}"), "R", "", None, 0.0, 0.0, 0.0, false))
        .collect();
    let links: Vec<MapLink> = (0..MAX_LINKS)
        .map(|i| MapLink::new(&format!("r{}", i % 1_000), "r0", &format!("d{i}"), true))
        .collect();
    let mut t = RoomMapTracker::from_snapshot(MapSnapshot { links, ..map(rooms) });
    assert!(!t.upsert_room(room("new")), "a 10,001st room");
    let mut existing = t.room("r5").unwrap().clone();
    existing.name = "Renamed".into();
    assert!(t.upsert_room(existing), "an existing room can still change");
    assert!(
        !t.upsert_link(MapLink::new("r1", "r2", "extra", true)),
        "a 60,001st exit"
    );
    assert!(!t.edit_link(MapLink::new("r1", "r2", "extra", true), None, None));
    let mut changed = t.link("r0", "d0").unwrap().clone();
    changed.weight = 4.0;
    assert!(t.upsert_link(changed), "an existing exit can still change");
}

#[test]
fn an_invalid_import_is_refused_before_anything_is_replaced() {
    let mut t = RoomMapTracker::from_snapshot(abc());
    let before = t.snapshot();
    let mut bad = map(vec![room("x")]);
    bad.links = vec![MapLink::new("missing", "x", "north", true)];
    assert!(!t.replace_map(&bad));
    assert_eq!(t.snapshot(), before);
    assert!(!t.can_undo());
}

#[test]
fn undo_and_redo_cover_fifty_edits_including_an_import() {
    let mut t = RoomMapTracker::from_snapshot(abc());
    let original: Vec<(String, String)> = t.rooms().map(|r| (r.id.clone(), r.name.clone())).collect();
    // 25 renames, an import, 24 more edits on the imported map: 50 edits.
    for i in 0..25 {
        let mut r = t.room("a").unwrap().clone();
        r.name = format!("Hall {i}");
        assert!(t.upsert_room(r));
    }
    let imported = deserialize(
        &serialize(&MapSnapshot {
            links: vec![MapLink::new("x", "y", "east", true)],
            ..map(vec![room("x"), room("y")])
        })
        .unwrap(),
    )
    .unwrap();
    assert!(t.replace_map(&imported));
    for i in 0..24 {
        let mut r = t.room("x").unwrap().clone();
        r.x = f64::from(i + 1);
        assert!(t.upsert_room(r));
    }
    let names =
        |t: &RoomMapTracker| -> Vec<(String, String)> { t.rooms().map(|r| (r.id.clone(), r.name.clone())).collect() };
    let after_all = t.snapshot();
    // Undo back across the import to the start: exactly 50 steps.
    for step in 0..50 {
        assert!(t.undo(), "undo {step}");
        if step == 23 {
            // The import is next: the imported map is in place, untouched by the later edits.
            assert_eq!(t.room("x").unwrap().x, 0.0);
        }
    }
    assert!(!t.undo(), "only 50 edits are kept");
    let mut restored = names(&t);
    restored.sort();
    let mut expected = original.clone();
    expected.sort();
    assert_eq!(restored, expected);
    assert_eq!(t.link_count(), 2);
    assert_eq!(t.room("a").unwrap().name, "a");
    // Redo everything: the import and the edits after it come back.
    for step in 0..50 {
        assert!(t.redo(), "redo {step}");
    }
    assert!(!t.redo());
    assert_eq!(t.room("x").unwrap().x, 24.0);
    assert!(t.room("a").is_none());
    let rooms = |s: &MapSnapshot| -> Vec<(String, f64, String)> {
        s.rooms.iter().map(|r| (r.id.clone(), r.x, r.name.clone())).collect()
    };
    assert_eq!(rooms(&t.snapshot()), rooms(&after_all));
    // A 51st edit pushes the oldest out.
    let mut r = t.room("y").unwrap().clone();
    r.notes = "one more".into();
    assert!(t.upsert_room(r));
    let mut undone = 0;
    while t.undo() {
        undone += 1;
    }
    assert_eq!(undone, 50);
    assert_eq!(
        t.room("a").unwrap().name,
        "Hall 0",
        "the first rename can no longer be undone"
    );
}
