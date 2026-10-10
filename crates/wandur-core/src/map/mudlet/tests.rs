//! Mudlet's JSON map export, read from a hand-written fixture (The Lantern Road): every mapped
//! feature, the way up, special exits, labels with text and a picture, the limits, the summary,
//! idempotent re-import and its undo, and the `.dat` note.

use serde_json::Value;

use super::*;
use crate::l10n::{Language, override_thread};
use crate::map::RoomMapTracker;

fn fixture() -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/mudlet/lantern-road-map.json"
    );
    std::fs::read_to_string(path).unwrap()
}

fn english() {
    override_thread(Some(Language::En));
}

fn prepared(text: &str) -> Prepared {
    read(Some("json"), text.as_bytes(), &|_| {}).unwrap()
}

fn link<'a>(map: &'a MapSnapshot, from: &str, direction: &str) -> &'a MapLink {
    map.links
        .iter()
        .find(|l| l.from_id == from && l.direction == direction)
        .unwrap_or_else(|| panic!("no exit {from} {direction}"))
}

#[test]
fn every_mapped_feature_comes_across() {
    english();
    let p = prepared(&fixture());
    assert!(!p.replaces());
    let map = &p.map;
    let s = &p.summary;
    assert_eq!((s.areas, s.rooms, s.exits), (3, 8, 12));
    assert_eq!((s.special_exits, s.doors, s.locked_exits), (1, 3, 1));
    assert_eq!((s.notes, s.symbols, s.text_labels, s.picture_labels), (1, 2, 1, 1));
    assert!(!s.flipped);
    // Ids: the room hash becomes the server's id, the others keep Mudlet's.
    let end = map.room("s:1001").unwrap();
    assert_eq!(end.server_id.as_deref(), Some("1001"));
    assert_eq!(end.name, "Road's End");
    assert_eq!(end.area.as_deref(), Some("Lantern Road"));
    assert_eq!((end.x, end.y, end.z), (0.0, 0.0, 0.0));
    assert_eq!(
        end.color.as_deref(),
        Some("#008000"),
        "environment 2, the default green"
    );
    assert_eq!(end.symbol.as_deref(), Some("R"));
    assert_eq!(end.description, "A lantern sways over the end of the road.");
    assert_eq!(end.notes, "owner: Odo");
    assert!(end.known_exits.contains(&"west".to_string()), "the stub");
    let square = map.room("mudlet:2").unwrap();
    assert!(square.is_locked);
    assert_eq!(square.color.as_deref(), Some("#C87828"), "a custom environment colour");
    let smithy = map.room("mudlet:3").unwrap();
    assert_eq!(smithy.weight, 5.0);
    assert_eq!(smithy.symbol.as_deref(), Some("⚒"));
    assert_eq!(smithy.color.as_deref(), Some("#404040"));
    assert_eq!(map.room("mudlet:4").unwrap().z, -1.0);
    assert_eq!(map.room("mudlet:10").unwrap().name, "Room 10");
    assert_eq!(map.room("mudlet:10").unwrap().area.as_deref(), Some("Odo's Garden"));
    assert_eq!(map.room("mudlet:20").unwrap().area, None, "Mudlet's default area");
    // Exits: compass, up, down, in, out; doors, weights, locks.
    assert_eq!(link(map, "s:1001", "north").to_id, "mudlet:2");
    let east = link(map, "s:1001", "east");
    assert_eq!((east.door_state, east.weight), (DoorState::Closed, 3.0));
    assert_eq!(link(map, "s:1001", "up").to_id, "mudlet:5");
    assert_eq!(link(map, "mudlet:5", "down").to_id, "s:1001");
    assert_eq!(link(map, "mudlet:2", "in").to_id, "mudlet:4");
    assert_eq!(link(map, "mudlet:4", "out").to_id, "mudlet:2");
    assert!(link(map, "mudlet:3", "west").is_locked);
    assert_eq!(link(map, "mudlet:11", "west").door_state, DoorState::Locked);
    assert!(map.links.iter().all(|l| l.confirmed));
    assert!(
        map.area_settings
            .iter()
            .any(|a| a.area == "Odo's Garden" && a.grid_mode)
    );
    // What was left out, and why.
    for (skip, n) in [
        (Skip::ScriptExit, 1),
        (Skip::MissingRoom, 1),
        (Skip::CustomLine, 1),
        (Skip::SymbolColor, 1),
        (Skip::FixedSizeLabel, 1),
        (Skip::EmptyLabel, 1),
        (Skip::AreaUserData, 1),
        (Skip::MapUserData, 1),
    ] {
        assert_eq!(s.skipped(skip), n, "{skip:?}");
    }
    assert_eq!(s.skipped(Skip::DuplicateExit), 0);
    format::validate(map).unwrap();
}

