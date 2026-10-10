//! File > Import map through the whole app with real frames: a Mudlet map into the open
//! session's map as one undo step (the toast takes it back, a second import changes nothing),
//! into a world with no session open (saved, and the toast takes that back too), the `.dat`
//! note, and the full map's Import opening the dialog for its own session.

use std::time::{Duration, Instant};

use eframe::App as _;
use egui::{RawInput, Rect, pos2, vec2};
use wandur_core::directory::client::{Fetched, Fetcher};
use wandur_core::map::store::{MapStore, MapWorld};

use super::*;
use crate::map_import::Target;

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

fn frame(app: &mut WandurApp, ctx: &egui::Context) {
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
}

fn frames_until(app: &mut WandurApp, ctx: &egui::Context, done: impl Fn(&WandurApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(app) {
        assert!(Instant::now() < deadline, "timed out");
        frame(app, ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wandur-core/tests/fixtures/mudlet/lantern-road-map.json")
}

/// The demo app with its session's map showing its first room.
fn demo_app(ctx: &egui::Context, dir: &std::path::Path) -> (WandurApp, SessionId) {
    let mut app = WandurApp::new(
        ctx,
        Options {
            demo: true,
            ..options(dir)
        },
    );
    frame(&mut app, ctx);
    let id = app.sessions.iter().next().unwrap().tab.id;
    frames_until(&mut app, ctx, |a| {
        a.sessions.get(id).unwrap().tab.map.tracker().room_count() > 0
    });
    (app, id)
}

/// Read the fixture in the open dialog and wait for its summary.
fn read_fixture(app: &mut WandurApp, ctx: &egui::Context, path: &std::path::Path) {
    app.map_import.as_mut().expect("the dialog").read_file(path);
    frames_until(app, ctx, |a| {
        a.map_import
            .as_ref()
            .is_some_and(|d| d.prepared().is_some() || d.error().is_some())
    });
}

#[test]
fn a_mudlet_map_goes_into_the_open_session_as_one_undo_step() {
    wandur_core::l10n::override_thread(Some(wandur_core::l10n::Language::En));
    let dir = data_dir("map-import-session");
    let ctx = egui::Context::default();
    let (mut app, id) = demo_app(&ctx, &dir);
    let rooms = |a: &WandurApp| a.sessions.get(id).unwrap().tab.map.tracker().room_count();
    let before = rooms(&app);
    app.run_command(&ctx, Command::ImportMap);
    let dialog = app.map_import.as_ref().expect("File > Import map opens the dialog");
    assert_eq!(
        dialog.target(),
        Some(&Target::Session(id)),
        "the active session's world first"
    );
    read_fixture(&mut app, &ctx, &fixture());
    let prepared = app.map_import.as_ref().unwrap().prepared().unwrap().clone();
    assert_eq!(prepared.summary.rooms, 8);
    assert_eq!(rooms(&app), before, "nothing changes before Import");
    app.apply_map_import(prepared.clone(), Target::Session(id));
    frame(&mut app, &ctx);
    assert!(app.map_import.is_none());
    assert_eq!(rooms(&app), before + 8);
    let tracker = app.sessions.get(id).unwrap().tab.map.tracker();
    assert_eq!(tracker.label_count(), 2);
    let toast = app.toast.clone().expect("the undo toast");
    assert!(toast.text.starts_with("Imported a map into"), "{}", toast.text);
    // The same file again changes nothing, and says so.
    app.open_map_import(&ctx, Some(id));
    read_fixture(&mut app, &ctx, &fixture());
    let again = app.map_import.as_ref().unwrap().prepared().unwrap().clone();
    app.apply_map_import(again, Target::Session(id));
    frame(&mut app, &ctx);
    assert_eq!(rooms(&app), before + 8);
    assert_eq!(
        app.notes.first().map(String::as_str),
        Some("The map already has everything in this file.")
    );
    assert_eq!(
        app.toast.as_ref().map(|t| t.serial),
        Some(toast.serial),
        "the first toast stays"
    );
    // Undo takes the whole import back.
    app.push_action(AppAction::UndoToast(toast.serial));
    frame(&mut app, &ctx);
    assert_eq!(rooms(&app), before);
    assert_eq!(app.sessions.get(id).unwrap().tab.map.tracker().label_count(), 0);
    assert!(app.toast.is_none());
    app.on_exit();
    drop(app);
    wandur_core::l10n::override_thread(None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_map_goes_into_a_world_with_no_session_and_undo_takes_it_back() {
    wandur_core::l10n::override_thread(Some(wandur_core::l10n::Language::En));
    let dir = data_dir("map-import-world");
    let ctx = egui::Context::default();
    let mut app = WandurApp::new(&ctx, options(&dir));
    frame(&mut app, &ctx);
    let world_id = wandur_core::db::worlds::new_world_id();
    let mut world = wandur_core::settings::SavedWorld::from_endpoint(
        "Lantern Road",
        &wandur_core::endpoint::Endpoint::new("lantern.example", 4100),
    );
    world.world_id = world_id.clone();
    app.settings.worlds.push(world);
    app.run_command(&ctx, Command::ImportMap);
    let key = MapWorld::Id(world_id);
    {
        let dialog = app.map_import.as_mut().unwrap();
        let i = dialog
            .choices
            .iter()
            .position(|c| c.target == Target::World(key.clone()))
            .expect("the saved world is offered");
        assert_eq!(dialog.choices[i].label, "Lantern Road");
        dialog.chosen = i;
    }
    read_fixture(&mut app, &ctx, &fixture());
    let prepared = app.map_import.as_ref().unwrap().prepared().unwrap().clone();
    app.apply_map_import(prepared, Target::World(key.clone()));
    frames_until(&mut app, &ctx, |a| a.toast.is_some());
    assert!(app.map_import.is_none());
    let store = MapStore::new(app.db.clone().unwrap());
    let saved = store.load(&key).unwrap().unwrap();
    assert_eq!((saved.rooms.len(), saved.links.len(), saved.labels.len()), (8, 12, 2));
    assert_eq!(saved.images.len(), 1, "the picture is saved with its label");
    let serial = app.toast.as_ref().unwrap().serial;
    assert_eq!(app.toast.as_ref().unwrap().text, "Imported a map into Lantern Road");
    app.push_action(AppAction::UndoToast(serial));
    frame(&mut app, &ctx);
    app.map_worker.as_ref().unwrap().flush();
    let undone = store.load(&key).unwrap().unwrap();
    assert!(undone.rooms.is_empty() && undone.labels.is_empty() && undone.images.is_empty());
    assert!(app.detached_maps.is_empty());
    app.on_exit();
    drop(app);
    wandur_core::l10n::override_thread(None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mudlet_dat_file_gets_the_json_note_and_the_full_maps_import_opens_the_dialog() {
    wandur_core::l10n::override_thread(Some(wandur_core::l10n::Language::En));
    let dir = data_dir("map-import-dat");
    let ctx = egui::Context::default();
    let (mut app, id) = demo_app(&ctx, &dir);
    // The full map's Import, nothing selected.
    app.push_action(AppAction::OpenMapEditor(id));
    frame(&mut app, &ctx);
    app.full_maps
        .get_mut(&id)
        .unwrap()
        .editor
        .as_deref_mut()
        .unwrap()
        .pending
        .push(crate::map_view::editor::EditAction::OpenImport);
    frames_until(&mut app, &ctx, |a| a.map_import.is_some());
    assert_eq!(app.map_import.as_ref().unwrap().target(), Some(&Target::Session(id)));
    std::fs::create_dir_all(&dir).unwrap();
    let dat = dir.join("map.dat");
    std::fs::write(&dat, [0u8, 0, 0, 20, 0, 0, 0, 1, 0, 0]).unwrap();
    read_fixture(&mut app, &ctx, &dat);
    let error = app.map_import.as_ref().unwrap().error().unwrap().to_string();
    assert!(
        error.contains("Mudlet binary map (.dat)") && error.contains("saveJsonMap"),
        "{error}"
    );
    assert!(app.map_import.as_ref().unwrap().prepared().is_none());
    // In another language too.
    wandur_core::l10n::override_thread(Some(wandur_core::l10n::Language::Es));
    read_fixture(&mut app, &ctx, &dat);
    let error = app.map_import.as_ref().unwrap().error().unwrap().to_string();
    assert!(
        error.contains("mapa binario de Mudlet") && error.contains("saveJsonMap"),
        "{error}"
    );
    app.on_exit();
    drop(app);
    wandur_core::l10n::override_thread(None);
    let _ = std::fs::remove_dir_all(&dir);
}
