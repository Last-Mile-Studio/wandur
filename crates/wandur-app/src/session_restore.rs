//! Reopening the sessions open when Wandur last quit (Settings > General, "When Wandur starts").
//!
//! `sessions.json` in the data directory lists the session tabs in their order: each one's world
//! (its saved world's id, else its address), a name given with Rename, the character the world
//! reported, the page shown (Play, Map or Diagnostics, side by side or not), whether it was
//! connected, and which tab was shown. Never a password or anything else secret: a reopened
//! session that connects reads the vault for auto-login as any connection does, and only when
//! its world has auto-login on. The demo is not kept. The file is written with the layout (when
//! it changed, every two seconds at most) and on quit.
//!
//! The same file is the crash marker: `running` is true from start until a clean quit writes it
//! false (eframe's `on_exit`: the window closed, File > Exit, Cmd+Q, and on macOS the system
//! logging out, which quits the app through `applicationWillTerminate`). Found true at start,
//! the last run ended another way (a crash, a kill, the power lost, or on Windows and Linux a
//! logout that ended the process): the tabs come back but nothing connects, and a strip offers
//! to reconnect them. Shift held at start does the same with its own wording.
//!
//! Shift: on macOS `main` asks the system before the window exists
//! ([`crate::platform::mac_keys`]). Elsewhere egui knows the modifiers only once the window has
//! the focus and the system has told winit, so the sessions to reconnect wait [`SHIFT_WINDOW`]
//! after the first frame ([`ShiftWatch`]) and any frame with Shift down until then skips them. A
//! Shift let go before the window shows, or a window that opens without the focus, is missed
//! there: the sessions connect.
//!
//! Scenes (ephemeral runs) never read or write the file; a scene can hand the app one of its own.

use std::path::Path;
use std::time::{Duration, Instant};

use egui::{RichText, Ui};
use serde::{Deserialize, Deserializer, Serialize};
use wandur_core::Endpoint;
use wandur_core::db::worlds::valid_world_id;
use wandur_core::l10n::{S, t};
use wandur_core::settings::{SavedWorld, StartupSessions};

use crate::session_tab::{SessionId, Status};
use crate::sessions::SessionEntry;
use crate::terminal_view::Page;
use crate::theme::Theme;

pub const SESSIONS_FILE: &str = "sessions.json";
pub const SESSIONS_VERSION: u32 = 1;
/// Where the system cannot be asked (not macOS), how long after the first frame reopened
/// sessions wait for a held Shift before they connect.
pub const SHIFT_WINDOW: Duration = Duration::from_millis(400);

/// What `sessions.json` holds.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionsFile {
    pub version: u32,
    /// True from start until a clean quit: found true at start, the last run did not end
    /// cleanly.
    pub running: bool,
    /// The tab shown, by its place in `tabs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<usize>,
    pub tabs: Vec<SavedSession>,
}

/// One session tab.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedSession {
    /// The saved world it was opened from, by id; empty for a session opened by address.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub world_id: String,
    /// Its address (`host:port`, `tls://host:port`). A saved world's session comes back at the
    /// world's address as it is now.
    pub address: String,
    /// A name given with Rename.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The character the world reported, when it is not the saved username.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub character: String,
    #[serde(deserialize_with = "lenient_view")]
    pub view: SavedView,
    /// Play and Map side by side.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub split: bool,
    /// Connected, connecting or waiting to reconnect at quit (not disconnected).
    pub connected: bool,
}

/// The page a session showed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedView {
    #[default]
    Play,
    Map,
    Diagnostics,
}

/// A page this build does not know is Play.
fn lenient_view<'de, D: Deserializer<'de>>(d: D) -> Result<SavedView, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(d)?;
    Ok(value.and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default())
}

