//! The session tabs and the undo toast, driven through the whole app with real frames: tabs
//! per session in order, new output and connection dots, closing (button, middle click, Cmd+W),
//! dragging to reorder, Ctrl+Tab and Cmd+1..9, overflow, the "+", the Workspace closed by
//! default; the toast for map rooms and exits, saved worlds and the world editor's scripts and
//! macros, its pause under the pointer, Undo, replacement, and big deletes that still ask.

use std::io::Write as _;
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use eframe::App as _;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};
use wandur_core::directory::client::{Fetched, Fetcher};

use super::*;
use crate::session_tabs::{self, Conn};

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

fn options(dir: &std::path::Path) -> Options {
    Options {
        data_dir: Some(dir.to_path_buf()),
        directory_url: Some("http://127.0.0.1:9".into()),
        fetcher: Some(Arc::new(Offline)),
        vault: Some(Arc::new(wandur_core::login::MemoryVault::new())),
        csharp_vault: Some(Arc::new(wandur_core::login::MemoryVault::new())),
        csharp_folder: Some(dir.join("no-csharp-data")),
        ..Default::default()
    }
}

fn frame(app: &mut WandurApp, ctx: &egui::Context, events: Vec<Event>) {
    let mut eframe_frame = eframe::Frame::_new_kittest();
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 800.0))),
        events,
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, |ui| {
        app.logic(ui.ctx(), &mut eframe_frame);
        app.ui(ui, &mut eframe_frame);
    });
    out.textures_delta.clear();
}

fn frames(app: &mut WandurApp, ctx: &egui::Context, n: usize) {
    for _ in 0..n {
        frame(app, ctx, vec![]);
    }
}

fn key(key: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// Cmd on macOS, Ctrl elsewhere, as the window system reports them (on macOS both the Mac
/// command key and egui's logical command).
fn command() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    }
}

