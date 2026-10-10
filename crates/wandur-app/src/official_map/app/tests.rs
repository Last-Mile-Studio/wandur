//! Official maps through the whole app with real frames: a loopback server sends GMCP
//! `Client.Map`, the session's strip offers the map, Download merges it into the session's map
//! as one undo step (the toast says what changed), and the file is kept in the data directory.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use eframe::App as _;
use egui::{RawInput, Rect, pos2, vec2};
use wandur_core::directory::client::{Fetched, Fetcher};
use wandur_core::map::official::download::Downloaded;

use super::*;

struct Offline;

impl Fetcher for Offline {
    fn get(&self, _url: &str, _limit: u64) -> Result<Fetched, String> {
        Err("offline test".into())
    }
}

const URL: &str = "https://maps.fixture.example/vale/map.xml";
const MAP: &str = r#"<map><areas><area id="1" name="Vale"/></areas><rooms><room id="1" area="1" title="Gate"><coord x="0" y="0" z="0"/><exit direction="east" target="2"/></room><room id="2" area="1" title="Well"><coord x="1" y="0" z="0"/><exit direction="west" target="1"/></room></rooms></map>"#;

fn data_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wandur-{name}-{}", uuid::Uuid::new_v4().simple()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
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

/// A loopback MUD that offers GMCP and names its official map.
fn server() -> (std::net::TcpListener, u16) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

fn serve(listener: std::net::TcpListener) {
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else { return };
        let _ = stream.write_all(&[255, 251, 201]);
        // Wait for DO GMCP.
        let mut buffer = [0u8; 256];
        let deadline = Instant::now() + Duration::from_secs(10);
        let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
        let mut got = Vec::new();
        while Instant::now() < deadline && !got.windows(3).any(|w| w == [255, 253, 201]) {
            if let Ok(n) = stream.read(&mut buffer) {
                got.extend_from_slice(&buffer[..n]);
            }
        }
        let mut out = vec![255, 250, 201];
        out.extend_from_slice(format!(r#"Client.Map {{"url": "{URL}"}}"#).as_bytes());
        out.extend_from_slice(&[255, 240]);
        out.extend_from_slice(b"Welcome to the Vale.\r\n");
        let _ = stream.write_all(&out);
        // Stay connected until the test ends.
        std::thread::sleep(Duration::from_secs(30));
    });
}

#[test]
fn client_map_offers_the_map_and_download_merges_it_as_one_undo_step() {
    wandur_core::l10n::override_thread(Some(wandur_core::l10n::Language::En));
    let dir = data_dir("official-map-app");
    let fetch: crate::official_map::Fetch = Arc::new(|url: &str, _: &wandur_core::map::official::store::Validators| {
        assert_eq!(url, URL);
        Ok(Downloaded::File {
            bytes: MAP.as_bytes().to_vec(),
            etag: Some("\"v1\"".into()),
            last_modified: None,
        })
    });
    let ctx = egui::Context::default();
    let mut app = WandurApp::new(
        &ctx,
        Options {
            data_dir: Some(dir.clone()),
            directory_url: Some("http://127.0.0.1:9".into()),
            fetcher: Some(Arc::new(Offline)),
            official_fetch: Some(fetch),
            vault: Some(Arc::new(wandur_core::login::MemoryVault::new())),
            csharp_vault: Some(Arc::new(wandur_core::login::MemoryVault::new())),
            csharp_folder: Some(dir.join("no-csharp-data")),
            ..Default::default()
        },
    );
    frame(&mut app, &ctx);
    let (listener, port) = server();
    serve(listener);
    app.open(Endpoint::new("127.0.0.1", port), None);
    let id = app.sessions.iter().next().unwrap().tab.id;
    let offer_visible = |a: &WandurApp| {
        a.sessions
            .get(id)
            .and_then(|e| e.tab.official_map.as_ref())
            .is_some_and(|o| o.visible())
    };
    frames_until(&mut app, &ctx, offer_visible);
    assert_eq!(
        app.sessions.get(id).unwrap().tab.official_map.as_ref().unwrap().url,
        URL
    );
    // Download, as the strip's button does.
    app.sessions
        .get_mut(id)
        .unwrap()
        .tab
        .official_map
        .as_mut()
        .unwrap()
        .download();
    frames_until(&mut app, &ctx, |a| a.toast.is_some());
    let rooms = |a: &WandurApp| a.sessions.get(id).unwrap().tab.map.tracker().room_count();
    assert_eq!(rooms(&app), 2);
    let toast = app.toast.clone().unwrap();
    assert_eq!(
        toast.text,
        "Official map for 127.0.0.1:".to_string()
            + &port.to_string()
            + ": 4 added, 0 updated, 0 kept with your edits, 0 removed"
    );
    // The file is kept for the next merge.
    let folder = dir.join(format!("official-maps/endpoint-127_0_0_1-{port}"));
    frames_until(&mut app, &ctx, |a| {
        !a.sessions.get(id).unwrap().tab.official_map.as_ref().unwrap().busy()
    });
    assert_eq!(std::fs::read_to_string(folder.join("map.xml")).unwrap(), MAP);
    let meta = std::fs::read_to_string(folder.join("meta.json")).unwrap();
    assert!(meta.contains(URL) && meta.contains("\\\"v1\\\""), "{meta}");
    assert!(!offer_visible(&app));
    // Undo takes the whole merge back.
    app.push_action(AppAction::UndoToast(toast.serial));
    frame(&mut app, &ctx);
    assert_eq!(rooms(&app), 0);
    app.on_exit();
    drop(app);
    wandur_core::l10n::override_thread(None);
    let _ = std::fs::remove_dir_all(&dir);
}