/// The file in `dir`, if there is a usable one. One that does not parse is moved aside
/// (`sessions.json.bad`) and the start is a fresh one; a newer version's file is left alone.
pub fn load(dir: &Path) -> Option<SessionsFile> {
    let path = dir.join(SESSIONS_FILE);
    let text = std::fs::read_to_string(&path).ok()?;
    match serde_json::from_str::<SessionsFile>(&text) {
        Ok(file) if file.version <= SESSIONS_VERSION => Some(file),
        Ok(_) => None,
        Err(_) => {
            let _ = std::fs::rename(&path, dir.join(format!("{SESSIONS_FILE}.bad")));
            None
        }
    }
}

pub fn to_json(file: &SessionsFile) -> String {
    serde_json::to_string_pretty(file).unwrap_or_default()
}

/// Describe the open sessions, in the order given (the session tabs'), the demo left out.
pub fn capture<'a>(
    entries: impl Iterator<Item = &'a SessionEntry>,
    active: Option<SessionId>,
    worlds: &[SavedWorld],
    running: bool,
) -> SessionsFile {
    let mut file = SessionsFile {
        version: SESSIONS_VERSION,
        running,
        ..Default::default()
    };
    for entry in entries.filter(|e| !e.tab.is_demo()) {
        let tab = &entry.tab;
        let world = tab
            .world
            .and_then(|i| worlds.get(i))
            .filter(|w| valid_world_id(&w.world_id));
        let username = world.map_or("", |w| w.username.trim());
        if active == Some(tab.id) {
            file.active = Some(file.tabs.len());
        }
        file.tabs.push(SavedSession {
            world_id: world.map(|w| w.world_id.clone()).unwrap_or_default(),
            address: tab.endpoint.to_string(),
            name: tab.custom_name.clone(),
            character: if tab.character() == username {
                String::new()
            } else {
                tab.character().to_string()
            },
            view: match entry.view.page {
                Page::Play => SavedView::Play,
                Page::Map => SavedView::Map,
                Page::Diagnostics => SavedView::Diagnostics,
            },
            split: entry.view.split && entry.view.page != Page::Diagnostics,
            connected: matches!(
                tab.status,
                Status::Connected { .. } | Status::Connecting | Status::Waiting { .. }
            ),
        });
    }
    file
}

/// A session to reopen.
#[derive(Clone, Debug, PartialEq)]
pub struct Reopen {
    pub endpoint: Endpoint,
    /// The saved world, by index.
    pub world: Option<usize>,
    pub saved: SavedSession,
    /// It was connected at quit and the setting is Reopen and reconnect: it connects, unless
    /// the last run did not end cleanly or Shift is held.
    pub reconnect: bool,
}

/// What start does with the saved tabs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub tabs: Vec<Reopen>,
    /// The tab to show, by its place in `tabs`.
    pub active: Option<usize>,
    /// The last run did not end cleanly.
    pub crashed: bool,
}

/// The tabs to reopen under the setting: none for Start fresh; a saved world deleted since,
/// or an address that no longer reads, is skipped without a word.
pub fn plan(file: &SessionsFile, worlds: &[SavedWorld], choice: StartupSessions) -> Plan {
    let mut plan = Plan {
        crashed: file.running,
        ..Default::default()
    };
    if choice == StartupSessions::Fresh {
        return plan;
    }
    for (n, saved) in file.tabs.iter().enumerate() {
        let found = if saved.world_id.is_empty() {
            saved
                .address
                .parse::<Endpoint>()
                .ok()
                .map(|e| (worlds.iter().position(|w| w.is_at(&e)), e))
        } else {
            worlds
                .iter()
                .position(|w| w.world_id == saved.world_id)
                .map(|i| (Some(i), worlds[i].endpoint()))
        };
        let Some((world, endpoint)) = found else {
            continue;
        };
        // The shown tab, or the nearest kept one before it when it was skipped.
        if file.active.is_some_and(|a| n <= a) || plan.active.is_none() {
            plan.active = Some(plan.tabs.len());
        }
        plan.tabs.push(Reopen {
            endpoint,
            world,
            saved: saved.clone(),
            reconnect: choice == StartupSessions::Reconnect && saved.connected,
        });
    }
    plan
}

