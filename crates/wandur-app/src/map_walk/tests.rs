//! Walking and saved maps over loopback (the C# `MapNavigationTests`): a session tab against a
//! socket the test plays the world on, with Room 1 north to Room 2 east to Room 3.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use wandur_core::map::{MapLink, MapRoom, MapRoute, MapSession, MapSnapshot, RoomSource, TrackingState, WalkStatus};

use crate::session_tab::test_support::{local_tab, pump_until};
use crate::session_tab::{SessionTab, TabOptions};

fn gmcp(message: &str) -> Vec<u8> {
    let mut out = vec![255, 250, 201];
    out.extend_from_slice(message.as_bytes());
    out.extend_from_slice(&[255, 240]);
    out
}

fn room(id: &str) -> Vec<u8> {
    gmcp(&format!(r#"Room.Info {{"num":"{id}","name":"Room {id}"}}"#))
}

#[derive(Clone, Copy, PartialEq)]
enum Telnet {
    Text,
    Iac,
    Option,
    Sub,
    SubIac,
}

struct World {
    tab: SessionTab,
    server: TcpStream,
    route: MapRoute,
    telnet: Telnet,
    line: Vec<u8>,
    /// Commands the client sent, oldest first (telnet negotiation dropped).
    sent: std::collections::VecDeque<String>,
}

impl World {
    fn open() -> World {
        let (mut tab, mut server) = local_tab();
        let rooms = vec![
            MapRoom::new("s:1", "Room 1", "", None, 0.0, 0.0, 0.0, false).with_server_id("1"),
            MapRoom::new("s:2", "Room 2", "", None, 0.0, 1.0, 0.0, false).with_server_id("2"),
            MapRoom::new("s:3", "Room 3", "", None, 1.0, 1.0, 0.0, false).with_server_id("3"),
        ];
        let links = vec![
            MapLink::new("s:1", "s:2", "north", true),
            MapLink::new("s:2", "s:3", "east", true),
        ];
        tab.map = MapSession::with_map(MapSnapshot::of(rooms, links.clone()));
        pump_until(&mut tab, SessionTab::is_connected);
        let mut hello = vec![255, 251, 201];
        hello.extend(room("1"));
        server.write_all(&hello).unwrap();
        pump_until(&mut tab, |t| t.map.tracker().state() == TrackingState::Confirmed);
        let mut world = World {
            tab,
            server,
            route: MapRoute {
                steps: links,
                cost: 2.0,
            },
            telnet: Telnet::Text,
            line: Vec::new(),
            sent: Default::default(),
        };
        world.read_for(Duration::from_millis(100));
        world.sent.clear();
        world
    }

    /// Read what the client sends for a while, pumping the tab.
    fn read_for(&mut self, time: Duration) {
        let deadline = Instant::now() + time;
        self.server.set_read_timeout(Some(Duration::from_millis(10))).unwrap();
        let mut buf = [0u8; 512];
        while Instant::now() < deadline {
            self.tab.pump(Instant::now());
            let n = match self.server.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => continue,
            };
            for &b in &buf[..n] {
                self.telnet = match (self.telnet, b) {
                    (Telnet::Text, 255) => Telnet::Iac,
                    (Telnet::Text, b'\n') => {
                        let line = String::from_utf8_lossy(&self.line).trim().to_string();
                        self.line.clear();
                        if !line.is_empty() {
                            self.sent.push_back(line);
                        }
                        Telnet::Text
                    }
                    (Telnet::Text, b) => {
                        self.line.push(b);
                        Telnet::Text
                    }
                    (Telnet::Iac, 250) => Telnet::Sub,
                    (Telnet::Iac, 251..=254) => Telnet::Option,
                    (Telnet::Iac | Telnet::Option, _) => Telnet::Text,
                    (Telnet::Sub, 255) => Telnet::SubIac,
                    (Telnet::Sub, _) => Telnet::Sub,
                    (Telnet::SubIac, 240) => Telnet::Text,
                    (Telnet::SubIac, _) => Telnet::Sub,
                };
            }
        }
    }

    /// The next command, waiting up to three seconds.
    fn command(&mut self) -> String {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(c) = self.sent.pop_front() {
                return c;
            }
            assert!(Instant::now() < deadline, "no command arrived");
            self.read_for(Duration::from_millis(20));
        }
    }

    /// Nothing more is sent.
    fn quiet(&mut self) {
        self.read_for(Duration::from_millis(150));
        assert_eq!(self.sent.drain(..).collect::<Vec<_>>(), Vec::<String>::new());
    }

    fn output(&mut self, bytes: &[u8]) {
        self.server.write_all(bytes).unwrap();
        self.read_for(Duration::from_millis(60));
    }

    fn room(&mut self, id: &str) {
        self.server.write_all(&room(id)).unwrap();
        let want = format!("s:{id}");
        pump_until(&mut self.tab, |t| t.map.tracker().current_id() == Some(want.as_str()));
    }

    fn walk(&mut self) {
        let route = self.route.clone();
        self.tab.walk_route(&route);
    }
}

