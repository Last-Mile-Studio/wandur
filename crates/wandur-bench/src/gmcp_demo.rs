//! A small GMCP world for trying the Channels and Map panels on a loopback server: it offers
//! GMCP, walks a loop of rooms (sending `Room.Info` with numbered exits, including stairs), and
//! talks on a few channels (`Comm.Channel.Text`, with the line printed as worlds usually do). It
//! ignores what the client sends.

use std::io::Write;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

const IAC: u8 = 255;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;
const GMCP: u8 = 201;

/// Rooms: number, name, area, environment, exits (direction, number).
type Room = (
    u32,
    &'static str,
    &'static str,
    &'static str,
    &'static [(&'static str, u32)],
);

const ROOMS: [Room; 9] = [
    (100, "Town Gate", "Lantern Town", "urban", &[("n", 101), ("e", 103)]),
    (
        101,
        "Market Road",
        "Lantern Town",
        "urban",
        &[("s", 100), ("n", 102), ("e", 104)],
    ),
    (
        102,
        "Old Square",
        "Lantern Town",
        "urban",
        &[("s", 101), ("e", 105), ("u", 108)],
    ),
    (103, "Smithy", "Lantern Town", "indoors", &[("w", 100), ("n", 104)]),
    (
        104,
        "Fountain",
        "Lantern Town",
        "urban",
        &[("w", 101), ("s", 103), ("n", 105), ("e", 106)],
    ),
    (105, "Temple Steps", "Lantern Town", "urban", &[("w", 102), ("s", 104)]),
    (106, "East Field", "Greenway", "field", &[("w", 104), ("ne", 107)]),
    (107, "Willow Copse", "Greenway", "forest", &[("sw", 106)]),
    (108, "Bell Tower", "Lantern Town", "indoors", &[("d", 102)]),
];

/// The walk: (direction walked, room reached).
const WALK: [(&str, u32); 12] = [
    ("north", 101),
    ("north", 102),
    ("up", 108),
    ("down", 102),
    ("east", 105),
    ("south", 104),
    ("east", 106),
    ("northeast", 107),
    ("southwest", 106),
    ("west", 104),
    ("south", 103),
    ("west", 100),
];

const CHATTER: [(&str, &str, &str); 8] = [
    ("gossip", "Ann", "Anyone up for the bell tower run?"),
    (
        "newbie",
        "Mentor",
        "Welcome! Type \u{1b}[1;33mhelp start\u{1b}[0m to begin.",
    ),
    ("gossip", "Bob", "The smithy has new \u{1b}[36msteel\u{1b}[0m in stock."),
    ("tell", "Cyra", "Meet me at the fountain."),
    ("ooc", "Dain", "brb, tea"),
    ("gossip", "Ann", "Found the willow copse, it's lovely."),
    ("newbie", "Elin", "How do I see the map?"),
    ("newbie", "Mentor", "The Map panel builds itself as you walk."),
];

fn gmcp(package: &str, json: &str) -> Vec<u8> {
    let mut out = vec![IAC, SB, GMCP];
    out.extend_from_slice(package.as_bytes());
    out.push(b' ');
    out.extend_from_slice(json.as_bytes());
    out.extend_from_slice(&[IAC, SE]);
    out
}

fn room_info(number: u32) -> Vec<u8> {
    let (num, name, area, env, exits) = ROOMS.iter().find(|r| r.0 == number).copied().unwrap_or(ROOMS[0]);
    let exits: Vec<String> = exits.iter().map(|(d, n)| format!("\"{d}\":{n}")).collect();
    gmcp(
        "Room.Info",
        &format!(
            r#"{{"num":{num},"name":"{name}","area":"{area}","environment":"{env}","exits":{{{}}}}}"#,
            exits.join(",")
        ),
    )
}

/// Serve one client until it goes away or the server stops.
pub fn serve(stream: &mut TcpStream, stop: &AtomicBool, sent: &std::sync::atomic::AtomicU64) {
    let mut write = |bytes: &[u8]| -> bool {
        if stream.write_all(bytes).is_err() {
            return false;
        }
        sent.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        true
    };
    if !write(&[IAC, WILL, GMCP]) {
        return;
    }
    // Give the client time to answer DO GMCP.
    thread::sleep(Duration::from_millis(300));
    let mut out = b"\x1b[1;32mTown Gate\x1b[0m\r\nA wide gate in the old wall. Roads lead north and east.\r\n".to_vec();
    out.extend(room_info(100));
    if !write(&out) {
        return;
    }
    let mut step = 0usize;
    while !stop.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(1500));
        let (dir, number) = WALK[step % WALK.len()];
        let name = ROOMS.iter().find(|r| r.0 == number).map_or("Somewhere", |r| r.1);
        let mut out =
            format!("You walk {dir}.\r\n\x1b[1;32m{name}\x1b[0m\r\nNothing much happens here.\r\n").into_bytes();
        out.extend(room_info(number));
        if step.is_multiple_of(2) {
            let (channel, talker, text) = CHATTER[(step / 2) % CHATTER.len()];
            let json = serde_json::json!({"channel": channel, "talker": talker, "text": format!("[{channel}] {talker}: {text}")});
            out.extend(format!("\x1b[35m[{channel}] {talker}: {text}\x1b[0m\r\n").into_bytes());
            out.extend(gmcp("Comm.Channel.Text", &json.to_string()));
        }
        out.extend_from_slice(b"\x1b[32m<500hp 200m 300mv>\x1b[0m ");
        if !write(&out) {
            return;
        }
        step += 1;
    }
}