/// Whether Shift is held now, when the system can be asked before the window exists (macOS).
/// `None` elsewhere: the first frames are watched instead ([`ShiftWatch`]).
pub fn system_shift() -> Option<bool> {
    #[cfg(target_os = "macos")]
    {
        Some(crate::platform::mac_keys::shift_held())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Reopened sessions waiting for Shift's chance before they connect (no system to ask).
#[derive(Clone, Debug)]
pub struct ShiftWatch {
    pub sessions: Vec<SessionId>,
    first_frame: Option<Instant>,
}

impl ShiftWatch {
    pub fn new(sessions: Vec<SessionId>) -> Self {
        Self {
            sessions,
            first_frame: None,
        }
    }

    /// One frame's look at Shift: `Some(true)` once it is seen (skip), `Some(false)` once
    /// [`SHIFT_WINDOW`] has passed since the first frame without it (connect), `None` meanwhile.
    pub fn frame(&mut self, shift: bool, now: Instant) -> Option<bool> {
        if shift {
            return Some(true);
        }
        let first = *self.first_frame.get_or_insert(now);
        (now.saturating_duration_since(first) >= SHIFT_WINDOW).then_some(false)
    }

    /// When the next look is due.
    pub fn deadline(&self) -> Option<Instant> {
        self.first_frame.map(|f| f + SHIFT_WINDOW)
    }
}

/// Why reopened sessions did not connect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// The last run did not end cleanly.
    Crashed,
    /// Shift was held at start.
    Shift,
}

/// The strip under the toolbar offering to reconnect the sessions that did not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreNotice {
    pub why: Why,
    /// The reopened sessions that were connected at quit.
    pub sessions: Vec<SessionId>,
}

/// What was clicked on the strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripAction {
    ReconnectAll,
    LeaveOffline,
}