#[test]
fn a_walk_sends_one_step_and_waits_for_the_expected_room() {
    let mut w = World::open();
    w.walk();
    assert_eq!(w.command(), "north");
    w.quiet();
    assert!(w.tab.map.is_walking());
    w.room("2");
    assert_eq!(w.command(), "east");
    w.room("3");
    w.read_for(Duration::from_millis(30));
    assert!(!w.tab.map.is_walking());
    assert_eq!(w.tab.map.walk_status(), WalkStatus::Complete);
    assert_eq!(w.tab.map.tracker().current_id(), Some("s:3"));
}

#[test]
fn the_walker_recognizes_vnum_arrivals_and_exit_flags_are_no_rooms() {
    let mut w = World::open();
    w.walk();
    assert_eq!(w.command(), "north");
    w.output(&gmcp(
        r#"Room.Info {"vnum":2,"name":"Room 2","exits":{"east":"O"},"planet":"Ring of Kafrene"}"#,
    ));
    assert_eq!(w.command(), "east");
    w.output(&gmcp(
        r#"Room.Info {"vnum":3,"name":"Room 3","planet":"Ring of Kafrene"}"#,
    ));
    assert!(!w.tab.map.is_walking());
    assert_eq!(w.tab.map.tracker().current_id(), Some("s:3"));
    assert!(!w.tab.map.tracker().links().any(|l| l.to_id == "s:O"));
}

#[test]
fn a_wrong_room_or_two_arrivals_in_one_burst_never_advance() {
    let mut w = World::open();
    w.walk();
    assert_eq!(w.command(), "north");
    let mut burst = room("2");
    burst.extend(room("1"));
    w.output(&burst);
    assert!(!w.tab.map.is_walking());
    assert_eq!(w.tab.map.walk_status(), WalkStatus::WrongRoom);
    w.quiet();
}

#[test]
fn a_failure_or_a_password_prompt_in_the_arrival_burst_prevents_the_next_move() {
    for (text, why) in [
        ("The door is closed.\n> ", WalkStatus::Blocked),
        ("Password: ", WalkStatus::Private),
    ] {
        let mut w = World::open();
        w.walk();
        assert_eq!(w.command(), "north");
        let mut burst = room("2");
        burst.extend_from_slice(text.as_bytes());
        w.output(&burst);
        assert!(!w.tab.map.is_walking(), "{text}");
        assert_eq!(w.tab.map.walk_status(), why, "{text}");
        w.quiet();
    }
}

#[test]
fn unsafe_or_unacknowledged_steps_stop_without_another_command() {
    for reason in ["blocked", "private", "echo-burst", "stop", "disconnect", "timeout"] {
        let mut w = World::open();
        w.tab.map.step_timeout = Duration::from_millis(150);
        w.walk();
        assert_eq!(w.command(), "north", "{reason}");
        let expected = match reason {
            "blocked" => {
                w.output(b"The door is closed.\n> ");
                WalkStatus::Blocked
            }
            "private" => {
                w.tab.set_manual_private(true);
                WalkStatus::Private
            }
            "echo-burst" => {
                let mut burst = vec![255, 251, 1, 255, 252, 1];
                burst.extend(room("2"));
                w.output(&burst);
                WalkStatus::Private
            }
            "stop" => {
                w.tab.stop_walk();
                WalkStatus::Stopped
            }
            "disconnect" => {
                w.tab.disconnect();
                WalkStatus::Disconnected
            }
            _ => {
                w.read_for(Duration::from_millis(250));
                WalkStatus::Timeout
            }
        };
        w.read_for(Duration::from_millis(30));
        assert!(!w.tab.map.is_walking(), "{reason}");
        assert_eq!(w.tab.map.walk_status(), expected, "{reason}");
        if !matches!(reason, "disconnect" | "echo-burst") {
            w.quiet();
        }
    }
}

