//! Headless frames of the whole app (top bar, dock with every panel, status bar) while sessions
//! receive output from the loopback server: time and allocation per frame, split by panel, plus
//! tessellation. This is what the app does on the UI thread per frame, without the GPU.
//!
//! `wandur-bench shell [--label NAME] [--frames N]` (run with `--release`). `WANDUR_BENCH_SKIN`
//! picks the window skin (Fleet, Armored, System; Fleet by default).

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::App as _;
use egui::{RawInput, Rect, pos2, vec2};
use wandur_app::shell::PanelStats;
use wandur_app::sysstat;
use wandur_app::workspace::Tab;
use wandur_app::{Options, WandurApp};
use wandur_core::Endpoint;
use wandur_core::directory::client::{Fetched, Fetcher};
use wandur_core::protocol::parse_gmcp;

use crate::mud_server::{MudServer, WRITE_INTERVAL_MS};

thread_local! {
    /// Time spent in `App::logic` (draining sessions into the grids, timers).
    static LOGIC: std::cell::Cell<Duration> = const { std::cell::Cell::new(Duration::ZERO) };
}

/// No network: every directory or artwork request fails at once.
struct Offline;

impl Fetcher for Offline {
    fn get(&self, _url: &str, _limit: u64) -> Result<Fetched, String> {
        Err("offline bench".into())
    }
}

#[derive(Clone, Copy)]
pub struct Scenario {
    pub name: &'static str,
    pub sessions: usize,
    /// Bytes a second per session (0: idle prompts).
    pub rate: u64,
    /// Fill the Channels and Map panels with data first.
    pub panel_data: bool,
    /// Only the document area (no Workspace, Map or Channels panels).
    pub documents_only: bool,
    /// Show Find a MUD over a loopback directory (300 worlds, 4000x3000 art) instead of sessions.
    pub directory: bool,
    /// Enabled trigger macros on the session's world (0: none, line events stay off).
    pub macros: usize,
    /// The bench script (a regex trigger and a line listener) runs on the session's world.
    pub script: bool,
    /// Scroll the directory's results by a step every frame (with `directory`).
    pub scroll: bool,
    /// The flood carries a Legends of the Jedi-like room every 20 lines (GMCP `Room.Info`
    /// without a description, and its text), so the map fills descriptions from the text.
    pub lotj_rooms: bool,
}

pub const SCENARIOS: [Scenario; 12] = [
    Scenario {
        name: "directory page, 300 worlds with art (frames forced)",
        sessions: 0,
        rate: 0,
        panel_data: false,
        documents_only: false,
        directory: true,
        macros: 0,
        script: false,
        scroll: false,
        lotj_rooms: false,
    },
    Scenario {
        name: "flood 1 MB/s, panels empty (harness-like)",
        sessions: 1,
        rate: 1_000_000,
        panel_data: false,
        documents_only: false,
        directory: false,
        macros: 0,
        script: false,
        scroll: false,
        lotj_rooms: false,
    },
    Scenario {
        name: "flood 1 MB/s, 1,000 macro triggers, panels empty",
        sessions: 1,
        rate: 1_000_000,
        panel_data: false,
        documents_only: false,
        directory: false,
        macros: 1_000,
        script: false,
        scroll: false,
        lotj_rooms: false,
    },
    Scenario {
        name: "flood 1 MB/s, a script trigger, panels empty",
        sessions: 1,
        rate: 1_048_576,
        panel_data: false,
        documents_only: false,
        directory: false,
        macros: 0,
        script: true,
        scroll: false,
        lotj_rooms: false,
    },
    Scenario {
        name: "flood 1 MB/s, panels with data",
        sessions: 1,
        rate: 1_000_000,
        panel_data: true,
        documents_only: false,
        directory: false,
        macros: 0,
        script: false,
        scroll: false,
        lotj_rooms: false,
    },
    Scenario {
        name: "flood 1 MB/s, LotJ rooms with Room.Info and no description",
        sessions: 1,
        rate: 1_000_000,
        panel_data: false,
        documents_only: false,
        directory: false,
        macros: 0,
        script: false,
        scroll: false,
        lotj_rooms: true,
    },
    Scenario {
        name: "flood 1 MB/s, session tab only",
        sessions: 1,
        rate: 1_000_000,
        panel_data: false,
        documents_only: true,
        directory: false,
        macros: 0,
        script: false,
        scroll: false,
        lotj_rooms: false,
    },
    Scenario {
        name: "4 sessions x 250 KB/s, panels with data",
        sessions: 4,
        rate: 250_000,
        panel_data: true,
        documents_only: false,
        directory: false,
        macros: 0,
        script: false,
        scroll: false,
        lotj_rooms: false,
    },
    Scenario {
        name: "idle session, panels with data (frames forced)",
        sessions: 1,
        rate: 0,
        panel_data: true,
        documents_only: false,
        directory: false,
        macros: 0,
        script: false,
        scroll: false,
        lotj_rooms: false,
    },
    Scenario {
        name: "idle session, session tab only (frames forced)",
        sessions: 1,
        rate: 0,
        panel_data: false,
        documents_only: true,
        directory: false,
        macros: 0,
        script: false,
        scroll: false,
        lotj_rooms: false,
    },
    Scenario {
        name: "directory page, 300 worlds, scrolling (frames forced)",
        sessions: 0,
        rate: 0,
        panel_data: false,
        documents_only: false,
        directory: true,
        macros: 0,
        script: false,
        scroll: true,
        lotj_rooms: false,
    },
    Scenario {
        name: "directory page alone, 300 worlds, scrolling (frames forced)",
        sessions: 0,
        rate: 0,
        panel_data: false,
        documents_only: true,
        directory: true,
        macros: 0,
        script: false,
        scroll: true,
        lotj_rooms: false,
    },
];