/// Draw the strip (the update strip's look). Returns what was clicked.
pub fn strip(ui: &mut Ui, notice: &RestoreNotice, theme: &Theme) -> Option<StripAction> {
    let mut action = None;
    let text = match notice.why {
        Why::Crashed => t(S::RestoreCrashedNotice),
        Why::Shift => t(S::RestoreShiftNotice),
    };
    egui::Panel::top("restore-notice")
        .frame(
            egui::Frame::new()
                .fill(theme.panel)
                .stroke(egui::Stroke::new(1.0, theme.border))
                .inner_margin(egui::Margin {
                    left: 18,
                    right: 18,
                    top: 7,
                    bottom: 7,
                }),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(26.0);
                ui.label(RichText::new(text).size(12.0).color(theme.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.spacing_mut().button_padding = egui::vec2(12.0, 4.0);
                    ui.spacing_mut().interact_size.y = 32.0;
                    if ui
                        .add(egui::Button::new(RichText::new(t(S::RestoreLeaveOffline)).size(12.0)))
                        .clicked()
                    {
                        action = Some(StripAction::LeaveOffline);
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(t(S::RestoreReconnectAll))
                                    .size(12.0)
                                    .strong()
                                    .color(theme.on_primary()),
                            )
                            .fill(theme.primary()),
                        )
                        .clicked()
                    {
                        action = Some(StripAction::ReconnectAll);
                    }
                });
            });
        });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(name: &str, id: &str, host: &str) -> SavedWorld {
        let mut w = SavedWorld::from_endpoint(name, &Endpoint::new(host, 4000));
        w.world_id = id.into();
        w
    }

    const A: &str = "0123456789abcdef0123456789abcdef";
    const B: &str = "fedcba9876543210fedcba9876543210";

    fn saved(world_id: &str, address: &str, connected: bool) -> SavedSession {
        SavedSession {
            world_id: world_id.into(),
            address: address.into(),
            connected,
            ..Default::default()
        }
    }

    #[test]
    fn the_file_round_trips_and_odd_values_are_tolerated() {
        let dir = std::env::temp_dir().join(format!("wandur-sessions-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(load(&dir), None, "no file: a fresh start");
        let file = SessionsFile {
            version: SESSIONS_VERSION,
            running: true,
            active: Some(1),
            tabs: vec![
                SavedSession {
                    name: Some("Alt".into()),
                    character: "Mira".into(),
                    view: SavedView::Map,
                    split: true,
                    ..saved(A, "a.example.org:4000", true)
                },
                saved("", "tls://[::1]:4443", false),
            ],
        };
        wandur_core::settings::write_atomic(&dir.join(SESSIONS_FILE), to_json(&file).as_bytes()).unwrap();
        assert_eq!(load(&dir), Some(file));
        let text = std::fs::read_to_string(dir.join(SESSIONS_FILE)).unwrap();
        assert!(!text.to_lowercase().contains("password"), "{text}");

        std::fs::write(
            dir.join(SESSIONS_FILE),
            r#"{"version":1,"tabs":[{"address":"h:23","view":"atlas","later":1}]}"#,
        )
        .unwrap();
        let loaded = load(&dir).unwrap();
        assert_eq!(loaded.tabs[0].view, SavedView::Play);
        assert!(!loaded.running);

        std::fs::write(dir.join(SESSIONS_FILE), r#"{"version":9,"tabs":[]}"#).unwrap();
        assert_eq!(load(&dir), None, "a newer file is not used");
        assert!(dir.join(SESSIONS_FILE).exists(), "and left alone");

        std::fs::write(dir.join(SESSIONS_FILE), "{ not json").unwrap();
        assert_eq!(load(&dir), None);
        assert!(dir.join("sessions.json.bad").exists() && !dir.join(SESSIONS_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_plan_follows_the_setting_and_skips_deleted_worlds() {
        let worlds = vec![world("Alpha", A, "a.example.org"), world("Beta", B, "b.example.org")];
        let gone = "99999999999999999999999999999999";
        let file = SessionsFile {
            version: 1,
            running: false,
            active: Some(2),
            tabs: vec![
                saved(B, "old.example.org:4000", true),
                saved("", "b.example.org:4000", false),
                saved(gone, "c.example.org:4000", true),
                saved("", "not an address", true),
                saved("", "tls://d.example.org:5000", true),
            ],
        };
        let plan = plan(&file, &worlds, StartupSessions::Reconnect);
        assert!(!plan.crashed);
        let got: Vec<(String, Option<usize>, bool)> = plan
            .tabs
            .iter()
            .map(|r| (r.endpoint.to_string(), r.world, r.reconnect))
            .collect();
        assert_eq!(
            got,
            [
                // A saved world comes back at its address as it is now.
                ("b.example.org:4000".to_string(), Some(1), true),
                // An address that is a saved world's is that world's session.
                ("b.example.org:4000".to_string(), Some(1), false),
                ("tls://d.example.org:5000".to_string(), None, true),
            ]
        );
        // The shown tab's world was deleted: the nearest kept tab before it is shown.
        assert_eq!(plan.active, Some(1));

        let tabs = super::plan(&file, &worlds, StartupSessions::Tabs);
        assert_eq!(tabs.tabs.len(), 3);
        assert!(tabs.tabs.iter().all(|r| !r.reconnect), "tabs only: nothing connects");
        assert_eq!(super::plan(&file, &worlds, StartupSessions::Fresh), Plan::default());

        let crashed = super::plan(
            &SessionsFile {
                running: true,
                active: None,
                ..file
            },
            &worlds,
            StartupSessions::Reconnect,
        );
        assert!(crashed.crashed);
        assert_eq!(crashed.active, Some(0));
    }

    #[test]
    fn the_shift_watch_waits_its_window_and_shift_skips() {
        let start = Instant::now();
        let mut watch = ShiftWatch::new(vec![1, 2]);
        assert_eq!(watch.deadline(), None);
        assert_eq!(watch.frame(false, start), None);
        assert_eq!(watch.deadline(), Some(start + SHIFT_WINDOW));
        assert_eq!(watch.frame(false, start + SHIFT_WINDOW / 2), None);
        assert_eq!(watch.frame(false, start + SHIFT_WINDOW), Some(false));
        let mut watch = ShiftWatch::new(vec![1]);
        assert_eq!(watch.frame(false, start), None);
        assert_eq!(watch.frame(true, start + Duration::from_millis(10)), Some(true));
    }
}
