//! Dustfall Station, a loopback MSDP world (`--page msdp`) shaped like the world in the C#
//! `MappedVitalsLiveTests`: it offers MSDP, answers `LIST REPORTABLE_VARIABLES` with its
//! variables, answers `REPORT` and `SEND` with one variable per subnegotiation, and afterwards
//! sends a reported variable only when it changes. Fights come and go one variable at a time:
//!
//! - `kill NAME`: the opponent's name first, then its maximum and health (100).
//! - `ambush NAME`: health and maximum before the name, the order most MSDP tables use.
//! - `hit`: the opponent drops to 30, the character to 980 health and 1018 movement.
//! - `won`: all three opponent variables cleared; `flee`: only the name.
//! - `rest`: the character back to full.
//!
//! What the client asked for is recorded for tests ([`Requests`]).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use wandur_core::protocol::msdp::{self, MsdpValue};

const IAC: u8 = 255;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;
const MSDP: u8 = 69;

pub const BANNER: &str = "\x1b[1;33mD U S T F A L L   S T A T I O N\x1b[0m\r\nA relay at the edge of the drift. Type kill, ambush, hit, won, flee or rest.\r\n\r\n";
pub const PROMPT: &str = "\x1b[32m[Dustfall Station]\x1b[0m > ";

/// The world's variables and their starting values.
pub const VARIABLES: [(&str, &str); 13] = [
    ("CHARACTERNAME", "Tester"),
    ("CLAN", "None"),
    ("HEALTH", "1000"),
    ("HEALTHMAX", "1000"),
    ("MOVEMENT", "1037"),
    ("MOVEMENTMAX", "1040"),
    ("OPPONENTNAME", ""),
    ("OPPONENTHEALTH", "0"),
    ("OPPONENTHEALTHMAX", "0"),
    ("LEVELCOMBAT", "12"),
    ("LEVELPILOTING", "3"),
    ("LEVELSLICER", "0"),
    ("WORLDTIME", "1400"),
];

/// Every MSDP command the clients sent, as text (`LIST REPORTABLE_VARIABLES`, `REPORT A B`).
pub type Requests = Arc<Mutex<Vec<String>>>;

struct World {
    values: Vec<(String, String)>,
    reported: std::collections::HashSet<String>,
}