#[test]
fn an_unsupported_later_command_rejects_the_whole_route_before_sending() {
    let mut w = World::open();
    let custom = MapLink {
        command: Some("drop all".into()),
        ..w.route.steps[1].clone()
    };
    assert!(w.tab.map.tracker_mut().upsert_link(custom));
    let route = wandur_core::map::find_route_live(w.tab.map.tracker(), "s:1", "s:3", false).unwrap();
    w.tab.walk_route(&route);
    assert!(!w.tab.map.is_walking());
    assert_eq!(w.tab.map.walk_status(), WalkStatus::CustomCommand);
    w.quiet();
}

#[test]
fn a_manual_command_takes_over_and_a_late_arrival_cannot_restart_walking() {
    let mut w = World::open();
    w.walk();
    assert_eq!(w.command(), "north");
    w.tab.input = "look".into();
    w.tab.submit();
    assert_eq!(w.command(), "look");
    assert_eq!(w.tab.map.walk_status(), WalkStatus::ManualCommand);
    w.room("2");
    w.quiet();
}

#[test]
fn editing_the_map_stops_an_outstanding_step() {
    let mut w = World::open();
    w.walk();
    assert_eq!(w.command(), "north");
    assert!(w.tab.map.tracker_mut().remove_link("s:2", "east"));
    w.read_for(Duration::from_millis(30));
    assert!(!w.tab.map.is_walking());
    assert_eq!(w.tab.map.walk_status(), WalkStatus::GraphChanged);
    w.room("2");
    w.quiet();
}

#[test]
fn an_unacknowledged_manual_move_cannot_start_a_walk() {
    let mut w = World::open();
    w.tab.input = "north".into();
    w.tab.submit();
    assert_eq!(w.command(), "north");
    w.walk();
    assert!(!w.tab.map.is_walking());
    assert_eq!(w.tab.map.walk_status(), WalkStatus::Unavailable);
    w.quiet();
}