#[test]
fn special_exits_keep_their_command_and_scripts_are_left_out() {
    let p = prepared(&fixture());
    let climb = link(&p.map, "mudlet:2", "climb the lantern post");
    assert_eq!(climb.command.as_deref(), Some("climb the lantern post"));
    assert_eq!(climb.to_id, "mudlet:5");
    assert_eq!(climb.door_state, DoorState::Open);
    assert!(!p.map.links.iter().any(|l| l.direction.starts_with("script:")));
    // Only real directions count as known exits.
    let square = p.map.room("mudlet:2").unwrap();
    assert_eq!(square.known_exits, vec!["in".to_string(), "south".to_string()]);
}

#[test]
fn labels_keep_their_text_colours_place_and_picture() {
    let p = prepared(&fixture());
    let text = p.map.labels.iter().find(|l| l.id == "mudlet:label:1:0").unwrap();
    assert_eq!(text.text, "Lantern Road");
    assert_eq!(text.area.as_deref(), Some("Lantern Road"));
    assert_eq!((text.x, text.y, text.width, text.height), (-1.0, 2.5, 3.0, 0.6));
    assert_eq!(text.color.as_deref(), Some("#FFDC78"));
    assert_eq!(text.background.as_deref(), Some("#141414"));
    assert!(text.above_rooms);
    assert!(text.image.is_none());
    assert!(text.font_size > 20.0 && text.font_size < 40.0, "{}", text.font_size);
    let picture = p.map.labels.iter().find(|l| l.id == "mudlet:label:1:1").unwrap();
    assert!(!picture.above_rooms);
    let hash = picture.image.as_deref().unwrap();
    let image = p.map.images.iter().find(|i| i.hash == hash).unwrap();
    assert_eq!(images::dimensions(&image.data), Some((32, 32)));
    assert!(images::valid_image(image));
}

/// The fixture with every y coordinate (rooms and labels) negated: north at -Y.
fn upside_down_fixture() -> String {
    let mut value: Value = serde_json::from_str(&fixture()).unwrap();
    for area in value["areas"].as_array_mut().unwrap() {
        for key in ["rooms", "labels"] {
            for item in area[key].as_array_mut().into_iter().flatten() {
                if let Some(y) = item["coordinates"][1].as_f64() {
                    item["coordinates"][1] = serde_json::json!(-y);
                }
            }
        }
    }
    serde_json::to_string(&value).unwrap()
}

#[test]
fn an_upside_down_file_is_turned_so_north_is_up() {
    let normal = prepared(&fixture());
    let flipped = prepared(&upside_down_fixture());
    assert!(flipped.summary.flipped);
    let place = |p: &Prepared, id: &str| {
        let r = p.map.room(id).unwrap();
        (r.x, r.y, r.z)
    };
    for id in ["s:1001", "mudlet:2", "mudlet:3", "mudlet:4", "mudlet:10"] {
        assert_eq!(place(&normal, id), place(&flipped, id), "{id}");
    }
    assert_eq!(
        place(&flipped, "mudlet:2").1,
        1.0,
        "the square is north of the road's end"
    );
    for (a, b) in normal.map.labels.iter().zip(&flipped.map.labels) {
        assert_eq!((a.x, a.y), (b.x, b.y), "{}", a.id);
    }
}

#[test]
fn importing_again_is_idempotent_and_one_undo_takes_it_back() {
    let p = prepared(&fixture());
    let again = prepared(&fixture());
    assert_eq!(p.map, again.map, "the same file gives the same ids and the same map");
    let mut t = RoomMapTracker::new();
    let changed = t.import_map(&p.map).unwrap();
    assert!(changed > 20);
    assert_eq!((t.room_count(), t.link_count(), t.label_count()), (8, 12, 2));
    let step = t.last_edit();
    assert_eq!(t.import_map(&again.map).unwrap(), 0);
    assert_eq!(t.last_edit(), step);
    assert!(t.undo());
    assert_eq!((t.room_count(), t.link_count(), t.label_count()), (0, 0, 0));
    assert!(t.redo());
    assert_eq!(t.room_count(), 8);
}

