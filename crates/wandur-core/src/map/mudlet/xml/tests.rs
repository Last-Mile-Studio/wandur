//! The MMP XML reader against a hand-written fixture (Ember Vale): rooms, areas, environment
//! colours, special exits and doors; malformed files, DTDs and entities refused; the limits.

use super::*;
use crate::l10n::{Language, override_thread};
use crate::map::RoomMapTracker;
use crate::map::model::{DoorState, MapSnapshot};

fn fixture() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/mudlet/ember-vale-map.xml"
    ))
    .unwrap()
}

fn english() {
    override_thread(Some(Language::En));
}

fn error(xml: &str) -> ImportError {
    english();
    super::super::read(Some("xml"), xml.as_bytes(), &|_| {}).unwrap_err()
}

fn has_link(map: &MapSnapshot, from: &str, direction: &str, to: &str) -> bool {
    map.links
        .iter()
        .any(|l| l.from_id == from && l.direction == direction && l.to_id == to)
}

#[test]
fn the_fixture_reads_into_rooms_exits_and_colours() {
    english();
    let p = super::super::read(Some("xml"), &fixture(), &|_| {}).unwrap();
    assert_eq!(p.summary.kind, SourceKind::MudletXml);
    assert!(!p.replaces(), "an XML map is merged");
    let s = &p.summary;
    assert_eq!((s.areas, s.rooms, s.exits), (3, 7, 10));
    assert_eq!((s.special_exits, s.doors), (2, 1));
    assert_eq!(s.skipped(super::super::Skip::MissingRoom), 1, "the exit to room 999");
    assert!(!s.flipped);
    let map = &p.map;
    // The game's room numbers are the rooms' server ids, as GMCP Room.Info gives them.
    let gate = map.room("s:500").unwrap();
    assert_eq!(gate.server_id.as_deref(), Some("500"));
    assert_eq!(gate.name, "Vale Gate");
    assert_eq!(gate.area.as_deref(), Some("Ember Vale"));
    assert_eq!(gate.color.as_deref(), Some("#808000"), "environment 3 is ANSI yellow");
    assert_eq!((gate.x, gate.y, gate.z), (0.0, 0.0, 0.0));
    assert_eq!(map.room("s:502").unwrap().color.as_deref(), Some("#C0C0C0"));
    assert_eq!(map.room("s:510").unwrap().color.as_deref(), Some("#008000"));
    assert_eq!(
        map.room("s:511").unwrap().color.as_deref(),
        Some("#336655"),
        "htmlcolor wins"
    );
    assert_eq!(map.room("s:510").unwrap().area.as_deref(), Some("Ashwood & Hollow"));
    let lost = map.room("s:520").unwrap();
    assert_eq!(
        (lost.area.as_deref(), lost.color.as_deref()),
        (None, None),
        "an area the file does not name"
    );
    assert_eq!(map.room("s:503").unwrap().z, 1.0);
    assert!(has_link(map, "s:500", "north", "s:501"));
    assert!(
        has_link(map, "s:510", "southeast", "s:511"),
        "short directions are normalized"
    );
    let door = map
        .links
        .iter()
        .find(|l| l.from_id == "s:500" && l.direction == "east")
        .unwrap();
    assert_eq!(door.door_state, DoorState::Closed);
    let rope = map
        .links
        .iter()
        .find(|l| l.from_id == "s:501" && l.to_id == "s:510")
        .unwrap();
    assert_eq!(rope.direction, "climb rope");
    assert_eq!(rope.command.as_deref(), Some("climb rope"));
}

#[test]
fn xml_is_recognized_without_its_extension_and_imports_once() {
    english();
    let p = super::super::read(None, &fixture(), &|_| {}).unwrap();
    assert_eq!(p.summary.kind, SourceKind::MudletXml);
    let mut tracker = RoomMapTracker::new();
    assert!(tracker.import_map(&p.map).unwrap() > 0);
    assert_eq!(
        tracker.import_map(&p.map).unwrap(),
        0,
        "importing the same file again changes nothing"
    );
}

#[test]
fn malformed_files_are_refused() {
    assert!(matches!(
        error("<map><rooms><room id=\"1\"></rooms></map>"),
        ImportError::Invalid(_)
    ));
    assert!(matches!(error("<map><rooms>"), ImportError::Invalid(_)));
    assert!(matches!(
        error("<package><room id=\"1\"/></package>"),
        ImportError::Unknown
    ));
    assert!(matches!(error("<map/><map/>"), ImportError::Invalid(_)));
    assert!(matches!(
        error("<map><room id=\"1\" title=\"a</map>"),
        ImportError::Invalid(_)
    ));
    let ImportError::Invalid(text) = error("<map><room id=\"1\" title=\"&nope;\"/></map>") else {
        panic!("an undefined entity");
    };
    assert!(text.contains("not a map Wandur can read"), "{text}");
}

