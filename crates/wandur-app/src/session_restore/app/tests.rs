//! Reopening sessions at start, through the whole app over loopback: what `sessions.json` keeps
//! on quit, the tabs coming back in order with their names and pages, only the ones connected
//! at quit connecting (auto-login only where the world has it on), the setting's three choices,
//! a run that did not quit cleanly and Shift held (nothing connects; the strip's Reconnect all
//! and Leave offline), worlds deleted since, command line addresses, and scenes.

use std::io::Write as _;
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use eframe::App as _;
use egui::{Event, Modifiers, PointerButton, RawInput, Rect, pos2, vec2};
use wandur_core::directory::client::{Fetched, Fetcher};
use wandur_core::login::MemoryVault;
use wandur_core::settings::StartupSessions;

use super::*;
use crate::session_restore::{SESSIONS_FILE, SavedSession, SavedView, SessionsFile, Why};
use crate::session_tab::Status;
use crate::terminal_view::Page;

struct Offline;

impl Fetcher for Offline {
    fn get(&self, _url: &str, _limit: u64) -> Result<Fetched, String> {
        Err("offline test".into())
    }
}

fn data_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.superpowers/test-data")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A run over `dir` with these saved worlds, Shift as given (`None`: watched in the frames).
fn options(dir: &std::path::Path, worlds: &[SavedWorld], shift: Option<bool>, vault: &Arc<MemoryVault>) -> Options {
    let vault: Arc<dyn wandur_core::login::PasswordVault> = vault.clone();
    Options {
        data_dir: Some(dir.to_path_buf()),
        directory_url: Some("http://127.0.0.1:9".into()),
        fetcher: Some(Arc::new(Offline)),
        vault: Some(vault),
        csharp_vault: Some(Arc::new(MemoryVault::new())),
        csharp_folder: Some(dir.join("no-csharp-data")),
        worlds: Some(worlds.to_vec()),
        launch_shift: shift,
        language: Some(Language::En),
        ..Default::default()
    }
}

fn frame_with(app: &mut WandurApp, ctx: &egui::Context, events: Vec<Event>, modifiers: Modifiers) {
    let mut eframe_frame = eframe::Frame::_new_kittest();
    let mut all = vec![Event::ModifiersChanged(modifiers)];
    all.extend(events);
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))),
        events: all,
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx(), &mut eframe_frame);
        app.ui(ui, &mut eframe_frame);
    });
    out.textures_delta.clear();
}

fn frame(app: &mut WandurApp, ctx: &egui::Context) {
    frame_with(app, ctx, vec![], Modifiers::NONE);
}