impl World {
    fn new() -> Self {
        Self {
            values: VARIABLES.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(),
            reported: Default::default(),
        }
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.values.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// One variable per subnegotiation.
    fn frame(name: &str, value: &str) -> Vec<u8> {
        let mut out = Vec::new();
        wandur_core::telnet::subnegotiation(
            MSDP,
            &msdp::encode(&[(name.to_string(), MsdpValue::Text(value.to_string()))]),
            &mut out,
        );
        out
    }

    /// A reported variable changed: tell the client (only when it really changed).
    fn change(&mut self, name: &str, value: &str, out: &mut Vec<u8>) {
        let Some(slot) = self.values.iter_mut().find(|(n, _)| n == name) else {
            return;
        };
        if slot.1 == value {
            return;
        }
        slot.1 = value.to_string();
        if self.reported.contains(name) {
            out.extend(Self::frame(name, value));
        }
    }

    fn request(&mut self, payload: &[u8], requests: &Requests, out: &mut Vec<u8>) {
        let Some(table) = msdp::parse(payload) else { return };
        for (command, value) in table {
            let names: Vec<String> = match &value {
                MsdpValue::Text(t) => vec![t.clone()],
                MsdpValue::Array(items) => items.iter().filter_map(|v| v.as_text().map(String::from)).collect(),
                MsdpValue::Table(_) => Vec::new(),
            };
            requests
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("{command} {}", names.join(" ")));
            match command.as_str() {
                "LIST" if names.iter().any(|n| n == "REPORTABLE_VARIABLES") => {
                    let list = MsdpValue::Array(self.values.iter().map(|(n, _)| MsdpValue::Text(n.clone())).collect());
                    wandur_core::telnet::subnegotiation(
                        MSDP,
                        &msdp::encode(&[("REPORTABLE_VARIABLES".to_string(), list)]),
                        out,
                    );
                }
                "REPORT" | "SEND" => {
                    for name in names {
                        if let Some(value) = self.get(&name).map(String::from) {
                            if command == "REPORT" {
                                self.reported.insert(name.clone());
                            }
                            out.extend(Self::frame(&name, &value));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn command(&mut self, line: &str, out: &mut Vec<u8>) {
        let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
        let text = match verb {
            "kill" | "ambush" if !rest.is_empty() => {
                if verb == "kill" {
                    self.change("OPPONENTNAME", rest, out);
                    self.change("OPPONENTHEALTHMAX", "100", out);
                    self.change("OPPONENTHEALTH", "100", out);
                } else {
                    self.change("OPPONENTHEALTH", "100", out);
                    self.change("OPPONENTHEALTHMAX", "100", out);
                    self.change("OPPONENTNAME", rest, out);
                }
                format!("You attack {rest}!\r\n- Enemy: [ 100% ] -\r\n")
            }
            "hit" => {
                self.change("OPPONENTHEALTH", "30", out);
                self.change("HEALTH", "980", out);
                self.change("MOVEMENT", "1018", out);
                "You hit hard.\r\n- Enemy: [ 30% ] -\r\n".to_string()
            }
            "won" => {
                self.change("OPPONENTHEALTH", "0", out);
                self.change("OPPONENTHEALTHMAX", "0", out);
                self.change("OPPONENTNAME", "", out);
                "Your opponent is dead.\r\n".to_string()
            }
            "flee" => {
                self.change("OPPONENTNAME", "", out);
                "You flee.\r\n".to_string()
            }
            "rest" => {
                self.change("HEALTH", "1000", out);
                self.change("MOVEMENT", "1037", out);
                "You rest.\r\n".to_string()
            }
            "" => String::new(),
            _ => "Huh?\r\n".to_string(),
        };
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(PROMPT.as_bytes());
    }
}

/// Serve one client until it goes away or the server stops.
pub fn serve(stream: &mut TcpStream, requests: &Requests, stop: &AtomicBool, sent: &AtomicU64) {
    let write = |stream: &mut TcpStream, bytes: &[u8]| -> bool {
        if bytes.is_empty() {
            return true;
        }
        if stream.write_all(bytes).is_err() {
            return false;
        }
        sent.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        true
    };
    let mut greeting = vec![IAC, WILL, MSDP];
    greeting.extend_from_slice(BANNER.as_bytes());
    greeting.extend_from_slice(PROMPT.as_bytes());
    if !write(stream, &greeting) {
        return;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let mut world = World::new();
    let mut buf = [0u8; 4096];
    let mut line = Vec::new();
    let mut sub: Option<Vec<u8>> = None;
    let (mut iac, mut skip) = (false, false);
    while !stop.load(Ordering::Relaxed) {
        let n = match stream.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(_) => return,
        };
        let mut out = Vec::new();
        for &b in &buf[..n] {
            if skip {
                skip = false;
                continue;
            }
            if iac {
                iac = false;
                match b {
                    SB => sub = Some(Vec::new()),
                    SE => {
                        if let Some(payload) = sub.take()
                            && payload.first() == Some(&MSDP)
                        {
                            world.request(&payload[1..], requests, &mut out);
                        }
                    }
                    251..=254 => skip = true,
                    IAC => match &mut sub {
                        Some(s) => s.push(IAC),
                        None => line.push(IAC),
                    },
                    _ => {}
                }
                continue;
            }
            if b == IAC {
                iac = true;
                continue;
            }
            if let Some(s) = &mut sub {
                s.push(b);
                continue;
            }
            match b {
                b'\n' => {
                    let command = String::from_utf8_lossy(&line).trim().to_string();
                    line.clear();
                    world.command(&command, &mut out);
                }
                b'\r' => {}
                _ => line.push(b),
            }
        }
        if !write(stream, &out) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Instant, SystemTime};
    use wandur_core::diagnostics::{Gate, SessionProtocol};
    use wandur_core::protocol::mapping::{FieldBinding, FieldReference, MappingEndpoint, MappingTarget, WorldMapping};
    use wandur_core::session::{Drained, Session, SessionConfig, SessionEvent};

    fn bind(variable: &str, entity: &str, category: &str, key: &str, member: &str, conversion: &str) -> FieldBinding {
        FieldBinding {
            source: FieldReference {
                protocol: "MSDP".into(),
                package: "MSDP".into(),
                path: format!("/{variable}"),
            },
            target: MappingTarget {
                entity: entity.into(),
                category: category.into(),
                key: key.into(),
                member: member.into(),
            },
            label: key.into(),
            conversion: conversion.into(),
            scale: 1.0,
        }
    }

    /// The world's mapping, as in `MappedVitalsLiveTests.Mapping`.
    fn mapping(port: u16) -> WorldMapping {
        WorldMapping {
            schema_version: 1,
            world_id: "dustfall".into(),
            endpoint: MappingEndpoint {
                host: "127.0.0.1".into(),
                port: i64::from(port),
                use_tls: false,
            },
            schema_fingerprint: "a".repeat(64),
            revision: 1,
            generated_at: "2026-10-08T12:00:00Z".into(),
            provenance: "deterministic".into(),
            provisional: true,
            bindings: vec![
                bind("HEALTH", "character", "resource", "health", "current", "number"),
                bind("HEALTHMAX", "character", "resource", "health", "maximum", "number"),
                bind("MOVEMENT", "character", "resource", "movement", "current", "number"),
                bind("MOVEMENTMAX", "character", "resource", "movement", "maximum", "number"),
                bind("OPPONENTHEALTH", "opponent", "resource", "health", "current", "number"),
                bind(
                    "OPPONENTHEALTHMAX",
                    "opponent",
                    "resource",
                    "health",
                    "maximum",
                    "number",
                ),
                bind("OPPONENTNAME", "opponent", "identity", "name", "value", "text"),
                bind("CHARACTERNAME", "character", "identity", "name", "value", "text"),
                bind("CLAN", "character", "identity", "faction", "value", "text"),
                bind("LEVELCOMBAT", "character", "progression", "combat", "current", "number"),
                bind("WORLDTIME", "world", "metric", "time", "value", "number"),
            ],
        }
    }

    struct Client {
        session: Session,
        protocol: SessionProtocol,
        drained: Drained,
        text: String,
    }

    impl Client {
        fn pump(&mut self) {
            self.session.drain(&mut self.drained);
            let gate = Gate::default();
            for (_, event) in &self.drained.events {
                match event {
                    SessionEvent::Connected { .. } => self.protocol.connected(),
                    SessionEvent::MsdpEnabled => self.protocol.msdp_enabled = true,
                    SessionEvent::Msdp(payload) => self.protocol.receive_msdp(payload, gate, SystemTime::now()),
                    _ => {}
                }
            }
            self.protocol.receive_text(&self.drained.text, gate);
            self.text.push_str(&self.drained.text);
        }

        fn cards(&self) -> Vec<String> {
            wandur_app::vitals_view::cards(self.protocol.bindings(), true)
                .iter()
                .map(|c| c.tip())
                .collect()
        }

        fn wait_for(&mut self, what: &str, done: impl Fn(&Client) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !done(self) {
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for {what}; cards {:?}",
                    self.cards()
                );
                std::thread::sleep(Duration::from_millis(5));
                self.pump();
            }
        }

        fn send(&mut self, line: &str) {
            assert!(self.session.send_line(line));
        }
    }

    /// The loopback version of `ListedWorldVitalsKeepFollowingTheWorldThroughFightsAndCatalogRefreshes`
    /// and `TheOpponentCardShowsInTheSecondFight`: the client lists and reports every variable on
    /// its own, the strip shows the vitals, and the opponent card follows two fights, the second
    /// against the same maximum with its health before its name and after a fight that cleared
    /// only the name.
    #[test]
    fn the_vitals_strip_follows_two_fights_one_msdp_variable_at_a_time() {
        let (server, requests) = crate::mud_server::MudServer::start_msdp(0).unwrap();
        let config = SessionConfig::new(wandur_core::Endpoint::new("127.0.0.1", server.port));
        let mut client = Client {
            session: Session::connect(config, std::sync::Arc::new(|| {})),
            protocol: SessionProtocol::new(Some(mapping(server.port))),
            drained: Drained::default(),
            text: String::new(),
        };
        let base = ["Health: 1000 / 1000", "Movement: 1037 / 1040"];
        client.wait_for("the vitals", |c| c.cards() == base);
        let engine = client.protocol.bindings().unwrap();
        assert_eq!(client.protocol.reported_character().as_deref(), Some("Tester"));
        assert_eq!(engine.world().metrics["time"].number.as_ref().unwrap().value, 1400.0);
        assert_eq!(
            engine.character().progression["combat"].current.as_ref().unwrap().value,
            12.0
        );
        {
            let requests = requests.lock().unwrap();
            assert_eq!(requests[0], "LIST REPORTABLE_VARIABLES");
            assert!(requests[1].starts_with("REPORT CHARACTERNAME CLAN HEALTH"));
        }
        client.send("kill A Vicious Womprat");
        client.wait_for("the first fight", |c| c.cards().len() == 3);
        assert_eq!(client.cards()[2], "Opponent · Health: 100 / 100");
        client.send("hit");
        client.wait_for("the round", |c| {
            c.cards()
                == [
                    "Health: 980 / 1000",
                    "Movement: 1018 / 1040",
                    "Opponent · Health: 30 / 100",
                ]
        });
        client.send("won");
        client.wait_for("the end of the fight", |c| c.cards().len() == 2);
        // Fight two: health and maximum before the name.
        client.send("ambush A Stormtrooper");
        client.wait_for("the second fight", |c| c.cards().len() == 3);
        assert_eq!(client.cards()[2], "Opponent · Health: 100 / 100");
        assert_eq!(
            client.protocol.bindings().unwrap().opponent().identity["name"].value,
            "A Stormtrooper"
        );
        // A fight that ends with only the name cleared, then the same opponent again: the world
        // repeats neither the maximum nor the unchanged health, and the card still comes back.
        client.send("flee");
        client.wait_for("the flight", |c| c.cards().len() == 2);
        client.send("ambush A Stormtrooper");
        client.wait_for("the third fight", |c| c.cards().len() == 3);
        assert_eq!(client.cards()[2], "Opponent · Health: 100 / 100");
        // Diagnostics saw every variable as its own message, and the console the transcript.
        assert!(
            client
                .protocol
                .messages
                .kinds()
                .iter()
                .any(|k| k.name == "OPPONENTHEALTH")
        );
        assert!(client.protocol.console.render(0).contains("You attack A Stormtrooper!"));
    }
}