#[test]
fn a_dtd_or_an_entity_is_refused() {
    let laughs = r#"<?xml version="1.0"?>
<!DOCTYPE map [<!ENTITY lol "lol"><!ENTITY lol2 "&lol;&lol;&lol;&lol;">]>
<map><rooms><room id="1" title="&lol2;"><coord x="0" y="0" z="0"/></room></rooms></map>"#;
    let ImportError::Invalid(text) = error(laughs) else {
        panic!("a DTD");
    };
    assert!(text.contains("DTD"), "{text}");
    let ImportError::Invalid(text) = error("<map>&lol;</map>") else {
        panic!("an entity in text");
    };
    assert!(text.contains("DTD"), "{text}");
    // Built-in entities and character references are fine.
    english();
    let ok = r#"<map><room id="1" title="A &lt;b&gt; &#65;"><coord x="0" y="0" z="0"/></room></map>"#;
    let p = super::super::read(Some("xml"), ok.as_bytes(), &|_| {}).unwrap();
    assert_eq!(p.map.rooms[0].name, "A <b> A");
}

#[test]
fn limits_are_checked_while_reading() {
    // Rooms past the map's limit: refused with the counts, nothing kept.
    let mut many = String::from("<map><rooms>");
    for i in 0..=MAX_ROOMS {
        many.push_str(&format!(
            r#"<room id="{i}"><coord x="{i}" y="0" z="0"/><exit direction="east" target="{}"/></room>"#,
            i + 1
        ));
    }
    many.push_str("</rooms></map>");
    assert_eq!(
        error(&many),
        ImportError::TooBig {
            rooms: MAX_ROOMS + 1,
            exits: MAX_ROOMS + 1
        }
    );
    // Areas.
    let mut areas = String::from("<map><areas>");
    for i in 0..=MAX_AREAS {
        areas.push_str(&format!(r#"<area id="{i}" name="A{i}"/>"#));
    }
    areas.push_str("</areas></map>");
    let ImportError::Invalid(text) = error(&areas) else {
        panic!("too many areas");
    };
    assert!(text.contains("areas") && text.contains("10000"), "{text}");
    // Exits of one room.
    let mut exits = String::from(r#"<map><room id="1"><coord x="0" y="0" z="0"/>"#);
    for i in 0..=MAX_ROOM_EXITS {
        exits.push_str(&format!(r#"<exit direction="go {i}" target="1"/>"#));
    }
    exits.push_str("</room></map>");
    let ImportError::Invalid(text) = error(&exits) else {
        panic!("too many exits");
    };
    assert!(text.contains("exits of one room"), "{text}");
    // Nesting.
    let deep = format!("<map>{}{}</map>", "<x>".repeat(40), "</x>".repeat(40));
    assert!(matches!(error(&deep), ImportError::Invalid(_)));
    // Size.
    let mut big = b"<map>".to_vec();
    big.resize(format::MAX_BYTES + 1, b' ');
    english();
    assert_eq!(
        super::super::read(Some("xml"), &big, &|_| {}).unwrap_err(),
        ImportError::Invalid(t(S::MapImportFileTooLarge).into())
    );
}

#[test]
fn ansi_colours_cover_the_basic_and_256_colour_palettes() {
    assert_eq!(ansi_color(0).as_deref(), Some("#000000"));
    assert_eq!(ansi_color(1).as_deref(), Some("#800000"));
    assert_eq!(ansi_color(15).as_deref(), Some("#FFFFFF"));
    assert_eq!(ansi_color(196).as_deref(), Some("#FF0000"));
    assert_eq!(ansi_color(232).as_deref(), Some("#080808"));
    assert_eq!(ansi_color(256), None);
    assert!(looks_like_xml(b"\xef\xbb\xbf  <map/>"));
    assert!(!looks_like_xml(b"{\"areas\":[]}"));
}

#[test]
fn the_bench_worlds_official_map_reads() {
    english();
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/mudlet/lantern-town-map.xml"
    ))
    .unwrap();
    let p = super::super::read(None, &bytes, &|_| {}).unwrap();
    assert_eq!((p.summary.rooms, p.summary.exits, p.summary.skipped.len()), (9, 20, 0));
    assert!(has_link(&p.map, "s:102", "up", "s:108"));
}