fn frames_until(app: &mut WandurApp, ctx: &egui::Context, what: &str, done: impl Fn(&WandurApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(app) {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        frame(app, ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Frames for a while (long enough for a connection to reach a listener).
fn frames_for(app: &mut WandurApp, ctx: &egui::Context, ms: u64) {
    let until = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < until {
        frame(app, ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn listener() -> (TcpListener, Endpoint) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.set_nonblocking(true).unwrap();
    let endpoint = Endpoint::new("127.0.0.1", l.local_addr().unwrap().port());
    (l, endpoint)
}

/// A connection the listener has, if one arrived.
fn accepted(l: &TcpListener) -> Option<TcpStream> {
    l.accept().ok().map(|(s, _)| {
        s.set_nonblocking(false).unwrap();
        s
    })
}

/// Wait (drawing frames) for a connection on `l`.
fn wait_accept(app: &mut WandurApp, ctx: &egui::Context, l: &TcpListener) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(s) = accepted(l) {
            return s;
        }
        assert!(Instant::now() < deadline, "no connection arrived");
        frame(app, ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn saved_world(name: &str, id: &str, endpoint: &Endpoint) -> SavedWorld {
    let mut w = SavedWorld::from_endpoint(name, endpoint);
    w.world_id = id.into();
    w
}

const ALPHA: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
const GAMMA: &str = "c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3";

fn titles(app: &WandurApp) -> Vec<String> {
    app.sessions.iter().map(|e| e.tab.title().to_string()).collect()
}

fn session_at(app: &WandurApp, n: usize) -> SessionId {
    app.sessions.iter().nth(n).unwrap().tab.id
}

fn status(app: &WandurApp, n: usize) -> Status {
    app.sessions.iter().nth(n).unwrap().tab.status.clone()
}

fn write_file(dir: &std::path::Path, file: &SessionsFile) {
    wandur_core::settings::write_atomic(
        &dir.join(SESSIONS_FILE),
        crate::session_restore::to_json(file).as_bytes(),
    )
    .unwrap();
}

fn read_file(dir: &std::path::Path) -> SessionsFile {
    serde_json::from_str(&std::fs::read_to_string(dir.join(SESSIONS_FILE)).unwrap()).unwrap()
}

fn tab(world_id: &str, endpoint: &Endpoint, connected: bool) -> SavedSession {
    SavedSession {
        world_id: world_id.into(),
        address: endpoint.to_string(),
        connected,
        ..Default::default()
    }
}

/// Quit with three sessions (a saved world on its Map page, an address renamed and shown, a
/// saved world on Diagnostics, disconnected), moved into another order: `sessions.json` keeps
/// them in the tab order with no secret in it and the run marked as ended cleanly. The next
/// start reopens them in that order with the same names, pages and shown tab; the two that were
/// connected connect, the disconnected one waits offline. The file says running again.
#[test]
fn sessions_come_back_in_order_with_their_pages_and_only_connected_ones_connect() {
    let dir = data_dir("app-restore-order");
    let vault = Arc::new(MemoryVault::new());
    let (la, a) = listener();
    let (lb, b) = listener();
    let (lc, c) = listener();
    let worlds = vec![saved_world("Alpha", ALPHA, &a), saved_world("Gamma", GAMMA, &c)];
    let ctx = egui::Context::default();
    let mut app = WandurApp::new(&ctx, options(&dir, &worlds, Some(false), &vault));
    frame(&mut app, &ctx);
    assert!(app.sessions.is_empty(), "a first start opens nothing");
    assert!(read_file(&dir).running, "the run is marked as running at once");

    app.push_action(AppAction::ConnectWorld(0));
    app.push_action(AppAction::Connect(b.clone()));
    app.push_action(AppAction::ConnectWorld(1));
    frame(&mut app, &ctx);
    let (alpha, bee, gamma) = (session_at(&app, 0), session_at(&app, 1), session_at(&app, 2));
    let _streams = [&la, &lb, &lc].map(|l| wait_accept(&mut app, &ctx, l));
    app.push_action(AppAction::OpenFullMap(alpha));
    app.push_action(AppAction::Rename(bee, "Bee".into()));
    app.push_action(AppAction::Disconnect(gamma));
    frame(&mut app, &ctx);
    app.sessions.get_mut(gamma).unwrap().view.page = Page::Diagnostics;
    // Gamma first, then Alpha and Bee.
    app.push_action(AppAction::MoveTab(Tab::Session(gamma), 0));
    app.push_action(AppAction::Focus(bee));
    frame(&mut app, &ctx);
    frames_until(&mut app, &ctx, "gamma closed", |app| {
        app.sessions.get(gamma).unwrap().tab.is_closed()
    });
    let before = titles(&app);
    assert_eq!(before[0], "Gamma");
    app.on_exit();
    drop(app);

    let text = std::fs::read_to_string(dir.join(SESSIONS_FILE)).unwrap();
    assert!(!text.to_lowercase().contains("password"), "{text}");
    let file = read_file(&dir);
    assert!(!file.running, "a clean quit");
    assert_eq!(file.active, Some(2));
    let kept: Vec<(&str, SavedView, bool)> = file
        .tabs
        .iter()
        .map(|t| (t.world_id.as_str(), t.view, t.connected))
        .collect();
    assert_eq!(
        kept,
        [
            (GAMMA, SavedView::Diagnostics, false),
            (ALPHA, SavedView::Map, true),
            ("", SavedView::Play, true),
        ]
    );
    assert_eq!(file.tabs[2].name.as_deref(), Some("Bee"));
    assert_eq!(file.tabs[2].address, b.to_string());

    let mut app = WandurApp::new(&ctx, options(&dir, &worlds, Some(false), &vault));
    frame(&mut app, &ctx);
    assert_eq!(titles(&app), before);
    let (gamma, alpha, bee) = (session_at(&app, 0), session_at(&app, 1), session_at(&app, 2));
    assert_eq!(app.active_session, Some(bee));
    assert!(!app.directory_active);
    assert_eq!(app.sessions.get(bee).unwrap().tab.custom_name.as_deref(), Some("Bee"));
    assert_eq!(app.sessions.get(alpha).unwrap().view.page, Page::Map);
    assert!(app.full_maps.contains_key(&alpha));
    assert_eq!(app.sessions.get(gamma).unwrap().view.page, Page::Diagnostics);
    assert_eq!(app.sessions.get(alpha).unwrap().tab.world, Some(0));
    assert_eq!(app.sessions.get(bee).unwrap().tab.world, None);
    let _a = wait_accept(&mut app, &ctx, &la);
    let _b = wait_accept(&mut app, &ctx, &lb);
    frames_for(&mut app, &ctx, 200);
    assert!(accepted(&lc).is_none(), "Gamma was disconnected at quit");
    assert!(matches!(status(&app, 0), Status::Closed { .. }));
    assert!(
        app.sessions
            .get(gamma)
            .unwrap()
            .tab
            .terminal
            .transcript()
            .contains("Reopened from last time, not connected")
    );
    assert!(app.restore_notice.is_none());
    assert!(read_file(&dir).running, "this run is running");
    app.on_exit();
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Auto-login runs for a reopened session only where its world has it on: both worlds have a
/// username and a saved password; the one with auto-login sends the name at the name prompt,
/// the other sends nothing.
#[test]
fn auto_login_runs_only_for_worlds_that_have_it_on() {
    use crate::session_tab::test_support::read_lines;
    let dir = data_dir("app-restore-login");
    let vault = Arc::new(MemoryVault::new());
    let (la, a) = listener();
    let (lc, c) = listener();
    let mut on = saved_world("Alpha", ALPHA, &a);
    let mut off = saved_world("Gamma", GAMMA, &c);
    for (w, key) in [
        (&mut on, "3f2504e0-4f89-11d3-9a0c-0305e82c3301"),
        (&mut off, "3f2504e0-4f89-11d3-9a0c-0305e82c3302"),
    ] {
        w.username = "odo".into();
        w.password_id = Some(key.into());
        vault
            .write(&wandur_core::login::vault::key(w).unwrap(), "secret-test-only")
            .unwrap();
    }
    on.auto_login = true;
    write_file(
        &dir,
        &SessionsFile {
            version: 1,
            tabs: vec![tab(ALPHA, &a, true), tab(GAMMA, &c, true)],
            ..Default::default()
        },
    );
    let ctx = egui::Context::default();
    let mut app = WandurApp::new(&ctx, options(&dir, &[on, off], Some(false), &vault));
    let mut sa = wait_accept(&mut app, &ctx, &la);
    let mut sc = wait_accept(&mut app, &ctx, &lc);
    frames_until(&mut app, &ctx, "both connected", |app| {
        app.sessions.iter().all(|e| e.tab.is_connected())
    });
    frames_for(&mut app, &ctx, 100);
    sa.write_all(b"Name: ").unwrap();
    sc.write_all(b"Name: ").unwrap();
    frames_until(&mut app, &ctx, "the name sent", |app| {
        app.sessions.iter().next().unwrap().tab.login_sends == 1
    });
    assert_eq!(read_lines(&mut sa, 1), ["odo"]);
    frames_for(&mut app, &ctx, 150);
    assert_eq!(app.sessions.iter().nth(1).unwrap().tab.login_sends, 0);
    sc.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
    let mut buf = [0u8; 64];
    let got = std::io::Read::read(&mut sc, &mut buf).unwrap_or(0);
    let sent = String::from_utf8_lossy(&buf[..got]);
    assert!(!sent.contains("odo") && !sent.contains("secret"), "Gamma got {sent:?}");
    app.on_exit();
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The setting: Start fresh opens nothing; Reopen my sessions brings the tabs back, none
/// connected and no strip (even after a run that did not quit cleanly); Reopen and reconnect
/// (the default) connects. A saved world deleted since is skipped without a word.
#[test]
fn the_setting_chooses_fresh_tabs_or_reconnect_and_deleted_worlds_are_skipped() {
    let vault = Arc::new(MemoryVault::new());
    let (la, a) = listener();
    let worlds = vec![saved_world("Alpha", ALPHA, &a)];
    let deleted = "d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4";
    let ctx = egui::Context::default();
    for (choice, running) in [
        (StartupSessions::Fresh, false),
        (StartupSessions::Tabs, false),
        (StartupSessions::Tabs, true),
        (StartupSessions::Reconnect, false),
    ] {
        let dir = data_dir(&format!("app-restore-setting-{choice:?}-{running}"));
        let settings = wandur_core::settings::Settings {
            startup_sessions: choice,
            worlds: worlds.clone(),
            ..Default::default()
        };
        settings.save(&dir).unwrap();
        write_file(
            &dir,
            &SessionsFile {
                version: 1,
                running,
                active: Some(1),
                tabs: vec![tab(deleted, &Endpoint::new("127.0.0.1", 9), true), tab(ALPHA, &a, true)],
            },
        );
        let mut opts = options(&dir, &worlds, Some(false), &vault);
        opts.worlds = None;
        let mut app = WandurApp::new(&ctx, opts);
        assert_eq!(app.settings.startup_sessions, choice);
        frames_for(&mut app, &ctx, 150);
        match choice {
            StartupSessions::Fresh => assert!(app.sessions.is_empty(), "{choice:?}"),
            StartupSessions::Tabs => {
                assert_eq!(titles(&app), ["Alpha"], "the deleted world's tab is skipped");
                assert!(accepted(&la).is_none(), "tabs only: nothing connects");
                assert!(app.restore_notice.is_none());
                assert!(app.notes.is_empty(), "{:?}", app.notes);
            }
            StartupSessions::Reconnect => {
                assert_eq!(titles(&app), ["Alpha"]);
                assert_eq!(app.active_session, Some(session_at(&app, 0)));
                let _s = wait_accept(&mut app, &ctx, &la);
            }
        }
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// After a run that did not quit cleanly, the tabs come back but nothing connects (not even
/// with the setting on Reopen and reconnect); the strip says why, and Reconnect all connects
/// the sessions that were connected (not the one that was not), then goes.
#[test]
fn after_a_crash_nothing_connects_and_reconnect_all_does() {
    let dir = data_dir("app-restore-crash");
    let vault = Arc::new(MemoryVault::new());
    let (la, a) = listener();
    let (lb, b) = listener();
    let (lc, c) = listener();
    let worlds = vec![saved_world("Alpha", ALPHA, &a), saved_world("Gamma", GAMMA, &c)];
    write_file(
        &dir,
        &SessionsFile {
            version: 1,
            running: true,
            active: Some(0),
            tabs: vec![tab(ALPHA, &a, true), tab("", &b, true), tab(GAMMA, &c, false)],
        },
    );
    let ctx = egui::Context::default();
    let mut app = WandurApp::new(&ctx, options(&dir, &worlds, Some(false), &vault));
    frames_for(&mut app, &ctx, 200);
    assert_eq!(app.sessions.len(), 3);
    assert!(
        [&la, &lb, &lc].iter().all(|l| accepted(l).is_none()),
        "nothing connects"
    );
    let notice = app.restore_notice.clone().expect("the strip");
    assert_eq!(notice.why, Why::Crashed);
    assert_eq!(notice.sessions, [session_at(&app, 0), session_at(&app, 1)]);
    assert!(app.scene_probe().restore_notice);

    let at = drawn_text(&mut app, &ctx, "Reconnect all").expect("the strip's button");
    assert!(
        drawn_text(
            &mut app,
            &ctx,
            "Wandur did not close properly last time. Your sessions are back, not connected."
        )
        .is_some()
    );
    click(&mut app, &ctx, at);
    let _a = wait_accept(&mut app, &ctx, &la);
    let _b = wait_accept(&mut app, &ctx, &lb);
    frames_for(&mut app, &ctx, 150);
    assert!(accepted(&lc).is_none(), "Gamma was not connected");
    assert!(app.restore_notice.is_none(), "the strip goes");
    // The first connection of a reopened session says nothing about reconnecting.
    let first = app.sessions.iter().next().unwrap().tab.terminal.transcript();
    assert!(!first.contains("Reconnecting"), "{first}");
    app.on_exit();
    drop(app);
    assert!(!read_file(&dir).running);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Shift held at start (as `main` reads it on macOS): nothing connects, the strip says Shift;
/// Leave offline puts it away and the sessions stay offline. Where the system cannot be asked,
/// Shift down in the first frames does the same; without it they connect after the window.
#[test]
fn shift_at_start_skips_reconnecting_and_leave_offline_keeps_them_offline() {
    let vault = Arc::new(MemoryVault::new());
    let (la, a) = listener();
    let worlds = vec![saved_world("Alpha", ALPHA, &a)];
    let ctx = egui::Context::default();
    for case in ["told", "watched", "not held"] {
        let dir = data_dir(&format!("app-restore-shift-{}", case.replace(' ', "-")));
        write_file(
            &dir,
            &SessionsFile {
                version: 1,
                tabs: vec![tab(ALPHA, &a, true)],
                ..Default::default()
            },
        );
        let shift = (case == "told").then_some(true);
        let mut app = WandurApp::new(&ctx, options(&dir, &worlds, shift, &vault));
        let held = if case == "not held" {
            Modifiers::NONE
        } else {
            Modifiers::SHIFT
        };
        let started = Instant::now();
        while started.elapsed() < crate::session_restore::SHIFT_WINDOW + Duration::from_millis(150) {
            frame_with(&mut app, &ctx, vec![], held);
            std::thread::sleep(Duration::from_millis(5));
        }
        if case == "not held" {
            let _s = wait_accept(&mut app, &ctx, &la);
            assert!(app.restore_notice.is_none());
        } else {
            assert!(accepted(&la).is_none(), "{case}: nothing connects");
            assert_eq!(app.restore_notice.as_ref().map(|n| n.why), Some(Why::Shift), "{case}");
            assert!(
                drawn_text(
                    &mut app,
                    &ctx,
                    "Shift was held at start, so your sessions reopened without connecting."
                )
                .is_some()
            );
            let at = drawn_text(&mut app, &ctx, "Leave offline").unwrap();
            click(&mut app, &ctx, at);
            assert!(app.restore_notice.is_none());
            frames_for(&mut app, &ctx, 150);
            assert!(accepted(&la).is_none(), "{case}: still offline");
            assert!(app.sessions.iter().next().unwrap().tab.is_closed());
        }
        app.on_exit();
        drop(app);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A command line address that a reopened session already has is not opened twice: that
/// session connects (it was disconnected at quit) and is shown; another address opens as
/// before, after the reopened tabs.
#[test]
fn command_line_addresses_open_as_before_without_duplicating_a_reopened_session() {
    let dir = data_dir("app-restore-cli");
    let vault = Arc::new(MemoryVault::new());
    let (la, a) = listener();
    let (lb, b) = listener();
    let (lx, x) = listener();
    write_file(
        &dir,
        &SessionsFile {
            version: 1,
            active: Some(0),
            tabs: vec![tab("", &a, true), tab("", &b, false)],
            ..Default::default()
        },
    );
    let ctx = egui::Context::default();
    let mut opts = options(&dir, &[], Some(false), &vault);
    opts.connect = vec![b.clone(), x.clone()];
    let mut app = WandurApp::new(&ctx, opts);
    frame(&mut app, &ctx);
    assert_eq!(titles(&app), [a.to_string(), b.to_string(), x.to_string()]);
    let _streams = [&la, &lb, &lx].map(|l| wait_accept(&mut app, &ctx, l));
    assert_eq!(
        app.active_session,
        Some(session_at(&app, 2)),
        "the last address is shown"
    );
    app.on_exit();
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A scene never reads or writes `sessions.json`: a data directory's file is left as it was
/// and nothing is reopened from it; a scene's own tabs come back without a file being made.
#[test]
fn scenes_leave_the_sessions_file_alone() {
    let dir = data_dir("app-restore-scene");
    let vault = Arc::new(MemoryVault::new());
    let (la, a) = listener();
    let worlds = vec![saved_world("Alpha", ALPHA, &a)];
    let file = SessionsFile {
        version: 1,
        running: true,
        tabs: vec![tab(ALPHA, &a, true)],
        ..Default::default()
    };
    write_file(&dir, &file);
    let before = std::fs::read(dir.join(SESSIONS_FILE)).unwrap();
    let ctx = egui::Context::default();
    let mut opts = options(&dir, &worlds, Some(false), &vault);
    opts.ephemeral = true;
    let mut app = WandurApp::new(&ctx, opts);
    frames_for(&mut app, &ctx, 100);
    assert!(app.sessions.is_empty());
    assert!(app.restore_notice.is_none());
    app.shutdown();
    drop(app);
    assert_eq!(std::fs::read(dir.join(SESSIONS_FILE)).unwrap(), before);
    assert!(accepted(&la).is_none());

    let empty = data_dir("app-restore-scene-own");
    let mut opts = options(&empty, &worlds, Some(false), &vault);
    opts.ephemeral = true;
    opts.restore = Some(file);
    let mut app = WandurApp::new(&ctx, opts);
    frames_for(&mut app, &ctx, 100);
    assert_eq!(app.sessions.len(), 1);
    assert_eq!(app.restore_notice.as_ref().map(|n| n.why), Some(Why::Crashed));
    app.shutdown();
    drop(app);
    assert!(!empty.join(SESSIONS_FILE).exists());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&empty);
}

/// Where `text` was drawn last frame (the topmost such text), if anywhere.
fn drawn_text(app: &mut WandurApp, ctx: &egui::Context, text: &str) -> Option<egui::Pos2> {
    fn walk(shape: &egui::Shape, text: &str, out: &mut Option<egui::Pos2>) {
        match shape {
            egui::Shape::Text(t) if t.galley.text() == text => {
                let at = t.pos + t.galley.rect.center().to_vec2();
                if out.is_none_or(|o| at.y < o.y) {
                    *out = Some(at);
                }
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| walk(s, text, out)),
            _ => {}
        }
    }
    let mut eframe_frame = eframe::Frame::_new_kittest();
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))),
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx(), &mut eframe_frame);
        app.ui(ui, &mut eframe_frame);
    });
    out.textures_delta.clear();
    let mut found = None;
    for clipped in &out.shapes {
        walk(&clipped.shape, text, &mut found);
    }
    found
}

fn click(app: &mut WandurApp, ctx: &egui::Context, pos: egui::Pos2) {
    let button = |pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    frame_with(app, ctx, vec![Event::PointerMoved(pos), button(true)], Modifiers::NONE);
    frame_with(app, ctx, vec![button(false)], Modifiers::NONE);
    frame(app, ctx);
}
