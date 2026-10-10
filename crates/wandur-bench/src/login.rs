//! Starfall Reach's login screen, a loopback world that asks for a name and a password (the
//! C# `session-private` reference capture), for auto-login tests and the login scenes:
//!
//! 1. The station banner, then `By what name are you known? `.
//! 2. Any name: `Welcome back, NAME.` and `Password: `.
//! 3. The fixture password lets the player in (`Docking clamps release.` and a `> ` prompt);
//!    anything else gets `Wrong password.` and the password prompt again.
//!
//! With GMCP login on (`--page login-gmcp`) it first offers GMCP `Char.Login` password
//! credentials: the right ones are accepted with `Char.Login.Result {"success":true}` and no text
//! login follows; wrong ones get `{"success":false}` and the text login; `{}` (declined) gets the
//! text login. Everything the client sends is recorded for tests ([`Received`]).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

const IAC: u8 = 255;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;
const GMCP: u8 = 201;

/// The password the fixture accepts (fixture only, nothing real).
pub const PASSWORD: &str = "starfall-test-only";

pub const BANNER: &str = "\x1b[1;33m  S T A R F A L L   R E A C H\x1b[0m\r\n\
The beacon at the edge of the settled systems.\r\n\
\r\n\
\x1b[36mStation news\x1b[0m\r\n\
\x20 * The survey skiff Kestrel is back at the eastern berth.\r\n\
\x20 * Docking fees at Beacon Anchorage are waived this week.\r\n\
\r\n\
Players online: 38. Ships in dock: 12.\r\n\
\r\n";

pub const NAME_PROMPT: &str = "By what name are you known? ";

/// What one connection sent: text lines and GMCP messages, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Received {
    pub lines: Vec<String>,
    pub gmcp: Vec<String>,
}

/// Every connection's [`Received`], shared with the test that started the server.
pub type Log = Arc<Mutex<Vec<Received>>>;

/// Telnet input split into lines and GMCP messages (other negotiation is dropped).
#[derive(Default)]
struct Input {
    line: Vec<u8>,
    sub: Option<Vec<u8>>,
    iac: bool,
    /// The byte after IAC was WILL/WONT/DO/DONT: skip the option byte.
    skip_option: bool,
}

enum Item {
    Line(String),
    Gmcp(String),
}

impl Input {
    fn feed(&mut self, bytes: &[u8], out: &mut Vec<Item>) {
        for &b in bytes {
            if self.skip_option {
                self.skip_option = false;
                continue;
            }
            if self.iac {
                self.iac = false;
                match b {
                    IAC => self.byte(b, out),
                    SB => self.sub = Some(Vec::new()),
                    SE => {
                        if let Some(sub) = self.sub.take()
                            && sub.first() == Some(&GMCP)
                        {
                            out.push(Item::Gmcp(String::from_utf8_lossy(&sub[1..]).into_owned()));
                        }
                    }
                    251..=254 => self.skip_option = true,
                    _ => {}
                }
                continue;
            }
            if b == IAC {
                self.iac = true;
                continue;
            }
            self.byte(b, out);
        }
    }

    fn byte(&mut self, b: u8, out: &mut Vec<Item>) {
        if let Some(sub) = &mut self.sub {
            sub.push(b);
            return;
        }
        match b {
            b'\n' => {
                let line = String::from_utf8_lossy(&self.line).trim_end_matches('\r').to_string();
                self.line.clear();
                out.push(Item::Line(line));
            }
            _ => self.line.push(b),
        }
    }
}

fn gmcp(message: &str) -> Vec<u8> {
    let mut out = vec![IAC, SB, GMCP];
    out.extend_from_slice(message.as_bytes());
    out.extend_from_slice(&[IAC, SE]);
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Waiting for the client's GMCP hello before the offer.
    Offer,
    /// The offer is out; waiting for credentials.
    Credentials,
    Name,
    Password,
    In,
}