fn button(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

/// A click (press, then release a frame later) with a pointer button, then a quiet frame.
fn click_with(app: &mut WandurApp, ctx: &egui::Context, pos: Pos2, which: PointerButton) {
    frame(app, ctx, vec![Event::PointerMoved(pos)]);
    frame(app, ctx, vec![button(pos, which, true)]);
    frame(app, ctx, vec![button(pos, which, false)]);
    frame(app, ctx, vec![]);
}

/// Where the strip drew each tab last frame.
fn tabs(ctx: &egui::Context) -> Vec<(Tab, Rect)> {
    let mut rects = session_tabs::tab_rects(ctx, shell::strip_id(0));
    rects.sort_by(|a, b| a.1.left().total_cmp(&b.1.left()));
    rects
}

fn rect_of(ctx: &egui::Context, tab: Tab) -> Rect {
    tabs(ctx).into_iter().find(|(t, _)| *t == tab).expect("a tab").1
}

/// Open `n` sessions to a loopback listener (accepted, so they connect); returns their ids and
/// the server ends.
fn open_sessions(app: &mut WandurApp, ctx: &egui::Context, n: usize) -> (TcpListener, Vec<SessionId>, Vec<TcpStream>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = Endpoint::new("127.0.0.1", listener.local_addr().unwrap().port());
    let mut ids = Vec::new();
    let mut servers = Vec::new();
    for _ in 0..n {
        app.open(endpoint.clone(), None);
        ids.push(app.active_session.unwrap());
        servers.push(listener.accept().unwrap().0);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.sessions.connected() < n {
        assert!(Instant::now() < deadline, "sessions connect");
        frame(app, ctx, vec![]);
        std::thread::sleep(Duration::from_millis(5));
    }
    frames(app, ctx, 2);
    (listener, ids, servers)
}

fn items(app: &WandurApp) -> Vec<(Tab, String, Option<Conn>, bool)> {
    let documents = workspace::documents(&app.dock);
    let visible = workspace::visible_sessions(&app.dock);
    shell::strip_items(&documents, &app.sessions, &visible)
        .into_iter()
        .map(|i| (i.tab, i.title.into_owned(), i.conn, i.activity))
        .collect()
}

/// A new install has no Workspace panel; the session tabs show Find a MUD and one tab per
/// session in the order they opened, numbered when they share a name, the newest shown.
#[test]
fn a_tab_per_session_in_order_with_the_workspace_closed() {
    let dir = data_dir("tabs-order");
    let ctx = egui::Context::default();
    let mut app = WandurApp::new(&ctx, options(&dir));
    frames(&mut app, &ctx, 2);
    assert!(
        !app.panel_visible(Tab::Workspace),
        "the Workspace is closed on a new install"
    );
    let (_listener, ids, _servers) = open_sessions(&mut app, &ctx, 3);
    let expected = [
        Tab::Directory,
        Tab::Session(ids[0]),
        Tab::Session(ids[1]),
        Tab::Session(ids[2]),
    ];
    assert_eq!(workspace::documents(&app.dock), expected);
    let drawn: Vec<Tab> = tabs(&ctx).into_iter().map(|(t, _)| t).collect();
    assert_eq!(drawn, expected, "left to right in that order");
    assert_eq!(app.active_session, Some(ids[2]));
    let titles: Vec<String> = items(&app).into_iter().map(|i| i.1).collect();
    assert_eq!(titles[0], "Find a MUD");
    for (n, title) in titles[1..].iter().enumerate() {
        assert!(title.starts_with("127.0.0.1"), "{title}");
        assert!(title.ends_with(&format!(" · {}", n + 1)), "numbered: {title}");
    }
    // A click shows a tab.
    let first = rect_of(&ctx, Tab::Session(ids[0]));
    click_with(&mut app, &ctx, first.center() - vec2(20.0, 0.0), PointerButton::Primary);
    assert_eq!(app.active_session, Some(ids[0]));
    assert!(!app.directory_active);
    app.on_exit();
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A background session with new output gets the activity marker until it is shown; the dot
/// follows the connection (connected, then disconnected when the server goes).
#[test]
fn background_output_marks_a_tab_and_the_dot_follows_the_connection() {
    let dir = data_dir("tabs-activity");
    let ctx = egui::Context::default();
    let mut app = WandurApp::new(&ctx, options(&dir));
    let (_listener, ids, mut servers) = open_sessions(&mut app, &ctx, 2);
    for (_, _, conn, activity) in items(&app).into_iter().skip(1) {
        assert_eq!(conn, Some(Conn::Connected));
        assert!(!activity);
    }
    servers[0].write_all(b"a whisper from the first world\r\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !items(&app)[1].3 {
        assert!(Instant::now() < deadline, "the background tab is marked");
        frame(&mut app, &ctx, vec![]);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!items(&app)[2].3, "the shown session has no marker");
    // Shown, its output is seen.
    app.push_action(AppAction::Focus(ids[0]));
    frames(&mut app, &ctx, 2);
    assert!(!items(&app)[1].3);
    // The server goes: the dot says disconnected.
    drop(servers.remove(0));
    while items(&app)[1].2 != Some(Conn::Disconnected) {
        assert!(Instant::now() < deadline, "the dot shows the session closed");
        frame(&mut app, &ctx, vec![]);
        std::thread::sleep(Duration::from_millis(5));
    }
    app.on_exit();
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A middle click closes a background tab (the shown one stays); the close button closes the
/// shown tab and its neighbour is shown; Cmd+W closes the shown tab; the last session gives way
/// to Find a MUD, which stays.
#[test]
fn tabs_close_by_button_middle_click_and_cmd_w() {
    let dir = data_dir("tabs-close");
    let ctx = egui::Context::default();
    let mut app = WandurApp::new(&ctx, options(&dir));
    let (_listener, ids, _servers) = open_sessions(&mut app, &ctx, 3);
    let a = rect_of(&ctx, Tab::Session(ids[0]));
    click_with(&mut app, &ctx, a.center() - vec2(20.0, 0.0), PointerButton::Middle);
    assert!(app.sessions.get(ids[0]).is_none(), "middle click closed it");
    assert_eq!(app.active_session, Some(ids[2]), "the shown tab stayed");
    // The shown (last) tab's close button: its left neighbour is shown.
    let c = rect_of(&ctx, Tab::Session(ids[2]));
    click_with(
        &mut app,
        &ctx,
        session_tabs::close_rect(c).center(),
        PointerButton::Primary,
    );
    assert!(app.sessions.get(ids[2]).is_none());
    assert_eq!(app.active_session, Some(ids[1]));
    assert_eq!(workspace::documents(&app.dock), [Tab::Directory, Tab::Session(ids[1])]);
    // Cmd+W closes the shown session; Find a MUD is shown.
    frame(&mut app, &ctx, vec![key(Key::W, command())]);
    frames(&mut app, &ctx, 2);
    assert!(app.sessions.is_empty());
    assert!(app.directory_active);
    assert_eq!(workspace::documents(&app.dock), [Tab::Directory]);
    // Find a MUD alone stays.
    frame(&mut app, &ctx, vec![key(Key::W, command())]);
    frames(&mut app, &ctx, 2);
    assert_eq!(workspace::documents(&app.dock), [Tab::Directory]);
    // The last session closed with Find a MUD already closed: Find a MUD takes its place.
    let (_listener, ids, _servers) = open_sessions(&mut app, &ctx, 1);
    app.push_action(AppAction::CloseDirectory);
    frames(&mut app, &ctx, 2);
    assert_eq!(workspace::documents(&app.dock), [Tab::Session(ids[0])]);
    app.push_action(AppAction::Close(ids[0]));
    frames(&mut app, &ctx, 2);
    assert_eq!(workspace::documents(&app.dock), [Tab::Directory]);
    assert!(app.directory_active);
    app.on_exit();
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Dragging a tab past the others puts it there; the sessions (and so the Workspace list and
/// Ctrl+Tab) follow; a preset keeps the order.
#[test]
fn dragging_a_tab_reorders_the_sessions() {
    let dir = data_dir("tabs-drag");
    let ctx = egui::Context::default();
    let mut app = WandurApp::new(&ctx, options(&dir));
    let (_listener, ids, _servers) = open_sessions(&mut app, &ctx, 3);
    let a = rect_of(&ctx, Tab::Session(ids[0]));
    let c = rect_of(&ctx, Tab::Session(ids[2]));
    let from = a.center() - vec2(20.0, 0.0);
    frame(&mut app, &ctx, vec![Event::PointerMoved(from)]);
    frame(&mut app, &ctx, vec![button(from, PointerButton::Primary, true)]);
    let to = pos2(c.right() - 10.0, from.y);
    for step in 1..=12 {
        let x = from.x + (to.x - from.x) * step as f32 / 12.0;
        frame(&mut app, &ctx, vec![Event::PointerMoved(pos2(x, from.y))]);
    }
    frame(&mut app, &ctx, vec![button(to, PointerButton::Primary, false)]);
    frames(&mut app, &ctx, 2);
    let order = [
        Tab::Directory,
        Tab::Session(ids[1]),
        Tab::Session(ids[2]),
        Tab::Session(ids[0]),
    ];
    assert_eq!(workspace::documents(&app.dock), order);
    let sessions: Vec<SessionId> = app.sessions.iter().map(|e| e.tab.id).collect();
    assert_eq!(sessions, [ids[1], ids[2], ids[0]]);
    assert_eq!(app.active_session, Some(ids[2]), "a drag does not change the shown tab");
    app.apply_preset(workspace::Preset::Right);
    frames(&mut app, &ctx, 2);
    let after: Vec<Tab> = workspace::documents(&app.dock)
        .into_iter()
        .filter(|t| matches!(t, Tab::Session(_)))
        .collect();
    assert_eq!(after, order[1..]);
    app.on_exit();
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ctrl+Tab and Ctrl+Shift+Tab go round the tabs in the order shown (Find a MUD among them);
/// Cmd+1..9 show tab n; a number past the last tab does nothing.
#[test]
fn ctrl_tab_and_cmd_digits_follow_the_tab_order() {
    let dir = data_dir("tabs-keys");
    let ctx = egui::Context::default();
    let mut app = WandurApp::new(&ctx, options(&dir));
    let (_listener, ids, _servers) = open_sessions(&mut app, &ctx, 3);
    assert_eq!(app.active_session, Some(ids[2]));
    let ctrl_tab = || key(Key::Tab, Modifiers::CTRL);
    frame(&mut app, &ctx, vec![ctrl_tab()]);
    frames(&mut app, &ctx, 2);
    assert!(app.directory_active, "round from the last tab to Find a MUD");
    frame(&mut app, &ctx, vec![ctrl_tab()]);
    frames(&mut app, &ctx, 2);
    assert_eq!((app.directory_active, app.active_session), (false, Some(ids[0])));
    frame(&mut app, &ctx, vec![key(Key::Tab, Modifiers::CTRL | Modifiers::SHIFT)]);
    frames(&mut app, &ctx, 2);
    assert!(app.directory_active);
    frame(&mut app, &ctx, vec![key(Key::Num3, command())]);
    frames(&mut app, &ctx, 2);
    assert_eq!((app.directory_active, app.active_session), (false, Some(ids[1])));
    frame(&mut app, &ctx, vec![key(Key::Num9, command())]);
    frames(&mut app, &ctx, 2);
    assert_eq!(app.active_session, Some(ids[1]), "no ninth tab");
    // After a move, the numbers follow the new order.
    app.push_action(AppAction::MoveTab(Tab::Session(ids[0]), 3));
    frames(&mut app, &ctx, 2);
    frame(&mut app, &ctx, vec![key(Key::Num4, command())]);
    frames(&mut app, &ctx, 2);
    assert_eq!(app.active_session, Some(ids[0]));
    app.on_exit();
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A few tabs take what their titles need; many shrink to the narrowest width and then scroll,
/// the shown one kept in view, with arrows and a list of every tab; the "+" opens Find a MUD.
#[test]
fn many_tabs_shrink_then_scroll_and_the_plus_opens_find_a_mud() {
    let dir = data_dir("tabs-overflow");
    let ctx = egui::Context::default();
    let mut app = WandurApp::new(&ctx, options(&dir));
    let (_l1, ids, _s1) = open_sessions(&mut app, &ctx, 3);
    let [plus, left, ..] = session_tabs::button_rects(&ctx, shell::strip_id(0));
    assert!(plus.is_some() && left.is_none(), "no arrows while the tabs fit");
    assert!(
        tabs(&ctx)
            .iter()
            .skip(1)
            .all(|(_, r)| r.width() > session_tabs::TAB_MIN)
    );
    let (_l2, more, _s2) = open_sessions(&mut app, &ctx, 13);
    frames(&mut app, &ctx, 2);
    let [plus, left, right, list] = session_tabs::button_rects(&ctx, shell::strip_id(0));
    let (left, _right, list) = (left.expect("arrows"), right.expect("arrows"), list.expect("the list"));
    let drawn = tabs(&ctx);
    assert_eq!(drawn.len(), 17);
    assert!(
        drawn
            .iter()
            .skip(1)
            .all(|(_, r)| (r.width() - session_tabs::TAB_MIN).abs() < 0.5)
    );
    // The newest (shown) tab is scrolled into view, left of the arrows.
    let shown = rect_of(&ctx, Tab::Session(*more.last().unwrap()));
    assert!(shown.right() <= left.left() + 0.5, "{shown:?} {left:?}");
    let scrolled = session_tabs::scroll_of(&ctx, shell::strip_id(0));
    assert!(scrolled > 0.0);
    click_with(&mut app, &ctx, left.center(), PointerButton::Primary);
    assert!(
        session_tabs::scroll_of(&ctx, shell::strip_id(0)) < scrolled,
        "the left arrow scrolls back"
    );
    click_with(&mut app, &ctx, list.center(), PointerButton::Primary);
    assert!(egui::Popup::is_any_open(&ctx), "the list of every tab opens");
    frame(&mut app, &ctx, vec![key(Key::Escape, Modifiers::NONE)]);
    frames(&mut app, &ctx, 2);
    // The "+" opens Find a MUD (here after it was closed).
    app.push_action(AppAction::Focus(ids[0]));
    app.push_action(AppAction::CloseDirectory);
    frames(&mut app, &ctx, 3);
    assert!(!workspace::documents(&app.dock).contains(&Tab::Directory));
    let [plus2, ..] = session_tabs::button_rects(&ctx, shell::strip_id(0));
    let _ = plus;
    click_with(&mut app, &ctx, plus2.unwrap().center(), PointerButton::Primary);
    assert!(workspace::documents(&app.dock).contains(&Tab::Directory));
    assert!(app.directory_active);
    app.on_exit();
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The demo world's map: a delete of three rooms (from the full map's editor) shows the toast,
/// whose Undo is the tracker's undo step; an exit's delete too; another edit after it makes the
/// toast go (the map's own Undo still works); six rooms still ask first, with no toast.
#[test]
fn map_deletes_show_the_undo_toast() {
    use crate::map_view::editor::{EditAction, EditorState};
    fn editor(app: &mut WandurApp, id: SessionId) -> &mut EditorState {
        app.full_maps.get_mut(&id).unwrap().editor.as_deref_mut().unwrap()
    }
    let dir = data_dir("toast-map");
    let ctx = egui::Context::default();
    let mut app = WandurApp::new(
        &ctx,
        Options {
            demo: true,
            ..options(&dir)
        },
    );
    frame(&mut app, &ctx, vec![]);
    let id = app.sessions.iter().next().unwrap().tab.id;
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.sessions.get(id).unwrap().tab.map.tracker().room_count() < 1 {
        assert!(Instant::now() < deadline, "the demo map has its first room");
        frame(&mut app, &ctx, vec![]);
        std::thread::sleep(Duration::from_millis(5));
    }
    app.push_action(AppAction::OpenMapEditor(id));
    frames(&mut app, &ctx, 3);
    // Eight rooms of our own, the first two joined by an exit.
    let old: Vec<String> = app
        .sessions
        .get(id)
        .unwrap()
        .tab
        .map
        .tracker()
        .rooms()
        .map(|r| r.id.clone())
        .collect();
    for n in 0..8 {
        editor(&mut app, id)
            .pending
            .push(EditAction::AddRoomAt(20.0 + n as f64, 20.0));
        frames(&mut app, &ctx, 1);
    }
    let added: Vec<String> = app
        .sessions
        .get(id)
        .unwrap()
        .tab
        .map
        .tracker()
        .rooms()
        .map(|r| r.id.clone())
        .filter(|r| !old.contains(r))
        .collect();
    assert_eq!(added.len(), 8);
    editor(&mut app, id).pending.push(EditAction::Connect {
        from: added[0].clone(),
        to: added[1].clone(),
        direction: "east".into(),
        two_way: false,
    });
    frames(&mut app, &ctx, 2);
    assert!(app.toast.is_none(), "adding is not a delete");
    let before = app.sessions.get(id).unwrap().tab.map.tracker().room_count();
    let victims: Vec<String> = added[2..5].to_vec();
    editor(&mut app, id)
        .pending
        .push(EditAction::DeleteRooms(victims.clone()));
    frames(&mut app, &ctx, 2);
    assert_eq!(app.sessions.get(id).unwrap().tab.map.tracker().room_count(), before - 3);
    let toast = app.toast.as_ref().expect("the undo toast");
    assert_eq!(toast.text, "Deleted 3 rooms");
    app.push_action(AppAction::UndoToast(toast.serial));
    frames(&mut app, &ctx, 2);
    assert_eq!(app.sessions.get(id).unwrap().tab.map.tracker().room_count(), before);
    assert!(app.toast.is_none());
    // An exit.
    let link = app
        .sessions
        .get(id)
        .unwrap()
        .tab
        .map
        .tracker()
        .links()
        .next()
        .unwrap()
        .clone();
    editor(&mut app, id)
        .pending
        .push(EditAction::DeleteExit(link.from_id.clone(), link.direction.clone()));
    frames(&mut app, &ctx, 2);
    assert_eq!(app.toast.as_ref().unwrap().text, "Deleted 1 exit");
    // Another edit: the toast goes; the map's own Undo still takes the delete back after it.
    editor(&mut app, id)
        .pending
        .push(EditAction::DeleteRooms(vec![victims[0].clone()]));
    frames(&mut app, &ctx, 2);
    assert_eq!(
        app.toast.as_ref().unwrap().text,
        "Deleted 1 room",
        "replaced by the newer delete"
    );
    let replaced = app.toast.as_ref().unwrap().serial;
    editor(&mut app, id).pending.push(EditAction::Undo);
    frames(&mut app, &ctx, 2);
    assert!(app.toast.is_none(), "undone from the Edit menu: nothing left to offer");
    let _ = replaced;
    editor(&mut app, id).pending.push(EditAction::Undo);
    frames(&mut app, &ctx, 2);
    assert!(
        app.sessions
            .get(id)
            .unwrap()
            .tab
            .map
            .tracker()
            .link(&link.from_id, &link.direction)
            .is_some(),
        "the exit is back by the map's own Undo"
    );
    // Six rooms still ask first; nothing is deleted and no toast is shown until confirmed.
    let six: Vec<String> = added[2..8].to_vec();
    {
        editor(&mut app, id).rooms = six.clone();
        let state = app.full_maps.get_mut(&id).unwrap();
        assert!(crate::map_view::editor::canvas::request_delete(state).is_none());
        assert_eq!(state.editor.as_deref().unwrap().confirm_delete.as_ref(), Some(&six));
        frames(&mut app, &ctx, 2);
        assert!(app.toast.is_none());
    }
    app.on_exit();
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The toast's clock stands still under the pointer; the world editor's newer toast replaces
/// the main window's.
#[test]
fn the_toast_pauses_under_the_pointer_and_one_shows_at_a_time() {
    let dir = data_dir("toast-hover");
    let ctx = egui::Context::default();
    let mut app = WandurApp::new(&ctx, options(&dir));
    app.settings.worlds = vec![
        SavedWorld::from_endpoint(String::from("One"), &Endpoint::new("127.0.0.1", 9)),
        SavedWorld::from_endpoint(String::from("Two"), &Endpoint::new("127.0.0.1", 9)),
    ];
    frames(&mut app, &ctx, 2);
    app.push_action(AppAction::DeleteWorld(1));
    frames(&mut app, &ctx, 2);
    let serial = app.toast.as_ref().unwrap().serial;
    let area = ctx
        .memory(|m| m.area_rect(egui::Id::new("undo-toast").with(serial)))
        .expect("the toast was drawn");
    assert!(area.center().y > 600.0, "near the bottom of the window: {area:?}");
    frame(&mut app, &ctx, vec![Event::PointerMoved(area.center())]);
    frames(&mut app, &ctx, 2);
    assert!(app.toast.as_ref().unwrap().hovered);
    // The world editor's toast (a deleted macro) is newer: the main one goes.
    app.push_action(AppAction::EditWorld(0));
    frames(&mut app, &ctx, 2);
    let entry = wandur_core::db::scripts::LibraryEntry::new_macro("m", wandur_core::macros::MacroDefinition::starter());
    app.form.as_mut().unwrap().toast = Some(Toast::new(
        "Deleted macro m".into(),
        Undo::Macro {
            entry: Box::new(entry),
            index: 0,
        },
    ));
    frames(&mut app, &ctx, 2);
    assert!(app.toast.is_none());
    // A main-window toast clears the editor's.
    app.push_action(AppAction::DeleteWorld(0));
    frames(&mut app, &ctx, 2);
    assert!(app.toast.is_some());
    assert!(app.form.as_ref().is_none_or(|f| f.toast.is_none()));
    app.on_exit();
    drop(app);
    let _ = std::fs::remove_dir_all(&dir);
}