#[test]
fn the_summary_lists_counts_and_what_was_left_out() {
    english();
    let p = prepared(&fixture());
    let lines = p.summary.lines();
    assert_eq!(lines[..3], ["Areas: 3", "Rooms: 8", "Exits: 12"]);
    assert!(
        lines.contains(&"Special exits (with a command): 1".to_string()),
        "{lines:?}"
    );
    assert!(lines.contains(&"Picture labels: 1".to_string()));
    let skipped = p.summary.skipped_lines();
    assert_eq!(skipped.len(), 8);
    assert_eq!(skipped[0], "Special exits that run Lua scripts, left out: 1");
    override_thread(Some(Language::De));
    assert_eq!(p.summary.lines()[1], "Räume: 8");
    english();
}

fn rooms_file(count: usize) -> String {
    let rooms: Vec<Value> = (0..count)
        .map(|i| serde_json::json!({"id": i + 1, "name": "R", "coordinates": [i % 100, i / 100, 0]}))
        .collect();
    serde_json::json!({"areas": [{"id": 1, "name": "Big", "rooms": rooms}]}).to_string()
}

#[test]
fn files_past_the_room_limit_are_refused_with_the_numbers() {
    english();
    assert_eq!(prepared(&rooms_file(MAX_ROOMS)).summary.rooms, MAX_ROOMS);
    let error = read(Some("json"), rooms_file(MAX_ROOMS + 1).as_bytes(), &|_| {}).unwrap_err();
    assert_eq!(
        error,
        ImportError::TooBig {
            rooms: MAX_ROOMS + 1,
            exits: 0
        }
    );
    let text = error.to_string();
    assert!(
        text.contains("10001") && text.contains("10000") && text.contains("Nothing was imported"),
        "{text}"
    );
}

#[test]
fn progress_is_reported_while_rooms_are_converted() {
    let seen = std::cell::RefCell::new(Vec::new());
    read(Some("json"), rooms_file(1200).as_bytes(), &|p| {
        seen.borrow_mut().push(p)
    })
    .unwrap();
    let seen = seen.into_inner();
    assert_eq!(seen[0].stage, Stage::Reading);
    assert!(
        seen.iter()
            .any(|p| p.stage == Stage::Rooms && p.done == 1000 && p.total == 1200)
    );
}

#[test]
fn a_mudlet_binary_map_gets_the_json_note() {
    english();
    assert!(is_mudlet_binary(Some("DAT"), b"{}"));
    let binary = [0u8, 0, 0, 20, 0, 0, 0, 3, 1, 2];
    assert!(is_mudlet_binary(None, &binary));
    assert!(!is_mudlet_binary(Some("json"), b"  {\"areas\": []}"));
    let error = read(None, &binary, &|_| {}).unwrap_err();
    assert_eq!(error, ImportError::MudletBinary);
    let text = error.to_string();
    assert!(text.contains("saveJsonMap") && text.contains(".dat"), "{text}");
    override_thread(Some(Language::Fr));
    assert!(error.to_string().contains("saveJsonMap") && error.to_string().contains("JSON"));
    english();
}

#[test]
fn our_own_file_replaces_and_anything_else_is_unknown() {
    english();
    let mut t = RoomMapTracker::new();
    t.import_map(&prepared(&fixture()).map).unwrap();
    let ours = format::serialize(&t.snapshot()).unwrap();
    let p = prepared(&ours);
    assert!(p.replaces());
    assert_eq!(
        (
            p.summary.rooms,
            p.summary.exits,
            p.summary.text_labels,
            p.summary.picture_labels
        ),
        (8, 12, 1, 1)
    );
    assert_eq!(
        read(None, b"{\"hello\": 1}", &|_| {}).unwrap_err(),
        ImportError::Unknown
    );
    assert_eq!(
        read(None, b"not json at all", &|_| {}).unwrap_err(),
        ImportError::Unknown
    );
    assert!(matches!(
        read(None, b"{\"Version\": 1, \"Map\": {\"Rooms\": 3}}", &|_| {}),
        Err(ImportError::Invalid(_))
    ));
}