#[test]
fn rejected_room_data_cannot_complete_the_last_step() {
    let mut w = World::open();
    let one = MapRoute {
        steps: vec![w.route.steps[0].clone()],
        cost: 1.0,
    };
    w.tab.walk_route(&one);
    assert_eq!(w.command(), "north");
    w.output(&gmcp(&format!(r#"Room.Info {{"num":2,"name":"{}"}}"#, "x".repeat(513))));
    assert_eq!(w.tab.map.tracker().current_id(), Some("s:1"));
    assert_ne!(w.tab.map.walk_status(), WalkStatus::Complete);
    w.quiet();
}

#[test]
fn evidence_distinguishes_negotiation_from_received_fields() {
    let mut w = World::open();
    let e = w.tab.map.evidence.clone();
    assert_eq!(e.gmcp, wandur_core::map::OptionState::Enabled);
    assert_eq!(e.msdp, wandur_core::map::OptionState::Unknown);
    assert!(e.received_room_id && !e.received_exits && !e.received_terrain && !e.received_coordinates);
    w.output(&gmcp(
        r#"Room.Info {"num":1,"name":"Room 1","exits":{},"environment":"forest","coords":{"x":2,"y":3,"z":0}}"#,
    ));
    let e = w.tab.map.evidence.clone();
    assert!(e.received_exits && e.received_terrain && e.received_coordinates);
    assert_eq!(e.last_room_source, Some(RoomSource::Gmcp));
    w.output(&[255, 252, 69]);
    assert_eq!(w.tab.map.evidence.summary(), "GMCP: Supported · MSDP: Declined");
}

#[test]
fn msdp_rooms_track_and_walk_too() {
    let mut w = World::open();
    let msdp = |id: &str| {
        let mut out = vec![255, 250, 69];
        out.extend_from_slice(format!("\x01ROOM_VNUM\x02{id}\x01ROOM_NAME\x02Room {id}").as_bytes());
        out.extend_from_slice(&[255, 240]);
        out
    };
    let mut on = vec![255, 251, 69];
    on.extend(msdp("1"));
    w.output(&on);
    assert_eq!(w.tab.map.tracker().source(), RoomSource::Msdp);
    w.walk();
    assert_eq!(w.command(), "north");
    w.output(&msdp("2"));
    assert_eq!(w.command(), "east");
    w.output(&msdp("3"));
    assert_eq!(w.tab.map.walk_status(), WalkStatus::Complete);
}

#[test]
fn maps_persist_and_two_sessions_of_one_world_merge() {
    use std::sync::Arc;
    use wandur_core::map::store::{MapStore, MapWorker, MapWorld};
    let dir = std::env::temp_dir().join(format!("wandur-app-maps-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (db, _) = wandur_core::db::Database::open(&dir).unwrap();
    let worker = Arc::new(MapWorker::spawn(MapStore::new(db.clone())).unwrap());
    let world = MapWorld::Id(wandur_core::db::worlds::new_world_id());
    let options = || TabOptions {
        maps: Some(crate::session_tab::MapsConfig {
            worker: Arc::clone(&worker),
            world: world.clone(),
        }),
        ..TabOptions::default()
    };
    let gmcp_room = |id: &str, name: &str| {
        wandur_core::map::decode::from_gmcp(
            &wandur_core::protocol::parse_gmcp(format!(r#"Room.Info {{"num":"{id}","name":"{name}"}}"#).as_bytes())
                .unwrap(),
        )
        .unwrap()
    };
    // Two sessions of one world, each finding a room of its own.
    let open = |name: &str| {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = wandur_core::Endpoint::new("127.0.0.1", listener.local_addr().unwrap().port());
        let tab = SessionTab::open(7, endpoint, &options(), Arc::new(|| {}));
        let _ = name;
        (tab, listener.accept().unwrap().0)
    };
    let (mut first, _a) = open("first");
    let (mut second, _b) = open("second");
    pump_until(&mut first, |t| t.map.is_loaded());
    pump_until(&mut second, |t| t.map.is_loaded());
    let gate = first.walk_gate();
    first.map.observe_room(gmcp_room("1", "Hall"), gate, Instant::now());
    second.map.observe_room(gmcp_room("2", "Tower"), gate, Instant::now());
    first.save_map(true);
    second.save_map(true);
    worker.flush();
    // A third session of the world loads both.
    let (mut third, _c) = open("third");
    pump_until(&mut third, |t| t.map.is_loaded());
    let mut names: Vec<String> = third.map.tracker().rooms().map(|r| r.name.clone()).collect();
    names.sort();
    assert_eq!(names, ["Hall", "Tower"]);
    assert_eq!(third.map.tracker().current_id(), None, "the position is never saved");
    drop((first, second, third));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rooms_seen_while_the_map_loads_are_kept_and_placed_on_it() {
    use std::sync::Arc;
    use wandur_core::map::store::{MapStore, MapWorker, MapWorld};
    let dir = std::env::temp_dir().join(format!("wandur-app-maps-load-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (db, _) = wandur_core::db::Database::open(&dir).unwrap();
    let store = MapStore::new(db.clone());
    let world = MapWorld::Id(wandur_core::db::worlds::new_world_id());
    store
        .save(
            &world,
            &MapSnapshot::of(
                vec![MapRoom::new("s:1", "Hall", "", None, 0.0, 0.0, 0.0, false).with_server_id("1")],
                Vec::new(),
            ),
        )
        .unwrap();
    let worker = Arc::new(MapWorker::spawn(store).unwrap());
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = wandur_core::Endpoint::new("127.0.0.1", listener.local_addr().unwrap().port());
    let options = TabOptions {
        maps: Some(crate::session_tab::MapsConfig {
            worker: Arc::clone(&worker),
            world,
        }),
        ..TabOptions::default()
    };
    let mut tab = SessionTab::open(9, endpoint, &options, Arc::new(|| {}));
    let (mut server, _) = listener.accept().unwrap();
    let mut hello = vec![255, 251, 201];
    hello.extend(room("1"));
    server.write_all(&hello).unwrap();
    pump_until(&mut tab, |t| {
        t.map.is_loaded() && t.map.tracker().current_id() == Some("s:1")
    });
    assert_eq!(tab.map.tracker().room_count(), 1, "the saved room, not a copy");
    drop(tab);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_offline_demo_is_mapped_from_its_text() {
    let mut tab = SessionTab::demo(3, &TabOptions::default());
    tab.input = "north".into();
    tab.submit();
    tab.input = "south".into();
    tab.submit();
    let tracker = tab.map.tracker();
    assert!(tracker.room_count() >= 2, "{}", tracker.room_count());
    assert!(
        tracker.links().any(|l| l.direction == "north"),
        "the walked exit is learned"
    );
    assert_eq!(tracker.source(), RoomSource::Text);
}