pub struct ShellResult {
    pub scenario: &'static str,
    pub frame_ms: f64,
    pub frame_p95_ms: f64,
    pub frame_max_ms: f64,
    pub frame_kb: f64,
    pub tess_ms: f64,
    pub tess_kb: f64,
    pub logic_ms: f64,
    pub panels: PanelStats,
    pub frames: u64,
    /// Process CPU time over wall time while measuring, in percent of one core (the app's
    /// threads and the in-process loopback server).
    pub cpu_percent: f64,
}

fn fill_panels(app: &mut WandurApp) {
    for entry in app.sessions_mut().iter_mut() {
        let tab = &mut entry.tab;
        for i in 0..600u64 {
            let (channel, speaker) = match i % 4 {
                0 => ("gossip", "Ann"),
                1 => ("newbie", "Bob"),
                2 => ("tell", "Cy"),
                _ => ("ooc", "Dee"),
            };
            let text = format!(
                "\u{1b}[1;35m[{channel}]\u{1b}[0m {speaker}: message {i} with \u{1b}[33msome colour\u{1b}[0m and a longer tail of words to wrap"
            );
            tab.channels.log.add(channel, speaker, &text, 1_700_000_000 + i * 7);
        }
        let mut walked = None;
        for i in 0..80u32 {
            let json = format!(
                r#"Room.Info {{"num":{n},"name":"Bench room {n}","area":"Bench Town","environment":"urban","exits":{{"e":{e},"w":{w},"n":{u}}}}}"#,
                n = 1000 + i,
                e = 1001 + i,
                w = 999 + i,
                u = 5000 + i
            );
            if let Some(room) = parse_gmcp(json.as_bytes()).and_then(|m| wandur_core::map::decode::from_gmcp(&m)) {
                let gate = tab.walk_gate();
                if let Some(direction) = walked {
                    tab.map.track_command(direction, false, std::time::Instant::now());
                }
                tab.map.observe_room(room, gate, std::time::Instant::now());
            }
            walked = Some("east");
        }
    }
}

/// `n` enabled triggers over the flood's words: mostly pairs with a number that never occur,
/// a third ignoring case, some anchored, and one ("lantern river") that does occur, so the send
/// path and the rate limit are exercised too. More than a world may save (64): this measures
/// the engine, not the library.
pub fn bench_macros(n: usize) -> Vec<wandur_core::db::scripts::LibraryEntry> {
    use wandur_core::db::scripts::LibraryEntry;
    use wandur_core::macros::{MacroDefinition, MacroKind, MacroMatch};
    const WORDS: [&str; 12] = [
        "lantern", "river", "stone", "quiet", "forest", "tower", "ember", "glass", "harbor", "willow", "copper",
        "north",
    ];
    (0..n)
        .map(|i| {
            let pattern = if i == n - 1 {
                "lantern river".to_string()
            } else {
                format!("{} {} {i}", WORDS[i % 12], WORDS[(i / 12) % 12])
            };
            let mut def = MacroDefinition::new(MacroKind::Trigger, &pattern, "look");
            if i % 3 == 0 {
                def = def.ignoring_case();
            }
            if i % 7 == 0 {
                def = def.with_match(MacroMatch::StartsWith);
            } else if i % 11 == 0 {
                def = def.with_match(MacroMatch::Exact);
            }
            let mut entry = LibraryEntry::new_macro(&format!("m{i}"), def);
            entry.enabled = true;
            entry
        })
        .collect()
}

