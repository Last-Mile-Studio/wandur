//! The Lantern Road, the original fictional world of the C# client's help and reference
//! screenshots (`LanternRoadSession`), served over loopback so the Rust client's scenes show the
//! same session: GMCP `Char.Status` (Odo), `Char.Vitals`, the guild's conversation and three
//! other channels, the 21 rooms walked so the map is complete and the player stands at Lantern
//! Crossroads, MSSP, then the showcase transcript. Afterwards it answers a few commands (`lamps`,
//! `note`, `wave`, `wisp`, `clan`).

use std::io::Write;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

const IAC: u8 = 255;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;
const GMCP: u8 = 201;
const MSSP: u8 = 70;

pub const GUILD: [(&str, &str); 5] = [
    ("Wren", "Lanterns are lit at the crossroads. Road is clear to the ford."),
    ("Odo", "Watch the marsh after dusk, the will-o-wisps are out again."),
    ("Wren", "Meet at the Wayfarer's Rest? I'll save the corner table."),
    ("Bastian", "Bringing the map. Someone found a cave under the falls."),
    ("Odo", "On my way. Keep the kettle warm."),
];

/// The other channels of the C# tour (one message each).
pub const OTHERS: [(&str, &str, &str); 3] = [
    ("ooc", "Bastian", "Anyone know if the ferry runs at night?"),
    ("chat", "Wren", "New lantern oil at the market, half price till dusk."),
    ("tell", "Wren", "Saved you the corner table. Bring the map?"),
];

/// Id, name, terrain, exits (direction:room).
pub const ROOMS: [(&str, &str, &str, &[&str]); 21] = [
    ("rest", "The Wayfarer's Rest", "indoor", &["east:cross"]),
    (
        "cross",
        "Lantern Crossroads",
        "road",
        &["west:rest", "east:road2", "north:gate", "south:meadow"],
    ),
    (
        "gate",
        "Old Town Gate",
        "city",
        &["south:cross", "north:market", "east:lane"],
    ),
    ("market", "Market Square", "city", &["south:gate", "east:well"]),
    ("well", "Well Street", "city", &["west:market", "south:lane"]),
    ("lane", "Chandler's Lane", "city", &["west:gate", "north:well"]),
    ("road2", "The Lantern Road", "road", &["west:cross", "east:road3"]),
    (
        "road3",
        "The Lantern Road",
        "road",
        &["west:road2", "east:ford", "north:wood1"],
    ),
    (
        "wood1",
        "Edge of Hollowwood",
        "forest",
        &["south:road3", "north:wood2", "east:wood3"],
    ),
    ("wood2", "Hollowwood Deeps", "forest", &["south:wood1"]),
    ("wood3", "Mossy Clearing", "forest", &["west:wood1", "north:hill"]),
    ("hill", "Watcher's Hill", "grassland", &["south:wood3", "east:peak"]),
    ("peak", "Greywind Pass", "mountain", &["west:hill"]),
    ("ford", "Willow Ford", "water", &["west:road3", "east:road4"]),
    ("road4", "The Lantern Road", "road", &["west:ford", "south:falls"]),
    ("falls", "Below the Falls", "water", &["north:road4", "west:cave"]),
    ("cave", "Hidden Grotto", "cave", &["east:falls"]),
    (
        "meadow",
        "Barley Meadow",
        "grassland",
        &["north:cross", "east:marsh", "south:farm"],
    ),
    ("farm", "Hollis Farmstead", "indoor", &["north:meadow"]),
    ("marsh", "Lantern Marsh", "swamp", &["west:meadow", "east:marsh2"]),
    ("marsh2", "Reedbank", "swamp", &["west:marsh"]),
];

/// Map coordinates of [`ROOMS`], the C# capture's chart (east is +x, south is +y on screen; the
/// world sends y the other way, north up, as map coordinates are).
pub const COORDINATES: [(i32, i32); 21] = [
    (0, 0),
    (1, 0),
    (1, -1),
    (1, -2),
    (2, -2),
    (2, -1),
    (2, 0),
    (3, 0),
    (3, -1),
    (3, -2),
    (4, -1),
    (4, -2),
    (5, -2),
    (4, 0),
    (5, 0),
    (5, 1),
    (4, 1),
    (1, 1),
    (1, 2),
    (2, 1),
    (3, 1),
];

/// The room the transcript leaves the player in (the crossroads).
pub const START: usize = 1;

/// The showcase transcript; it ends at the crossroads prompt.
pub const TRANSCRIPT: &str = "\x1b[33mT H E   L A N T E R N   R O A D\x1b[0m
Every road remembers who walked it.

