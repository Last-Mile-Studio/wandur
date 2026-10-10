//! Descriptions read from the text for GMCP and MSDP rooms that carry none (Legends of the
//! Jedi): the room block before or after the protocol room, matched by title, kept stable.

use std::time::{Duration, Instant};

use super::*;
use crate::map::search::search;
use crate::map::text::{TextBlock, TextRoomObserver, title_matches};
use crate::protocol::parse_gmcp;

const BACTA: &str = "Rows of tall glass tanks line the walls, each filled with a thick blue fluid. \
                     A medical droid hovers between them, checking readouts.";

fn gate() -> WalkGate {
    WalkGate {
        connected: true,
        private: false,
        login: false,
        remote_echo: false,
    }
}

/// LotJ's `Room.Info`: a vnum, a planet, O/C exit states and no description.
fn lotj_gmcp(vnum: u32, name: &str, exits: &[&str]) -> RoomObservation {
    let exits = exits
        .iter()
        .map(|e| format!("\"{e}\":\"O\""))
        .collect::<Vec<_>>()
        .join(",");
    let raw = format!(r#"Room.Info {{"vnum":{vnum},"name":"{name}","planet":"Academy","exits":{{{exits}}}}}"#);
    crate::map::decode::from_gmcp(&parse_gmcp(raw.as_bytes()).unwrap()).unwrap()
}

/// LotJ's room text: a coloured title with its flags, the description, `Obvious exits:` and
/// one line per exit, then the occupants and an unterminated HP prompt.
fn lotj_text(title: &str, description: &[&str], exits: &[(&str, &str)]) -> String {
    let mut text = format!("\x1b[1;36m{title}\x1b[0m\r\n");
    for line in description {
        text.push_str(&format!("\x1b[0;37m{line}\x1b[0m\r\n"));
    }
    text.push_str("\x1b[1;37mObvious exits:\x1b[0m\r\n");
    for (direction, to) in exits {
        text.push_str(&format!("{direction} - {to}\r\n"));
    }
    text.push_str("\x1b[1;33mA medical droid is here, humming quietly.\x1b[0m\r\n");
    text.push_str("\x1b[0;35mA bacta canister lies here.\x1b[0m\r\n");
    text.push_str("\r\n*((==HP==((|||| ");
    text
}

fn bacta_text() -> String {
    lotj_text(
        "TRAINING: Bacta Tanks [Bacta]",
        &[
            "Rows of tall glass tanks line the walls, each filled with",
            "a thick blue fluid.  A medical droid hovers between them, checking readouts.",
        ],
        &[("Southwest", "TRAINING: HP"), ("North", "TRAINING: Hall")],
    )
}

fn session() -> MapSession {
    let mut map = MapSession::new();
    map.set_gmcp(OptionState::Enabled);
    map
}

fn description(map: &MapSession, id: &str) -> String {
    map.tracker().room(id).unwrap().description.clone()
}

#[test]
fn gmcp_room_without_description_takes_the_text_after_it() {
    let mut map = session();
    let now = Instant::now();
    map.observe_room(
        lotj_gmcp(100, "TRAINING: Bacta Tanks", &["southwest", "north"]),
        gate(),
        now,
    );
    assert_eq!(description(&map, "s:100"), "");
    map.track_output(&bacta_text(), gate(), now + Duration::from_millis(20));
    assert_eq!(description(&map, "s:100"), BACTA);
    let room = map.tracker().room("s:100").unwrap();
    assert_eq!(room.name, "TRAINING: Bacta Tanks", "the protocol keeps the name");
    assert!(!room.is_manually_edited);
    assert!(!map.tracker().can_undo(), "an observation, not an edit");
    assert!(!map.evidence.received_description, "the protocol sent none");
    assert_eq!(ids(search(map.tracker().rooms(), "bacta fluid")), ["s:100"]);
}

#[test]
fn gmcp_room_after_its_text_takes_the_block_before_it() {
    let mut map = session();
    let now = Instant::now();
    map.observe_room(lotj_gmcp(1, "TRAINING: Hall", &["south"]), gate(), now);
    map.track_command("south", false, now);
    // The text first, its prompt not yet ended; then the room change.
    let text = bacta_text();
    let (head, tail) = text.split_at(text.find("A medical droid").unwrap());
    map.track_output(head, gate(), now + Duration::from_millis(5));
    map.observe_room(
        lotj_gmcp(100, "TRAINING: Bacta Tanks", &["southwest", "north"]),
        gate(),
        now + Duration::from_millis(10),
    );
    map.track_output(tail, gate(), now + Duration::from_millis(12));
    assert_eq!(description(&map, "s:100"), BACTA);
    assert_eq!(description(&map, "s:1"), "", "the room before is not given this text");
}

#[test]
fn a_whole_block_before_the_room_change_is_used_too() {
    let mut map = session();
    let now = Instant::now();
    map.observe_room(lotj_gmcp(1, "TRAINING: Hall", &["south"]), gate(), now);
    map.track_command("south", false, now);
    map.track_output(&bacta_text(), gate(), now + Duration::from_millis(5));
    map.observe_room(
        lotj_gmcp(100, "TRAINING: Bacta Tanks", &["southwest", "north"]),
        gate(),
        now + Duration::from_millis(9),
    );
    assert_eq!(description(&map, "s:100"), BACTA);
}

#[test]
fn text_and_room_too_far_apart_or_across_a_command_are_not_paired() {
    let mut map = session();
    let now = Instant::now();
    map.observe_room(lotj_gmcp(100, "TRAINING: Bacta Tanks", &["southwest"]), gate(), now);
    map.track_output(
        &bacta_text(),
        gate(),
        now + DESCRIPTION_WINDOW + Duration::from_millis(1),
    );
    assert_eq!(description(&map, "s:100"), "");

    let mut map = session();
    map.track_output(&bacta_text(), gate(), now);
    map.track_command("look", false, now);
    map.observe_room(lotj_gmcp(100, "TRAINING: Bacta Tanks", &["southwest"]), gate(), now);
    assert_eq!(description(&map, "s:100"), "", "text from before a command is old");
}

#[test]
fn a_prompt_mark_ends_the_block_waiting_for_its_last_line() {
    let mut map = session();
    let now = Instant::now();
    map.observe_room(lotj_gmcp(100, "TRAINING: Bacta Tanks", &["southwest"]), gate(), now);
    map.track_output(
        "\x1b[1mTRAINING: Bacta Tanks [Bacta]\x1b[0m\r\nBlue fluid fills the tanks.\r\nObvious exits:\r\nSouthwest - TRAINING: HP\r\n",
        gate(),
        now,
    );
    assert_eq!(description(&map, "s:100"), "", "the exits may go on");
    map.prompt_seen(now);
    assert_eq!(description(&map, "s:100"), "Blue fluid fills the tanks.");
}

#[test]
fn titles_match_with_flags_case_and_spacing() {
    assert!(title_matches("TRAINING: Bacta Tanks [Bacta]", "TRAINING: Bacta Tanks"));
    assert!(title_matches("Docking Bay 94 [Hotel] [Engine]", "docking  bay 94"));
    assert!(title_matches("A Ward [Hospital]", "A Ward [Hospital]"));
    assert!(!title_matches("TRAINING: Bacta Tanks", "TRAINING: HP"));
    assert!(!title_matches("[Bacta]", ""));
}

#[test]
fn a_block_with_another_title_or_contradicting_exits_is_not_used() {
    let mut map = session();
    let now = Instant::now();
    map.observe_room(lotj_gmcp(100, "TRAINING: Bacta Tanks", &["southwest"]), gate(), now);
    map.track_output(
        &lotj_text(
            "TRAINING: HP",
            &["A bare room."],
            &[("Northeast", "TRAINING: Bacta Tanks")],
        ),
        gate(),
        now,
    );
    assert_eq!(description(&map, "s:100"), "");

    let mut map = session();
    map.observe_room(lotj_gmcp(100, "TRAINING: Bacta Tanks", &["up"]), gate(), now);
    map.track_output(&bacta_text(), gate(), now);
    assert_eq!(description(&map, "s:100"), "", "a same-named room elsewhere");
}

#[test]
fn brief_mode_leaves_the_description_empty() {
    let mut map = session();
    let now = Instant::now();
    map.observe_room(lotj_gmcp(100, "TRAINING: Bacta Tanks", &["southwest"]), gate(), now);
    map.track_output(
        &lotj_text("TRAINING: Bacta Tanks [Bacta]", &[], &[("Southwest", "TRAINING: HP")]),
        gate(),
        now,
    );
    assert_eq!(description(&map, "s:100"), "");
    // The brief block was this room's: the next block is not taken for it either.
    map.track_output(
        &lotj_text(
            "TRAINING: Bacta Tanks [Bacta]",
            &["Later text."],
            &[("Southwest", "TRAINING: HP")],
        ),
        gate(),
        now,
    );
    assert_eq!(description(&map, "s:100"), "");
}

#[test]
fn occupants_and_objects_after_the_exits_stay_out() {
    let mut map = session();
    let now = Instant::now();
    map.observe_room(
        lotj_gmcp(100, "TRAINING: Bacta Tanks", &["southwest", "north"]),
        gate(),
        now,
    );
    map.track_output(&bacta_text(), gate(), now);
    let text = description(&map, "s:100");
    assert!(!text.contains("droid is here") && !text.contains("canister") && !text.contains("Obvious"));
    assert!(!text.contains("HP"), "{text}");
}

#[test]
fn a_weather_line_does_not_churn_and_a_new_text_needs_two_visits() {
    let mut map = session();
    let mut now = Instant::now();
    let visit = |map: &mut MapSession, now: Instant, lines: &[&str]| {
        map.track_command("look", false, now);
        map.observe_room(lotj_gmcp(7, "A Forest Road", &["north"]), gate(), now);
        map.track_output(
            &lotj_text("A Forest Road", lines, &[("North", "A Clearing")]),
            gate(),
            now,
        );
    };
    let road = "Tall oaks lean over a rutted road of packed earth and old stones.";
    visit(&mut map, now, &[road, "The sun shines brightly."]);
    let first = description(&map, "s:7");
    assert_eq!(first, format!("{road} The sun shines brightly."));
    let revision = map.tracker().room("s:7").unwrap().revision;
    for weather in ["Rain falls steadily.", "Rain falls steadily.", "Snow drifts down."] {
        now += Duration::from_secs(5);
        visit(&mut map, now, &[road, weather]);
        assert_eq!(description(&map, "s:7"), first, "{weather}");
    }
    assert_eq!(map.tracker().room("s:7").unwrap().revision, revision);

    // A rewritten room: once is not enough, twice in a row is.
    let rewritten = "A wide paved highway runs between new stone houses.";
    now += Duration::from_secs(5);
    visit(&mut map, now, &[rewritten]);
    assert_eq!(description(&map, "s:7"), first);
    now += Duration::from_secs(5);
    visit(&mut map, now, &[road, "The sun shines brightly."]);
    now += Duration::from_secs(5);
    visit(&mut map, now, &[rewritten]);
    assert_eq!(description(&map, "s:7"), first, "the visits in between broke the run");
    now += Duration::from_secs(5);
    visit(&mut map, now, &[rewritten]);
    assert_eq!(description(&map, "s:7"), rewritten);
    assert!(map.tracker().room("s:7").unwrap().revision > revision);
}

#[test]
fn a_description_edited_by_hand_is_never_replaced() {
    let mut map = session();
    let now = Instant::now();
    map.observe_room(
        lotj_gmcp(100, "TRAINING: Bacta Tanks", &["southwest", "north"]),
        gate(),
        now,
    );
    let mut room = map.tracker().room("s:100").unwrap().clone();
    room.description = "My own notes.".into();
    assert!(map.tracker_mut().upsert_room(room));
    for i in 0..3 {
        let at = now + Duration::from_secs(i * 5);
        map.track_command("look", false, at);
        map.observe_room(
            lotj_gmcp(100, "TRAINING: Bacta Tanks", &["southwest", "north"]),
            gate(),
            at,
        );
        map.track_output(&bacta_text(), gate(), at);
    }
    assert_eq!(description(&map, "s:100"), "My own notes.");

    // Edited with the description left empty: still the person's room.
    let mut map = session();
    map.observe_room(
        lotj_gmcp(100, "TRAINING: Bacta Tanks", &["southwest", "north"]),
        gate(),
        now,
    );
    let mut room = map.tracker().room("s:100").unwrap().clone();
    room.notes = "Heal here".into();
    assert!(map.tracker_mut().upsert_room(room));
    map.track_command("look", false, now);
    map.observe_room(
        lotj_gmcp(100, "TRAINING: Bacta Tanks", &["southwest", "north"]),
        gate(),
        now,
    );
    map.track_output(&bacta_text(), gate(), now);
    assert_eq!(description(&map, "s:100"), "");
}

#[test]
fn a_world_that_sends_descriptions_is_unaffected() {
    let mut map = session();
    let now = Instant::now();
    let raw = r#"Room.Info {"num":5,"name":"Market Square","desc":"Stalls crowd the square.","exits":{"n":6}}"#;
    let room = crate::map::decode::from_gmcp(&parse_gmcp(raw.as_bytes()).unwrap()).unwrap();
    map.observe_room(room, gate(), now);
    map.track_output(
        &lotj_text(
            "Market Square",
            &["Some other words entirely, shown by the text."],
            &[("North", "Road")],
        ),
        gate(),
        now,
    );
    assert_eq!(description(&map, "s:5"), "Stalls crowd the square.");
    assert!(map.evidence.received_description);
}

#[test]
fn msdp_rooms_are_filled_the_same_way() {
    let mut map = session();
    let now = Instant::now();
    let room = RoomObservation::new(Some("42"), "TRAINING: Bacta Tanks", "", &[("southwest", None)])
        .with_source(RoomSource::Msdp);
    map.observe_room(room, gate(), now);
    map.track_output(&bacta_text(), gate(), now);
    assert_eq!(description(&map, "s:42"), BACTA);
}

#[test]
fn a_long_description_is_capped_and_whitespace_collapsed() {
    let mut map = session();
    let now = Instant::now();
    map.observe_room(lotj_gmcp(100, "TRAINING: Bacta Tanks", &["southwest"]), gate(), now);
    map.track_output(
        &lotj_text(
            "TRAINING: Bacta Tanks",
            &["  Blue\t fluid   glows.  ", "", "  Tanks hum. "],
            &[("Southwest", "HP")],
        ),
        gate(),
        now,
    );
    assert_eq!(description(&map, "s:100"), "Blue fluid glows. Tanks hum.");

    let block = TextBlock {
        lines: std::iter::once("Hall".to_string())
            .chain((0..400).map(|_| "word ".repeat(10)))
            .collect(),
        exits: vec!["north".into()],
    };
    assert!(block.description_for("Hall").is_none(), "over the limit");
}

#[test]
fn the_observer_keeps_blocks_only_when_asked() {
    let mut observer = TextRoomObserver::new();
    observer.feed(&bacta_text());
    assert!(observer.blocks.is_empty());
    observer.keep_blocks = true;
    observer.feed(&bacta_text());
    assert_eq!(observer.blocks.len(), 1);
    assert_eq!(observer.blocks[0].exits, ["southwest", "north"]);
    observer.reset();
    assert!(observer.keep_blocks, "a command keeps the setting");
}

#[test]
fn a_walk_is_not_stopped_by_descriptions_and_gets_them_when_it_ends() {
    let rooms = (1..=3)
        .map(|i| {
            MapRoom::new(
                &format!("s:{i}"),
                &format!("Hall {i}"),
                "",
                None,
                f64::from(i),
                0.0,
                0.0,
                false,
            )
            .with_server_id(&i.to_string())
        })
        .collect();
    let links = vec![
        MapLink::new("s:1", "s:2", "east", true),
        MapLink::new("s:2", "s:3", "east", true),
    ];
    let mut map = MapSession::with_map(MapSnapshot::of(rooms, links.clone()));
    map.set_gmcp(OptionState::Enabled);
    let now = Instant::now();
    map.observe_room(lotj_gmcp(1, "Hall 1", &["east"]), gate(), now);
    let route = crate::map::route::MapRoute {
        steps: links,
        cost: 2.0,
    };
    let mut command = map.start_walk(&route, gate(), now);
    for vnum in 2..=3 {
        map.track_command(command.as_deref().unwrap(), false, now);
        map.observe_room(lotj_gmcp(vnum, &format!("Hall {vnum}"), &["east", "west"]), gate(), now);
        map.track_output(
            &lotj_text(
                &format!("Hall {vnum}"),
                &["Plain walls."],
                &[("East", "Next"), ("West", "Back")],
            ),
            gate(),
            now,
        );
        command = map.advance_walk(gate(), now);
    }
    assert_eq!(map.walk_status(), WalkStatus::Complete);
    assert_eq!(description(&map, "s:2"), "Plain walls.");
    assert_eq!(description(&map, "s:3"), "Plain walls.");
}

fn ids(rooms: Vec<&MapRoom>) -> Vec<&str> {
    rooms.iter().map(|r| r.id.as_str()).collect()
}