/// Serve one client until it goes away or the server stops.
pub fn serve(stream: &mut TcpStream, gmcp_login: bool, log: &Log, stop: &AtomicBool, sent: &AtomicU64) {
    let index = {
        let mut log = log.lock().unwrap_or_else(PoisonError::into_inner);
        log.push(Received::default());
        log.len() - 1
    };
    let record = |f: &dyn Fn(&mut Received)| {
        if let Some(r) = log.lock().unwrap_or_else(PoisonError::into_inner).get_mut(index) {
            f(r);
        }
    };
    let Ok(mut reader) = stream.try_clone() else { return };
    let _ = reader.set_read_timeout(Some(Duration::from_millis(200)));
    let mut write = |bytes: &[u8]| -> bool {
        if stream.write_all(bytes).is_err() {
            return false;
        }
        sent.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        true
    };
    let mut stage = if gmcp_login {
        if !write(&[IAC, WILL, GMCP]) {
            return;
        }
        Stage::Offer
    } else {
        if !write(format!("{BANNER}{NAME_PROMPT}").as_bytes()) {
            return;
        }
        Stage::Name
    };
    let mut name = String::new();
    let mut input = Input::default();
    let mut items = Vec::new();
    let mut buf = [0u8; 1024];
    while !stop.load(Ordering::Relaxed) {
        let n = match reader.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => n,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            Err(_) => return,
        };
        input.feed(&buf[..n], &mut items);
        for item in items.drain(..) {
            let reply = match item {
                Item::Gmcp(message) => {
                    record(&|r| r.gmcp.push(message.clone()));
                    match stage {
                        Stage::Offer if message.starts_with("Core.Supports.Set") => {
                            stage = Stage::Credentials;
                            gmcp(r#"Char.Login.Default {"type":["password-credentials"]}"#)
                        }
                        Stage::Credentials if message.starts_with("Char.Login.Credentials") => {
                            let body: serde_json::Value = message
                                .split_once(' ')
                                .and_then(|(_, b)| serde_json::from_str(b).ok())
                                .unwrap_or_default();
                            let account = body["account"].as_str().unwrap_or("");
                            if !account.is_empty() && body["password"].as_str() == Some(PASSWORD) {
                                stage = Stage::In;
                                let mut out = gmcp(r#"Char.Login.Result {"success":true}"#);
                                out.extend(
                                    format!("{BANNER}Welcome aboard, {account}. Docking clamps release.\r\n> ").bytes(),
                                );
                                out
                            } else {
                                stage = Stage::Name;
                                let mut out = if body.as_object().is_some_and(|o| o.is_empty()) {
                                    Vec::new()
                                } else {
                                    gmcp(r#"Char.Login.Result {"success":false}"#)
                                };
                                out.extend(format!("{BANNER}{NAME_PROMPT}").bytes());
                                out
                            }
                        }
                        _ => Vec::new(),
                    }
                }
                Item::Line(line) => {
                    record(&|r| r.lines.push(line.clone()));
                    match stage {
                        Stage::Name => {
                            name = line.trim().to_string();
                            stage = Stage::Password;
                            format!("Welcome back, {name}.\r\nPassword: ").into_bytes()
                        }
                        Stage::Password if line == PASSWORD => {
                            stage = Stage::In;
                            format!("\r\nWelcome aboard, {name}. Docking clamps release.\r\n> ").into_bytes()
                        }
                        Stage::Password => b"Wrong password.\r\nPassword: ".to_vec(),
                        Stage::In => b"> ".to_vec(),
                        _ => Vec::new(),
                    }
                }
            };
            if !reply.is_empty() && !write(&reply) {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Instant;
    use wandur_core::login::{MemoryVault, PasswordVault};
    use wandur_core::settings::SavedWorld;

    const WORLD_ID: &str = "5f1a2b3c4d5e6f708192a3b4c5d6e7f8";

    fn starfall(port: u16, auto_login: bool) -> SavedWorld {
        SavedWorld {
            world_id: WORLD_ID.into(),
            name: "Starfall Reach".into(),
            host: "127.0.0.1".into(),
            port,
            username: "lantern-demo".into(),
            password_id: Some("0f8fad5b-d9cb-469f-a165-70867728950e".into()),
            auto_login,
            ..SavedWorld::default()
        }
    }

    /// Run the whole app headless against `world`, frame by frame, until `done` or 10 s.
    fn run_app(
        world: SavedWorld,
        vault: Arc<MemoryVault>,
        mut done: impl FnMut(&wandur_app::WandurApp) -> bool,
    ) -> (wandur_app::WandurApp, egui::Context) {
        let ctx = egui::Context::default();
        let dir = test_dir(world.port);
        // A throwaway directory (gitignored); the scene-like run writes only its database.
        let _ = std::fs::remove_dir_all(&dir);
        let options = wandur_app::Options {
            data_dir: Some(dir),
            language: Some(wandur_core::l10n::Language::En),
            ephemeral: true,
            connect: vec![world.endpoint()],
            worlds: Some(vec![world]),
            vault: Some(vault as Arc<dyn PasswordVault>),
            directory_url: Some("http://127.0.0.1:9".into()),
            ..Default::default()
        };
        let mut app = wandur_app::WandurApp::new(&ctx, options);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
            ..Default::default()
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(&app) {
            assert!(Instant::now() < deadline, "timed out");
            app.run_frame(&ctx, input.clone()).textures_delta.clear();
            std::thread::sleep(Duration::from_millis(10));
        }
        (app, ctx)
    }

    fn test_dir(port: u16) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.superpowers/test-data")
            .join(format!("bench-login-{port}"))
    }

    fn received(log: &Log) -> Received {
        log.lock().unwrap().last().cloned().unwrap_or_default()
    }

    /// The whole app against the loopback login world: auto-login sends the name and the
    /// password once each, neither is echoed or recorded, and the player gets in.
    #[test]
    fn auto_login_sends_each_credential_once_through_the_app() {
        let (server, log) = crate::mud_server::MudServer::start_login(0, false).unwrap();
        let world = starfall(server.port, true);
        let vault = Arc::new(MemoryVault::new());
        vault
            .write(&wandur_core::login::vault::key(&world).unwrap(), PASSWORD)
            .unwrap();
        let (mut app, _ctx) = run_app(world, vault, |app| {
            app.scene_probe().session_text.contains("Docking clamps")
        });
        let got = received(&log);
        assert_eq!(got.lines, ["lantern-demo", PASSWORD]);
        let text = app.scene_probe().session_text;
        assert!(text.contains("Welcome back, lantern-demo."), "{text}");
        assert!(!text.contains(PASSWORD));
        assert!(!text.contains("known? lantern-demo"), "the name is not echoed");
        assert_eq!(app.scene_probe().history, 0, "nothing in the command history");
        app.shutdown();
        let _ = std::fs::remove_dir_all(test_dir(server.port));
    }

    /// GMCP login: the credentials go once over GMCP, the server accepts them, and no text
    /// login is attempted.
    #[test]
    fn gmcp_login_through_the_app() {
        let (server, log) = crate::mud_server::MudServer::start_login(0, true).unwrap();
        let world = starfall(server.port, true);
        let vault = Arc::new(MemoryVault::new());
        vault
            .write(&wandur_core::login::vault::key(&world).unwrap(), PASSWORD)
            .unwrap();
        let (mut app, _ctx) = run_app(world, vault, |app| {
            app.scene_probe().session_text.contains("Docking clamps")
        });
        let got = received(&log);
        assert!(got.lines.is_empty(), "{:?}", got.lines);
        let credentials: Vec<&String> = got
            .gmcp
            .iter()
            .filter(|m| m.starts_with("Char.Login.Credentials"))
            .collect();
        assert_eq!(credentials.len(), 1);
        assert!(credentials[0].contains("\"account\":\"lantern-demo\""));
        assert!(!app.scene_probe().session_text.contains(PASSWORD));
        app.shutdown();
        let _ = std::fs::remove_dir_all(test_dir(server.port));
    }

    /// A rejected GMCP login stops: the server's text login screen gets nothing automatic.
    #[test]
    fn a_rejected_gmcp_login_is_not_retried_as_text() {
        let (server, log) = crate::mud_server::MudServer::start_login(0, true).unwrap();
        let world = starfall(server.port, true);
        let vault = Arc::new(MemoryVault::new());
        vault
            .write(&wandur_core::login::vault::key(&world).unwrap(), "not-the-password")
            .unwrap();
        let (mut app, ctx) = run_app(world, vault, |app| app.scene_probe().session_text.contains("known?"));
        let until = Instant::now() + Duration::from_millis(400);
        while Instant::now() < until {
            app.run_frame(&ctx, egui::RawInput::default()).textures_delta.clear();
            std::thread::sleep(Duration::from_millis(10));
        }
        let got = received(&log);
        assert!(got.lines.is_empty(), "no text retry: {:?}", got.lines);
        assert!(
            app.scene_probe()
                .session_text
                .contains(wandur_core::l10n::t(wandur_core::l10n::S::GmcpLoginRejected))
        );
        app.shutdown();
        let _ = std::fs::remove_dir_all(test_dir(server.port));
    }
}