\x1b[90m> look\x1b[0m

\x1b[1;33mThe Wayfarer's Rest\x1b[0m
\x1b[90mLantern Crossroads / The Common Room\x1b[0m

A fire crackles under a mantel hung with old lanterns, each one
carried here by a traveller who never came back for it. Rain
ticks against the shutters. Someone has left a map pinned to
the bar, a red thread looping east toward the falls.

\x1b[36mWren\x1b[0m is here, polishing a brass spyglass.

\x1b[33mWren says, \"If you're heading east, take a lantern. The marsh is bad tonight.\"\x1b[0m

\x1b[37mExits:\x1b[0m \x1b[32meast\x1b[0m

\x1b[90m> east\x1b[0m

\x1b[1;33mLantern Crossroads\x1b[0m
Four roads meet beneath a leaning signpost. Lamps hang from every
arm of it, swaying in the wind. North, the old town's gate; south,
barley fields run down toward the marsh.

\x1b[37mExits:\x1b[0m \x1b[32mnorth\x1b[0m, \x1b[32meast\x1b[0m, \x1b[32msouth\x1b[0m, \x1b[32mwest\x1b[0m

\x1b[90m> guild On my way. Keep the kettle warm.\x1b[0m
\x1b[35m[Guild] You:\x1b[0m On my way. Keep the kettle warm.

\x1b[32m312/380 hp\x1b[0m  \x1b[36m146/220 mana\x1b[0m  \x1b[33m188/240 mv\x1b[0m
\x1b[90mLantern Crossroads >\x1b[0m
";

pub fn gmcp(package: &str, json: &serde_json::Value) -> Vec<u8> {
    let mut out = vec![IAC, SB, GMCP];
    out.extend_from_slice(package.as_bytes());
    out.push(b' ');
    out.extend_from_slice(json.to_string().as_bytes());
    out.extend_from_slice(&[IAC, SE]);
    out
}

pub fn room_info(index: usize) -> Vec<u8> {
    let (id, name, terrain, exits) = ROOMS[index];
    let (x, y) = COORDINATES[index];
    let exits: serde_json::Map<String, serde_json::Value> = exits
        .iter()
        .filter_map(|e| e.split_once(':'))
        .map(|(d, to)| (d.to_string(), to.into()))
        .collect();
    gmcp(
        "Room.Info",
        &serde_json::json!({
            "num": id,
            "name": name,
            "area": "The Lantern Road",
            "environment": terrain,
            "exits": exits,
            "coord": {"x": x, "y": -y, "z": 0}
        }),
    )
}

fn mssp(pairs: &[(&str, &str)]) -> Vec<u8> {
    let mut out = vec![IAC, SB, MSSP];
    for (name, value) in pairs {
        out.push(1);
        out.extend_from_slice(name.as_bytes());
        out.push(2);
        out.extend_from_slice(value.as_bytes());
    }
    out.extend_from_slice(&[IAC, SE]);
    out
}

/// Everything the world sends after negotiation, in order.
pub fn script() -> Vec<u8> {
    let mut out = gmcp("Char.Status", &serde_json::json!({"name": "Odo"}));
    out.extend(gmcp(
        "Char.Vitals",
        &serde_json::json!({"hp": 312, "maxhp": 380, "mana": 146, "maxmana": 220, "moves": 188, "maxmoves": 240}),
    ));
    for (speaker, text) in GUILD {
        out.extend(gmcp(
            "Comm.Channel.Text",
            &serde_json::json!({"channel": "guild", "talker": speaker, "text": text}),
        ));
    }
    for (channel, speaker, text) in OTHERS {
        out.extend(gmcp(
            "Comm.Channel.Text",
            &serde_json::json!({"channel": channel, "talker": speaker, "text": text}),
        ));
    }
    // Walked in list order, then back to the crossroads, where the transcript leaves the player.
    for i in (0..ROOMS.len()).chain([1]) {
        out.extend(room_info(i));
    }
    out.extend(mssp(&[
        ("NAME", "The Lantern Road"),
        ("CODEBASE", "Custom"),
        ("PLAYERS", "38"),
        ("UPTIME", "1759900000"),
        ("CONTACT", "keeper@lanternroad.example.org"),
        ("WEBSITE", "https://lanternroad.example.org"),
        ("GENRE", "Fantasy"),
    ]));
    out.extend(TRANSCRIPT.replace('\n', "\r\n").into_bytes());
    out
}

/// Serve one client until it goes away or the server stops.
pub fn serve(stream: &mut TcpStream, stop: &AtomicBool, sent: &AtomicU64) {
    let write = |stream: &mut TcpStream, bytes: &[u8]| -> bool {
        if stream.write_all(bytes).is_err() {
            return false;
        }
        sent.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        true
    };
    if !write(stream, &[IAC, WILL, GMCP, IAC, WILL, MSSP]) {
        return;
    }
    // Give the client time to answer DO GMCP and DO MSSP.
    thread::sleep(Duration::from_millis(300));
    if !write(stream, &script()) {
        return;
    }
    // Answer the commands the session scenes type: `lamps` (eighty lines to scroll back through,
    // for the live view), `note` (a line with a web address, for the link bar) and `wave` (the
    // lamplighter, whose name the completion scene completes).
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let mut line = Vec::new();
    let mut buf = [0u8; 512];
    #[derive(Clone, Copy, PartialEq)]
    enum Telnet {
        Text,
        Iac,
        Option,
        Sub,
        SubIac,
    }
    let mut telnet = Telnet::Text;
    // Where the player stands: the world can be walked (the map's routes and walking).
    let mut here = START;
    while !stop.load(Ordering::Relaxed) {
        let n = match std::io::Read::read(stream, &mut buf) {
            Ok(0) => return,
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(_) => return,
        };
        for &b in &buf[..n] {
            // Telnet from the client (negotiation, GMCP subnegotiation) is not a command.
            telnet = match (telnet, b) {
                (Telnet::Text, IAC) => Telnet::Iac,
                (Telnet::Text, _) => Telnet::Text,
                (Telnet::Iac, SB) => Telnet::Sub,
                (Telnet::Iac, 251..=254) => Telnet::Option,
                (Telnet::Iac | Telnet::Option, _) => {
                    telnet = Telnet::Text;
                    continue;
                }
                (Telnet::Sub, IAC) => Telnet::SubIac,
                (Telnet::Sub, _) => Telnet::Sub,
                (Telnet::SubIac, SE) => {
                    telnet = Telnet::Text;
                    continue;
                }
                (Telnet::SubIac, _) => Telnet::Sub,
            };
            if telnet != Telnet::Text {
                continue;
            }
            match b {
                b'\n' => {
                    let command = String::from_utf8_lossy(&line).trim().to_string();
                    line.clear();
                    if let Some(reply) = walk(&mut here, &command) {
                        if !write(stream, &reply) {
                            return;
                        }
                    } else if let Some(reply) = answer(&command)
                        && !write(stream, &reply)
                    {
                        return;
                    }
                }
                b'\r' => {}
                _ => line.push(b),
            }
        }
    }
}

/// The crossroads prompt.
pub const PROMPT: &str = "\x1b[90mLantern Crossroads >\x1b[0m";
/// The address in Wren's note (`note`).
pub const NOTE_URL: &str = "https://lanternroad.example.org/map";

/// The marsh wisp's attack (`wisp`), as the C# `session-vitals` capture plays it: the fight in
/// the transcript, then the new vitals and the opponent over GMCP (`Char.Vitals`, `Char.Combat`).
pub const WISP: &str = "\r\nA will-o-wisp drifts out of the reeds, flickering with cold light.\r\n\x1b[1;31mThe marsh wisp attacks you!\x1b[0m\r\nYou swing your lantern pole and catch the wisp a glancing blow.\r\nThe marsh wisp sears your arm with cold fire.\r\n\x1b[32m296/380 hp\x1b[0m  \x1b[36m140/220 mana\x1b[0m  \x1b[33m181/240 mv\x1b[0m  \x1b[31m[wisp: wounded]\x1b[0m\r\n";

/// A move: the room the player arrives in (its block of text, a prompt, then `Room.Info`), or
/// "You can't go that way." `None` when the command is not a direction.
pub fn walk(here: &mut usize, command: &str) -> Option<Vec<u8>> {
    let direction = wandur_core::map::normalize_direction(command)?;
    let (_, name, _, exits) = ROOMS[*here];
    let target = exits
        .iter()
        .filter_map(|e| e.split_once(':'))
        .find(|(d, _)| *d == direction)
        .and_then(|(_, to)| ROOMS.iter().position(|r| r.0 == to));
    let Some(target) = target else {
        return Some(format!("\r\nYou can't go that way.\r\n\x1b[90m{name} >\x1b[0m").into_bytes());
    };
    *here = target;
    let (_, name, _, exits) = ROOMS[target];
    let list: Vec<String> = exits
        .iter()
        .filter_map(|e| e.split_once(':'))
        .map(|(d, _)| format!("\x1b[32m{d}\x1b[0m"))
        .collect();
    let mut out = format!(
        "\r\n\x1b[1;33m{name}\x1b[0m\r\nThe Lantern Road winds on around you.\r\n\r\n\x1b[37mExits:\x1b[0m {}\r\n\x1b[90m{name} >\x1b[0m",
        list.join(", ")
    )
    .into_bytes();
    out.extend(room_info(target));
    Some(out)
}

/// What the world answers to a command, if it knows it.
pub fn answer(command: &str) -> Option<Vec<u8>> {
    if command == "wisp" {
        let mut out = WISP.as_bytes().to_vec();
        out.extend_from_slice(PROMPT.as_bytes());
        out.extend(gmcp(
            "Char.Vitals",
            &serde_json::json!({"hp": 296, "maxhp": 380, "mana": 140, "maxmana": 220, "moves": 181, "maxmoves": 240}),
        ));
        out.extend(gmcp(
            "Char.Combat",
            &serde_json::json!({"enemy": "a marsh wisp", "enemyhp": 42, "enemymaxhp": 100}),
        ));
        return Some(out);
    }
    let mut out = String::from("\r\n");
    match command {
        "lamps" => {
            for i in 1..=80 {
                out.push_str(&format!("The lamps sway along the road, marker {i} of 80.\r\n"));
            }
        }
        "note" => out.push_str(&format!("Wren hands you a note: the full chart is at {NOTE_URL}\r\n")),
        "wave" => out.push_str("\x1b[32mThe lamplighter waves.\x1b[0m\r\nA lamplighter nods to you as he passes.\r\n"),
        // Two clan lines no rule knows yet (the C# `mark-channel-dialog` capture).
        "clan" => out.push_str("[Clan] Bastian: torches at the north gate\r\n[Clan] Wren: bring the spyglass\r\n"),
        // The help scripts' lines (the C# HelpScreenshotTests examples): a guildmate arrives (the
        // Guild greeter waves back), the lantern gutters (Lantern watch counts the oil down) and
        // the answers to what they send.
        "arrive" => out.push_str("Wren arrives from the east.\r\n"),
        "gutter" => out.push_str("Your lantern gutters in the wind.\r\n"),
        "wave Wren" => out.push_str("You wave to Wren. Wren waves back.\r\n"),
        "fill lantern" => out.push_str("You fill your lantern from the oil flask.\r\n"),
        "marsh" => out.push_str("You wade into the dark marsh.\r\n"),
        // Long chat lines a MUD sends unwrapped (the reading scenes: word wrapping).
        "chat" => {
            for line in CHAT {
                out.push_str(line);
                out.push_str("\r\n");
            }
        }
        _ => return None,
    }
    out.push_str(PROMPT);
    Some(out.into_bytes())
}

/// Channel chatter as long single lines, coloured as MUDs colour them (`chat`).
pub const CHAT: &[&str] = &[
    "\x1b[1;36m[gossip]\x1b[0m \x1b[33mTalek\x1b[0m: has anyone restocked with hunted goods at the market yet, or are the traders still waiting on the caravan from the river road to the north gate?",
    "\x1b[1;36m[gossip]\x1b[0m \x1b[33mIlsa\x1b[0m: the caravan came in at dawn, but the hunters kept the best pelts for the clan hall; try the tanner beside the old bridge before the evening bell.",
    "\x1b[1;35m[ooc]\x1b[0m \x1b[33mMorrow\x1b[0m: reminder that the lantern festival starts tonight at the crossroads: bring a lantern, a story, and something warm to drink, everyone is welcome.",
    "\x1b[0;37mThe old road bends east here, its cobbles worn smooth by centuries of carts, and a row of iron lamp posts leans toward the river as if listening for the ferryman.\x1b[0m",
    "\x1b[1;32m[newbie]\x1b[0m \x1b[33mBenn\x1b[0m: how do I get from the crossroads to the harbor without walking through the marsh? the wisps keep draining my lantern oil.",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_exit_leads_to_a_room_and_back() {
        for (id, _, _, exits) in ROOMS {
            for exit in exits {
                let (_, to) = exit.split_once(':').unwrap();
                let (_, _, _, back) = ROOMS.iter().find(|r| r.0 == to).expect(to);
                assert!(
                    back.iter().any(|e| e.ends_with(&format!(":{id}"))),
                    "{to} leads back to {id}"
                );
            }
        }
    }

    /// Every scene renders headless (wgpu, no window) against in-process loopback servers and is
    /// ready before the timeout: the session scene sees the whole Lantern Road.
    #[test]
    #[ignore = "slow (about 3 minutes): run with --include-ignored, or the ship profile"]
    fn every_scene_captures_a_png_headless() {
        let mud = crate::mud_server::MudServer::start_lantern(0).unwrap();
        let (login, _) = crate::mud_server::MudServer::start_login(0, false).unwrap();
        let directory = crate::directory_server::DirectoryServer::start_fixture(0).unwrap();
        // The update-notice scene asks the directory for the newest release.
        directory.set_latest("200 OK", crate::directory_server::latest_json("0.1.6"));
        let root = std::env::temp_dir().join(format!("wandur-scenes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for scene in wandur_app::scene::SCENES {
            let options = wandur_app::Options {
                data_dir: Some(root.join(scene.name)),
                directory_url: Some(directory.address()),
                connect: vec![
                    wandur_core::Endpoint::new("127.0.0.1", mud.port),
                    wandur_core::Endpoint::new("127.0.0.1", login.port),
                ],
                ..Default::default()
            };
            let path = root.join(format!("{}.png", scene.name));
            let warning = wandur_app::scene::capture(scene, options, [1300.0, 820.0], &path).unwrap();
            assert!(warning.is_none(), "{warning:?}");
            let image = image::open(&path).unwrap().to_rgba8();
            assert_eq!(image.dimensions(), (1300, 820), "{}", scene.name);
            let first = image.get_pixel(0, 0);
            assert!(image.pixels().any(|p| p != first), "{} is blank", scene.name);
            // Scenes save nothing in their data directory, not even a database (they make one of
            // their own elsewhere), so it may not exist at all.
            let saved: Vec<String> = std::fs::read_dir(root.join(scene.name))
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            assert!(saved.is_empty(), "{}: {saved:?}", scene.name);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The three help scripts of the C# `HelpScreenshotTests` run against The Lantern Road over
    /// loopback and do what they say: Guild greeter waves back at a guildmate who arrives,
    /// Marsh warning answers `marsh` locally and keeps it from the world, and Lantern watch
    /// counts its oil down when the lantern gutters (its panel and strip get the new value) and
    /// its Refill button sends `fill lantern`.
    #[test]
    fn the_help_scripts_do_what_they_say() {
        use std::time::{Duration, Instant};
        use wandur_app::session_tab::{SessionTab, TabOptions};
        use wandur_core::scripting::panels::PanelEvent;

        let mud = crate::mud_server::MudServer::start_lantern(0).unwrap();
        let endpoint = wandur_core::Endpoint::new("127.0.0.1", mud.port);
        let mut tab = SessionTab::open(1, endpoint, &TabOptions::default(), std::sync::Arc::new(|| {}));
        let mut library = wandur_app::scene::lantern_scripts();
        for script in &mut library {
            script.enabled = true;
        }
        tab.set_scripts(wandur_app::app::script_definitions(&library), Instant::now());
        let pump_until = |tab: &mut SessionTab, what: &str, done: &dyn Fn(&SessionTab) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(15);
            while !done(tab) {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                tab.pump(Instant::now());
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        pump_until(&mut tab, "the crossroads", &|t| {
            t.terminal.transcript().contains("Lantern Crossroads >") && t.scripts.entries.iter().all(|e| e.is_running())
        });
        let gauge = |t: &SessionTab, panel: &str, widget: &str| {
            t.panels
                .panels
                .iter()
                .find(|p| p.id == panel)
                .and_then(|p| p.widget(widget))
                .map(|w| w.props.number)
        };
        assert_eq!(
            gauge(&tab, "lantern-watch", "oil"),
            Some(8.0),
            "the watch drew its oil at 8"
        );
        assert_eq!(gauge(&tab, "lantern-oil", "oil"), Some(8.0));

        // Guild greeter: Wren arrives, the script waves back, the world answers.
        tab.input = "arrive".into();
        tab.submit();
        pump_until(&mut tab, "the wave", &|t| {
            t.terminal.transcript().contains("Wren waves back.")
        });
        assert_eq!(tab.script_commands_sent, 1);

        // Marsh warning: the alias answers locally; the world never hears `marsh`.
        tab.input = "marsh".into();
        tab.submit();
        pump_until(&mut tab, "the warning", &|t| {
            t.terminal
                .transcript()
                .contains("[script] Take a lantern: the marsh is bad tonight.")
        });
        assert_eq!(tab.history().last().map(String::as_str), Some("marsh"));

        // Lantern watch: the lantern gutters, the oil goes to 7 in the panel and the strip.
        tab.input = "gutter".into();
        tab.submit();
        pump_until(&mut tab, "the oil at 7", &|t| {
            gauge(t, "lantern-watch", "oil") == Some(7.0) && gauge(t, "lantern-oil", "oil") == Some(7.0)
        });

        // The Refill button's callback sends `fill lantern`.
        let script = tab.panels.panels[0].script.clone();
        tab.panel_event(&script, "lantern-watch", "refill", PanelEvent::Click, None);
        pump_until(&mut tab, "the refill", &|t| {
            t.terminal.transcript().contains("from the oil flask")
        });
        let transcript = tab.terminal.transcript();
        assert!(
            !transcript.contains("dark marsh"),
            "the alias kept `marsh` from the world"
        );
        assert_eq!(tab.script_commands_sent, 2);
        assert!(tab.scripts.entries.iter().all(|e| e.error.is_none()));
    }

    /// The full Lantern watch example against The Lantern Road over loopback, drawn in a session
    /// view (headless egui): the rail beside the transcript holds "Lantern watch" with its two
    /// gauges, the coloured road label and the Refill button; the vitals strip carries the
    /// script's "Lantern oil" gauge after the mapped vitals. The lantern gutters: both read 7.
    /// A click on Refill sends `fill lantern`, and the world answers.
    #[test]
    fn the_lantern_watch_renders_its_panel_and_strip() {
        use std::time::{Duration, Instant};
        use wandur_app::session_tab::{SessionTab, TabOptions};
        use wandur_app::terminal_view::{self, PaintOptions, TermFonts, TerminalViewState};

        let mud = crate::mud_server::MudServer::start_lantern(0).unwrap();
        let endpoint = wandur_core::Endpoint::new("127.0.0.1", mud.port);
        let options = TabOptions {
            mapping: Some(wandur_app::scene::lantern_mapping(&endpoint)),
            ..TabOptions::default()
        };
        let mut tab = SessionTab::open(1, endpoint, &options, std::sync::Arc::new(|| {}));
        let library: Vec<_> = wandur_app::scene::lantern_scripts()
            .into_iter()
            .filter(|s| s.name == "Lantern watch")
            .map(|mut s| {
                s.enabled = true;
                s
            })
            .collect();
        tab.set_scripts(wandur_app::app::script_definitions(&library), Instant::now());

        let ctx = egui::Context::default();
        wandur_app::fonts::install(&ctx);
        let theme = wandur_app::theme::Theme::preset("Hull");
        let fonts = TermFonts::new(13.0);
        let mut fallback = wandur_app::fonts::FallbackFonts::new();
        let mut view = TerminalViewState::default();
        let mut frame = |tab: &mut SessionTab, view: &mut TerminalViewState, events: Vec<egui::Event>| {
            tab.pump(Instant::now());
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(1000.0, 640.0),
                )),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    terminal_view::show(ui, tab, view, &theme, &fonts, &mut fallback, &PaintOptions::default());
                });
            });
            out.textures_delta.clear();
        };
        let until = |what: &str,
                     tab: &mut SessionTab,
                     view: &mut TerminalViewState,
                     frame: &mut dyn FnMut(&mut SessionTab, &mut TerminalViewState, Vec<egui::Event>),
                     done: &dyn Fn(&SessionTab, &TerminalViewState) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(15);
            while !done(tab, view) {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                frame(tab, view, vec![]);
                std::thread::sleep(Duration::from_millis(5));
            }
            // Once more, so the frame shows what the last pump brought.
            frame(tab, view, vec![]);
        };
        until("the panel", &mut tab, &mut view, &mut frame, &|t, v| {
            t.terminal.transcript().contains("Lantern Crossroads >") && v.rail.drawn.len() == 4 && v.vitals.len() >= 4
        });
        assert_eq!(view.rail.shown, ["Lantern watch"]);
        let drawn: Vec<&str> = view.rail.drawn.iter().map(|(_, w, _)| w.as_str()).collect();
        assert_eq!(drawn, ["oil", "wick", "road", "refill"]);
        let transcript = view.transcript_rect.expect("the transcript is drawn");
        let rail_left = view.rail.headers[0].left();
        assert!(transcript.right() <= rail_left, "the rail sits beside the transcript");
        let labels: Vec<&str> = view.vitals.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Health", "Mana", "Movement", "Lantern oil"]);
        assert_eq!(view.vitals[3].values(), "8 / 10");
        let road = &tab.panels.panels[0].widget("road").unwrap().props;
        assert_eq!(road.text.as_deref(), Some("&YRoad clear to the ford&D"));

        // The lantern gutters: the panel and the strip read 7.
        tab.input = "gutter".into();
        tab.submit();
        until("the oil at 7", &mut tab, &mut view, &mut frame, &|_, v| {
            v.vitals.last().is_some_and(|c| c.values() == "7 / 10")
        });
        let oil = tab.panels.panels[0].widget("oil").unwrap();
        assert_eq!(oil.props.number, 7.0);

        // A click on Refill lantern.
        let refill = view.rail.drawn.iter().find(|(_, w, _)| w == "refill").unwrap().2;
        let at = refill.center();
        frame(
            &mut tab,
            &mut view,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        until("the refill", &mut tab, &mut view, &mut frame, &|t, _| {
            t.terminal.transcript().contains("from the oil flask")
        });
        assert_eq!(tab.script_commands_sent, 1);
        assert!(tab.scripts.entries.iter().all(|e| e.error.is_none()));
    }

    /// The directory listing of The Lantern Road at this loopback port, with a one-script pack
    /// at `version` whose panel label reads `v{version}`.
    fn pack_directory(mud_port: u16, version: i64) -> String {
        let source = format!(
            r#"const lamp = mud.panel("lamp", {{ title: "Lamp", dock: "right" }});
lamp.label("version", {{ text: "v{version}" }});
lamp.button("fill", {{ label: "Fill", onClick: () => mud.send("fill lantern") }});
mud.trigger(/^Your lantern gutters/, () => mud.send("fill lantern"));"#
        );
        serde_json::json!({
            "format": "wandur.directory",
            "schema_version": 2,
            "fetched_at": crate::directory_server::now_rfc3339(),
            "worlds": [{
                "id": "lantern-road", "name": "The Lantern Road", "host": "127.0.0.1", "port": mud_port,
                "summary": "Lamps along the marsh road.", "availability": {"online": true},
                "scripts": [
                    {"id": "lamp-keeper", "name": "Lamp keeper", "description": "Keeps the lantern filled.",
                     "source": source, "provenance": "generated", "version": version},
                    {"id": "community", "name": "Community", "source": "mud.echo('x');",
                     "provenance": "community", "version": 1}
                ]
            }]
        })
        .to_string()
    }

    /// Script packs against the directory bench and The Lantern Road: opening the listed world
    /// installs its pack (enabled, marked, the send policy in force); the panel button may send,
    /// the trigger may not until sending is allowed; a new version in the directory upgrades the
    /// script on the next open and keeps the person's choices.
    #[test]
    fn script_packs_install_upgrade_and_the_send_policy() {
        use std::time::{Duration, Instant};
        use wandur_app::sessions::AppAction;
        use wandur_core::scripting::panels::PanelEvent;

        let mud = crate::mud_server::MudServer::start_lantern(0).unwrap();
        let directory =
            crate::directory_server::DirectoryServer::start(0, pack_directory(mud.port, 1), Vec::new(), Vec::new())
                .unwrap();
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.superpowers/test-data")
            .join(format!("bench-packs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let mut app = wandur_app::WandurApp::new(
            &ctx,
            wandur_app::Options {
                data_dir: Some(dir.clone()),
                directory_url: Some(directory.address()),
                ..Default::default()
            },
        );
        let frame = |app: &mut wandur_app::WandurApp| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(1300.0, 820.0),
                )),
                ..Default::default()
            };
            let mut out = app.run_frame(&ctx, input);
            out.textures_delta.clear();
            std::thread::sleep(Duration::from_millis(5));
        };
        let until = |app: &mut wandur_app::WandurApp, what: &str, done: &dyn Fn(&mut wandur_app::WandurApp) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(20);
            while !done(app) {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                frame(app);
            }
        };
        until(&mut app, "the catalog", &|a| a.catalog_len() > 0);

        // Add the listing to my worlds and connect: the pack is installed before the session
        // takes the library.
        app.push_action(AppAction::SaveListing {
            id: "lantern-road".into(),
            tls: false,
            connect: true,
        });
        let last = |a: &mut wandur_app::WandurApp| a.sessions_mut().iter().last().map(|e| e.tab.id);
        until(&mut app, "the session", &|a| {
            a.sessions_mut().iter().last().is_some_and(|e| {
                e.tab.terminal.transcript().contains("Lantern Crossroads >")
                    && e.tab.panels.panels.iter().any(|p| p.id == "lamp")
            })
        });
        let index = app
            .saved_worlds()
            .iter()
            .position(|w| w.listing_id == "lantern-road")
            .unwrap();
        let library = app.world_library(index);
        let packs: Vec<_> = library.iter().filter(|e| e.is_pack()).collect();
        assert_eq!(packs.len(), 1, "the unsupported provenance is not attached");
        let pack = packs[0].clone();
        assert_eq!(pack.name, "Lamp keeper");
        assert!(pack.enabled && pack.restricted_send());
        let info = pack.pack().unwrap();
        assert_eq!((info.provenance.as_str(), info.version), ("generated", 1));
        assert_eq!(info.description, "Keeps the lantern filled.");
        assert!(
            library.iter().any(|e| !e.is_pack() && !e.is_macro()),
            "the hand-written starter stays"
        );

        // The button may send under the policy.
        let id = last(&mut app).unwrap();
        let tab = &mut app.sessions_mut().get_mut(id).unwrap().tab;
        tab.panel_event(&pack.id, "lamp", "fill", PanelEvent::Click, None);
        until(&mut app, "the fill", &|a| {
            a.sessions_mut()
                .iter()
                .last()
                .is_some_and(|e| e.tab.terminal.transcript().contains("from the oil flask"))
        });

        // The trigger may not: the script stops with the policy's message.
        let tab = &mut app.sessions_mut().get_mut(id).unwrap().tab;
        tab.input = "gutter".into();
        tab.submit();
        until(&mut app, "the refusal", &|a| {
            a.sessions_mut().iter().last().is_some_and(|e| {
                e.tab
                    .scripts
                    .entry(&pack.id)
                    .and_then(|s| s.error.as_deref())
                    .is_some_and(|m| m.contains(wandur_core::l10n::t(wandur_core::l10n::S::ScriptPackSendRefused)))
            })
        });
        assert!(
            app.sessions_mut().iter().last().unwrap().tab.panels.is_empty(),
            "a stopped script keeps no panels"
        );

        // Allow sending (Save world in the editor): the open session runs it again, and the
        // trigger now refills the lantern.
        let mut after = app.world_library(index);
        after.iter_mut().find(|e| e.id == pack.id).unwrap().set_allow_send(true);
        app.save_world_library(index, after);
        until(&mut app, "the restart", &|a| {
            a.sessions_mut()
                .iter()
                .last()
                .is_some_and(|e| e.tab.scripts.entry(&pack.id).is_some_and(|s| s.is_running()))
        });
        let tab = &mut app.sessions_mut().get_mut(id).unwrap().tab;
        tab.input = "gutter".into();
        tab.submit();
        until(&mut app, "the second fill", &|a| {
            a.sessions_mut()
                .iter()
                .last()
                .is_some_and(|e| e.tab.terminal.transcript().matches("from the oil flask").count() == 2)
        });

        // Version 2 in the directory: the next open upgrades the script, keeping the choices.
        directory.set_directory(pack_directory(mud.port, 2));
        app.push_action(AppAction::RefreshDirectory);
        until(&mut app, "the new catalog", &|a| {
            a.listing("lantern-road")
                .is_some_and(|l| l.scripts.first().is_some_and(|s| s.version == 2))
        });
        app.push_action(AppAction::ConnectWorld(index));
        until(&mut app, "the upgraded panel", &|a| {
            a.sessions_mut().iter().count() == 2
                && a.sessions_mut().iter().last().is_some_and(|e| {
                    e.tab
                        .panels
                        .panels
                        .iter()
                        .flat_map(|p| p.widgets.iter())
                        .any(|w| w.props.text.as_deref() == Some("v2"))
                })
        });
        let upgraded = app.world_library(index).into_iter().find(|e| e.id == pack.id).unwrap();
        assert_eq!(upgraded.pack().unwrap().version, 2);
        assert!(upgraded.allow_send() && upgraded.enabled);
        assert_eq!(app.world_library(index).iter().filter(|e| e.is_pack()).count(), 1);
        app.shutdown();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Pump `tab` until `done`, failing after ten seconds.
    fn pump_until(
        tab: &mut wandur_app::session_tab::SessionTab,
        done: impl Fn(&wandur_app::session_tab::SessionTab) -> bool,
    ) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !done(tab) {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out: {:?}",
                tab.map.walk_status()
            );
            tab.pump(std::time::Instant::now());
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_map_walks_a_verified_route_across_the_lantern_road() {
        use wandur_app::session_tab::{SessionTab, TabOptions};
        use wandur_core::map::{TrackingState, WalkStatus, find_route_live};
        let server = crate::mud_server::MudServer::start_lantern(0).unwrap();
        let endpoint = wandur_core::Endpoint::new("127.0.0.1", server.port);
        let mut tab = SessionTab::open(1, endpoint, &TabOptions::default(), std::sync::Arc::new(|| {}));
        pump_until(&mut tab, |t| {
            t.map.tracker().room_count() == ROOMS.len()
                && t.map.tracker().current_id() == Some("s:cross")
                && t.map.tracker().state() == TrackingState::Confirmed
        });
        // The world's coordinates place the rooms as the C# chart does (north up).
        let gate = tab.map.tracker().room("s:gate").unwrap();
        assert_eq!((gate.x, gate.y), (1.0, 1.0));
        assert_eq!(tab.map.tracker().link_count(), 42);
        let route = find_route_live(tab.map.tracker(), "s:cross", "s:cave", false).unwrap();
        assert_eq!(route.commands(), "east → east → east → east → south → west");
        assert_eq!(route.cost, 6.0);
        tab.walk_route(&route);
        assert!(tab.map.is_walking());
        assert_eq!(tab.map.walk_status(), WalkStatus::Progress { step: 1, of: 6 });
        pump_until(&mut tab, |t| !t.map.is_walking());
        assert_eq!(tab.map.walk_status(), WalkStatus::Complete);
        assert_eq!(tab.map.tracker().current_id(), Some("s:cave"));
        assert!(tab.terminal.transcript().contains("Hidden Grotto"));
        // A way that does not exist: the world says so and the map stays put.
        tab.input = "north".into();
        tab.submit();
        pump_until(&mut tab, |t| t.terminal.transcript().contains("You can't go that way."));
        assert_eq!(tab.map.tracker().current_id(), Some("s:cave"));
        // And back by hand, each move confirmed by the world's room id.
        tab.input = "east".into();
        tab.submit();
        pump_until(&mut tab, |t| t.map.tracker().current_id() == Some("s:falls"));
    }

    #[test]
    fn the_world_answers_lamps_and_note() {
        let lamps = String::from_utf8(answer("lamps").unwrap()).unwrap();
        assert_eq!(lamps.matches("The lamps sway").count(), 80);
        assert!(lamps.ends_with(PROMPT));
        let note = String::from_utf8(answer("note").unwrap()).unwrap();
        assert!(note.contains(NOTE_URL));
        assert!(
            String::from_utf8(answer("wave").unwrap())
                .unwrap()
                .contains("lamplighter")
        );
        assert!(answer("dance").is_none());
        let wisp = String::from_utf8_lossy(&answer("wisp").unwrap()).into_owned();
        assert!(wisp.contains("[wisp: wounded]") && wisp.contains("Char.Combat"));
    }

    #[test]
    fn the_script_ends_with_the_crossroads_prompt() {
        let s = script();
        let text = String::from_utf8_lossy(&s);
        assert!(text.trim_end().ends_with("Lantern Crossroads >\x1b[0m"));
        assert!(text.contains("Char.Status"));
    }
}