pub fn run_scenario(mut s: Scenario, frames: usize) -> ShellResult {
    // WANDUR_BENCH_MACROS=N: N triggers instead of the scenario's 1,000 (to split the cost).
    if s.macros > 0
        && let Some(n) = std::env::var("WANDUR_BENCH_MACROS").ok().and_then(|v| v.parse().ok())
    {
        s.macros = n;
    }
    WRITE_INTERVAL_MS.store(2, std::sync::atomic::Ordering::Relaxed);
    crate::mud_server::LOTJ_ROOMS.store(s.lotj_rooms, std::sync::atomic::Ordering::Relaxed);
    let server = MudServer::start(0, s.rate, false).expect("loopback server");
    let endpoint = Endpoint::new("127.0.0.1", server.port);
    let dir = std::env::temp_dir().join(format!("wandur-shell-bench-{}-{}", std::process::id(), s.name.len()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("bench data directory");
    // WANDUR_BENCH_SUGGESTIONS=0: completion learning off (Settings > Input), to split its cost.
    // WANDUR_BENCH_HISTORY=0: session history off (Settings > General); on by default, as in the app.
    // WANDUR_BENCH_WRAP=0: word wrapping off (Settings > Terminal), the grid's own rows drawn.
    let off = |name: &str| std::env::var(name).is_ok_and(|v| v == "0");
    let mut settings = serde_json::Map::new();
    if off("WANDUR_BENCH_SUGGESTIONS") {
        settings.insert("composer_suggestions".into(), false.into());
    }
    if off("WANDUR_BENCH_HISTORY") {
        settings.insert("history_enabled".into(), false.into());
    }
    if off("WANDUR_BENCH_WRAP") {
        settings.insert("wrap_words".into(), false.into());
    }
    if !settings.is_empty() {
        let _ = std::fs::write(
            dir.join("settings.json"),
            serde_json::Value::Object(settings).to_string(),
        );
    }
    if s.documents_only {
        let _ = std::fs::write(
            dir.join("layout.json"),
            r#"{"version":1,"root":{"kind":"leaf","tabs":["directory"],"active":0}}"#,
        );
    }
    let ctx = egui::Context::default();
    let directory = s.directory.then(|| {
        let (jpeg, png) = crate::directory_server::artwork(std::path::Path::new(".superpowers/perf/art"), 4000, 3000);
        let json = crate::directory_server::listing_json(300, true, &crate::directory_server::now_rfc3339());
        crate::directory_server::DirectoryServer::start(0, json, jpeg, png).expect("loopback directory")
    });
    let options = Options {
        connect: vec![endpoint; s.sessions],
        data_dir: Some(dir.clone()),
        directory_url: Some(
            directory
                .as_ref()
                .map_or_else(|| "http://127.0.0.1:9".into(), |d| d.address()),
        ),
        theme: Some("Hull".into()),
        // WANDUR_BENCH_FONT_SIZE=13: the text size (the settings default otherwise; it was 13
        // before t16 and is the C# 15 since), for like-for-like comparisons.
        font_size: std::env::var("WANDUR_BENCH_FONT_SIZE")
            .ok()
            .and_then(|v| v.parse().ok()),
        // WANDUR_BENCH_SKIN=Fleet|Armored|System: the window skin (Fleet by default).
        skin: std::env::var("WANDUR_BENCH_SKIN").ok(),
        // WANDUR_BENCH_SESSION_VIEW=map|split: the first session on its Map page (the full map
        // and the output strip) or Play and Map side by side; Play otherwise.
        session_view: match std::env::var("WANDUR_BENCH_SESSION_VIEW").as_deref() {
            Ok("map") => Some(wandur_app::app::SessionViewScene::Map),
            Ok("split") => Some(wandur_app::app::SessionViewScene::Split),
            _ => None,
        },
        // The loopback directory goes through the real HTTP client; nothing else is reachable.
        fetcher: if directory.is_some() {
            None
        } else {
            Some(Arc::new(Offline))
        },
        show: directory.as_ref().map(|_| "directory".into()),
        worlds: (s.macros > 0 || s.script).then(|| {
            vec![wandur_core::settings::SavedWorld {
                name: "Bench".into(),
                host: "127.0.0.1".into(),
                port: server.port,
                ..Default::default()
            }]
        }),
        macros: (s.macros > 0 || s.script).then(|| {
            let mut library = bench_macros(s.macros);
            if s.script {
                library.push(wandur_core::db::scripts::LibraryEntry {
                    id: wandur_core::db::scripts::new_id(),
                    name: "Bench script".into(),
                    source: crate::micro::BENCH_SCRIPT.into(),
                    enabled: true,
                    ..wandur_core::db::scripts::LibraryEntry::default()
                });
            }
            library
        }),
        ..Default::default()
    };
    let mut app = WandurApp::new(&ctx, options);
    let mut frame = eframe::Frame::_new_kittest();
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1600.0, 1000.0))),
        ..Default::default()
    };
    let ppp = 2.0;
    ctx.set_pixels_per_point(ppp);
    let one = |app: &mut WandurApp, frame: &mut eframe::Frame| -> (Duration, u64, Duration, u64) {
        let a0 = sysstat::allocated_bytes();
        let t0 = Instant::now();
        let mut out = ctx.run_ui(input(), |ui| {
            let l0 = Instant::now();
            app.logic(ui.ctx(), frame);
            LOGIC.with(|c| c.set(c.get() + l0.elapsed()));
            app.ui(ui, frame);
        });
        let ui_time = t0.elapsed();
        let a1 = sysstat::allocated_bytes();
        out.textures_delta.clear();
        let t1 = Instant::now();
        let prims = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        std::hint::black_box(&prims);
        drop(prims);
        let tess = t1.elapsed();
        (ui_time, a1 - a0, tess, sysstat::allocated_bytes() - a1)
    };
    // Connect and settle.
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        one(&mut app, &mut frame);
        if s.directory {
            if app.catalog_len() > 0 {
                break;
            }
        } else if app.sessions_mut().connected() >= s.sessions {
            if s.panel_data {
                fill_panels(&mut app);
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let warm = Instant::now();
    let settle = if s.directory { 6 } else { 2 };
    while warm.elapsed() < Duration::from_secs(settle) {
        one(&mut app, &mut frame);
        std::thread::sleep(Duration::from_millis(33));
    }
    app.panel_stats_mut().reset();
    LOGIC.with(|c| c.set(Duration::ZERO));
    let mut times = Vec::with_capacity(frames);
    let (mut alloc, mut tess, mut tess_alloc) = (0u64, Duration::ZERO, 0u64);
    let (cpu_start, wall_start) = (sysstat::cpu_time(), Instant::now());
    for i in 0..frames {
        if s.scroll {
            // About a card and a half a frame, down then back up, so new cards keep arriving.
            let step = (i % 120) as f32;
            let offset = if step < 60.0 { step } else { 120.0 - step } * 260.0;
            app.scroll_directory(offset);
        }
        let start = Instant::now();
        let (t, a, tt, ta) = one(&mut app, &mut frame);
        times.push(t.as_secs_f64() * 1000.0);
        alloc += a;
        tess += tt;
        tess_alloc += ta;
        // 30 output frames a second, as the app's default cap.
        let spent = start.elapsed();
        if spent < Duration::from_millis(33) {
            std::thread::sleep(Duration::from_millis(33) - spent);
        }
    }
    let cpu_percent =
        (sysstat::cpu_time() - cpu_start).as_secs_f64() / wall_start.elapsed().as_secs_f64().max(1e-9) * 100.0;
    let n = frames as f64;
    let panels = app.panel_stats_mut().clone();
    if s.script {
        for entry in app.sessions_mut().iter() {
            let script = &entry.tab.scripts.entries[0];
            eprintln!(
                "script: running {}, error {:?}, lines {}",
                script.is_running(),
                script.error,
                entry.tab.terminal.lines_total()
            );
        }
    }
    if let Some(entry) = app.sessions_mut().iter().next() {
        eprintln!(
            "map: {} rooms ({} with a description), page {:?}, side by side {}",
            entry.tab.map.tracker().room_count(),
            entry
                .tab
                .map
                .tracker()
                .rooms()
                .filter(|r| !r.description.is_empty())
                .count(),
            entry.view.page,
            entry.view.split
        );
    }
    if s.macros > 0 {
        for entry in app.sessions_mut().iter() {
            eprintln!(
                "macros: {} rules active {}, lines seen by triggers {}, commands sent {}, error {:?}",
                entry.tab.macros.library().len(),
                entry.tab.macros.is_active(),
                entry.tab.terminal.lines_total(),
                entry.tab.macro_commands_sent,
                entry.tab.macros.error
            );
        }
    }
    drop(app);
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
    let mean = times.iter().sum::<f64>() / n;
    times.sort_by(f64::total_cmp);
    let p95 = times[((times.len() as f64 * 0.95) as usize).min(times.len() - 1)];
    ShellResult {
        scenario: s.name,
        frame_ms: mean,
        frame_p95_ms: p95,
        frame_max_ms: times.last().copied().unwrap_or(0.0),
        frame_kb: alloc as f64 / 1024.0 / n,
        tess_ms: tess.as_secs_f64() * 1000.0 / n,
        tess_kb: tess_alloc as f64 / 1024.0 / n,
        logic_ms: LOGIC.with(|c| c.get()).as_secs_f64() * 1000.0 / n,
        panels,
        frames: frames as u64,
        cpu_percent,
    }
}

const NAMES: [&str; Tab::KINDS] = ["Workspace", "Find a MUD", "Map", "Channels", "Session", "Saved worlds"];

pub fn run(label: &str, frames: usize, filter: Option<&str>) {
    let mut table = String::from(
        "| Scenario | UI ms/frame (mean, p95, max) | of which logic ms | UI KB/frame | Tessellate ms, KB/frame | Process CPU % | Per panel: ms, KB per frame |\n|---|---:|---:|---:|---:|---:|---|\n",
    );
    for s in SCENARIOS {
        if filter.is_some_and(|f| !s.name.contains(f)) {
            continue;
        }
        let r = run_scenario(s, frames);
        let n = r.frames as f64;
        let mut panels = String::new();
        for (i, (t, bytes, calls)) in r.panels.panels.iter().enumerate() {
            if *calls == 0 {
                continue;
            }
            let _ = write!(
                panels,
                "{} {:.3} ms {:.0} KB; ",
                NAMES[i],
                t.as_secs_f64() * 1000.0 / n,
                *bytes as f64 / 1024.0 / n
            );
        }
        let line = format!(
            "| {} | {:.3}, {:.3}, {:.1} | {:.3} | {:.0} | {:.3}, {:.0} | {:.1} | {} |",
            r.scenario,
            r.frame_ms,
            r.frame_p95_ms,
            r.frame_max_ms,
            r.logic_ms,
            r.frame_kb,
            r.tess_ms,
            r.tess_kb,
            r.cpu_percent,
            panels.trim_end_matches("; ")
        );
        println!("{line}");
        let _ = writeln!(table, "{line}");
    }
    let dir = std::path::Path::new(".superpowers/perf");
    if std::fs::create_dir_all(dir).is_ok() {
        let _ = std::fs::write(dir.join(format!("shell-{label}.md")), &table);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stall probe: the whole app under a 1 MB/s flood with every panel holding data must never
    /// keep the UI thread for long. The bound is loose because tests run unoptimized (release
    /// frames take about 2 ms); it catches blocking work (file or network I/O, decoding) that
    /// lands on the UI thread.
    #[test]
    #[ignore = "timing-sensitive, unreliable on a busy machine: run with --include-ignored, or the ship profile"]
    fn no_long_ui_thread_stalls_under_a_flood() {
        for name in [
            "flood 1 MB/s, panels with data",
            "flood 1 MB/s, 1,000 macro triggers, panels empty",
        ] {
            let s = SCENARIOS.into_iter().find(|s| s.name == name).unwrap();
            let r = run_scenario(s, 45);
            assert!(r.frame_max_ms < 250.0, "{name}: longest frame {:.1} ms", r.frame_max_ms);
            assert!(r.frame_p95_ms < 120.0, "{name}: p95 {:.1} ms", r.frame_p95_ms);
        }
    }
}
