//! The Map panel per frame with a large map: 2,000 rooms (a 50 by 40 block, every room linked
//! east and north), drawn headless and tessellated, fitted (every room on screen, no labels) and
//! at zoom 1 (labels, a few dozen rooms on screen), and the same in grid mode.
//!
//! `WANDUR_MICRO_ONLY_MAP=1 wandur-bench micro --label NAME` (run with `--release`).

use std::time::Instant;

use egui::{RawInput, Rect, pos2, vec2};
use wandur_app::map_view::{self, MapContext, MapViewState};
use wandur_app::session_tab::{SessionTab, TabOptions};
use wandur_app::sysstat;
use wandur_app::theme::Theme;
use wandur_core::map::{MapAreaSettings, MapLink, MapRoom, MapSession, MapSnapshot};

use crate::micro::Row;

/// A block of `n` rooms, 50 wide, linked east and north, one area, one floor.
pub fn block(n: usize) -> MapSnapshot {
    let mut rooms = Vec::with_capacity(n);
    let mut links = Vec::new();
    for i in 0..n {
        let (x, y) = ((i % 50) as f64, (i / 50) as f64);
        let mut room = MapRoom::new(
            &format!("s:{i}"),
            &format!("Room {i}"),
            "",
            Some("Block"),
            x,
            y,
            0.0,
            false,
        )
        .with_server_id(&i.to_string());
        room.environment = Some(["road", "forest", "city", "water"][i % 4].into());
        room.known_exits = vec!["east".into(), "north".into()];
        rooms.push(room);
        if i % 50 != 49 && i + 1 < n {
            links.push(MapLink::new(&format!("s:{i}"), &format!("s:{}", i + 1), "east", true));
        }
        if i + 50 < n {
            links.push(MapLink::new(&format!("s:{i}"), &format!("s:{}", i + 50), "north", true));
        }
    }
    MapSnapshot {
        current_room_id: None,
        ..MapSnapshot::of(rooms, links)
    }
}

/// The text room observer over the flood (it reads every line a world without room data sends).
pub fn text_observer(rows: &mut Vec<Row>) {
    let chunks = crate::generator::AnsiGenerator::new(42, true).chunks(15_000, 4096);
    let bytes: usize = chunks.iter().map(|c| c.len()).sum();
    let texts = chunks;
    let mut times = Vec::new();
    let runs = std::env::var("WANDUR_OBSERVER_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(41);
    for _ in 0..runs {
        let mut observer = wandur_core::map::text::TextRoomObserver::new();
        let start = Instant::now();
        let mut rooms = 0;
        for t in &texts {
            rooms += observer.feed(t).len();
        }
        times.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(rooms, 0);
    }
    times.sort_by(f64::total_cmp);
    rows.push(Row {
        name: "Text room observer, 1 MB of flood".into(),
        unit: "1 MB",
        ms: times[times.len() / 2] * 1_000_000.0 / bytes as f64,
        kb: 0.0,
        note: format!("{bytes} bytes in {} chunks", texts.len()),
    });
}

/// The map session over a Legends of the Jedi-like flood (the generated text with a room every
/// 20 lines, its `Room.Info` without a description before or after its text), as a session tab
/// feeds it: the text observer, the protocol rooms and the descriptions read from the text.
pub fn lotj_session(rows: &mut Vec<Row>) {
    use wandur_core::map::{OptionState, WalkGate};
    use wandur_core::protocol::parse_gmcp;
    // The flood, cut into text pieces and the GMCP messages between them.
    let mut generator = crate::generator::AnsiGenerator::new(42, true);
    let mut pieces: Vec<(String, Option<wandur_core::protocol::GmcpMessage>)> = Vec::new();
    let mut bytes = 0usize;
    let mut text = String::new();
    let mut room = 0u32;
    while bytes < 1_000_000 {
        for _ in 0..crate::lotj_page::LINES_PER_ROOM {
            generator.next_line(&mut text);
        }
        let data = crate::lotj_page::room(room);
        room += 1;
        let start = data
            .windows(3)
            .position(|w| w == [255, 250, 201])
            .expect("a GMCP message");
        let end = data.windows(2).position(|w| w == [255, 240]).expect("its end");
        text.push_str(&String::from_utf8_lossy(&data[..start]));
        bytes += text.len() + data.len() - start;
        pieces.push((std::mem::take(&mut text), parse_gmcp(&data[start + 3..end])));
        text.push_str(&String::from_utf8_lossy(&data[end + 2..]));
    }
    pieces.push((text, None));
    let gate = WalkGate {
        connected: true,
        ..WalkGate::default()
    };
    let mut times = Vec::new();
    let mut described = 0;
    for _ in 0..21 {
        let mut map = MapSession::new();
        map.set_gmcp(OptionState::Enabled);
        let start = Instant::now();
        for (text, message) in &pieces {
            let now = Instant::now();
            map.track_output(text, gate, now);
            if let Some(room) = message.as_ref().and_then(wandur_core::map::decode::from_gmcp) {
                map.observe_room(room, gate, now);
            }
        }
        times.push(start.elapsed().as_secs_f64() * 1000.0);
        described = map.tracker().rooms().filter(|r| !r.description.is_empty()).count();
    }
    times.sort_by(f64::total_cmp);
    rows.push(Row {
        name: "Map session, 1 MB of LotJ-like flood with Room.Info".into(),
        unit: "1 MB",
        ms: times[times.len() / 2] * 1_000_000.0 / bytes as f64,
        kb: 0.0,
        note: format!("{bytes} bytes, {room} rooms; {described} of 200 rooms described"),
    });
}

/// Room terrain inference's cost on the UI thread with 10,000 rooms without terrain: the scan
/// that queues rooms (text copied, revisions compared, no hashing) and applying 512 results;
/// against the C# way, which computed every room's inference key (SHA-256) on the UI thread and
/// now runs on the worker.
pub fn inference(rows: &mut Vec<Row>) {
    use std::sync::Arc;
    use wandur_core::classify::{
        InferenceResult, InferenceWorker, RoomClassificationService, RoomEnvironmentPrediction,
    };
    use wandur_core::map::RoomMapTracker;
    let description = "Tall oaks crowd the road and moss covers every stone. ".repeat(4);
    let rooms: Vec<MapRoom> = (0..10_000)
        .map(|i| {
            MapRoom::new(
                &format!("s:{i}"),
                &format!("Room {i}"),
                &description,
                Some("Block"),
                (i % 100) as f64,
                (i / 100) as f64,
                0.0,
                false,
            )
        })
        .collect();
    let snapshot = MapSnapshot::of(rooms, Vec::new());
    let median = |mut times: Vec<f64>| {
        times.sort_by(f64::total_cmp);
        times[times.len() / 2]
    };
    let service = Arc::new(RoomClassificationService::for_testing(Arc::new(
        wandur_app::scene::KeywordClassifier,
    )));
    let mut scan = Vec::new();
    for _ in 0..21 {
        let tracker = RoomMapTracker::from_snapshot(snapshot.clone());
        // A worker whose thread never gets to run the queue before the measurement ends.
        let mut worker = InferenceWorker::spawn(Arc::clone(&service), Box::new(|| {})).expect("thread");
        let start = Instant::now();
        let queued = worker.schedule(&tracker, "bench", 0.5);
        scan.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(queued, wandur_core::classify::worker::QUEUE_LIMIT);
        worker.reset();
    }
    rows.push(Row {
        name: "Inference scan, 10,000 rooms (UI thread)".into(),
        unit: "scan",
        ms: median(scan),
        kb: 0.0,
        note: "rooms copied and revisions compared, 512 queued, no hashing".into(),
    });
    // The whole map through the worker with the keyword stand-in, as the session runs it: every
    // frame applies what is done and rescans when the worker asks.
    {
        let mut tracker = RoomMapTracker::from_snapshot(snapshot.clone());
        let mut worker = InferenceWorker::spawn(Arc::clone(&service), Box::new(|| {})).expect("thread");
        let started = Instant::now();
        let (mut total, mut worst, mut frames) = (0.0f64, 0.0f64, 0);
        worker.schedule(&tracker, "fixture-1", 0.5);
        while tracker.rooms().any(|r| r.inferred_key.is_none()) && started.elapsed().as_secs() < 60 {
            std::thread::sleep(std::time::Duration::from_millis(1));
            let start = Instant::now();
            let (_, rescan) = worker.apply(&mut tracker);
            if rescan {
                worker.schedule(&tracker, "fixture-1", 0.5);
            }
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            total += ms;
            worst = worst.max(ms);
            frames += 1;
        }
        // One more scan with every room checked: the worst case of a rescan.
        let start = Instant::now();
        let queued = worker.schedule(&tracker, "fixture-1", 0.5);
        let idle = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(queued, 0);
        rows.push(Row {
            name: "Inference of 10,000 rooms end to end (UI thread per frame)".into(),
            unit: "frame",
            ms: worst,
            kb: 0.0,
            note: format!(
                "worst frame; {frames} frames, {total:.1} ms UI time in all, {:.0} ms wall; a scan with every room checked {idle:.3} ms",
                started.elapsed().as_secs_f64() * 1000.0
            ),
        });
    }
    let mut keys = Vec::new();
    for _ in 0..11 {
        let tracker = RoomMapTracker::from_snapshot(snapshot.clone());
        let start = Instant::now();
        let needing = tracker.rooms_needing_inference("bench").len();
        keys.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(needing, 10_000);
    }
    rows.push(Row {
        name: "Inference keys, 10,000 rooms (C# UI thread, here the worker)".into(),
        unit: "scan",
        ms: median(keys),
        kb: 0.0,
        note: "SHA-256 of every room's cleaned text, as C# RoomsNeedingInference".into(),
    });
    let mut apply = Vec::new();
    for _ in 0..21 {
        let mut tracker = RoomMapTracker::from_snapshot(snapshot.clone());
        let results: Vec<InferenceResult> = tracker
            .rooms()
            .take(512)
            .map(|r| InferenceResult {
                id: r.id.clone(),
                name: r.name.clone(),
                description: r.description.clone(),
                key: "0".repeat(64),
                model_version: "bench".into(),
                prediction: Some(RoomEnvironmentPrediction {
                    environment: "forest".into(),
                    confidence: 0.87,
                    model_version: "bench".into(),
                }),
            })
            .collect();
        let start = Instant::now();
        let applied = results.iter().filter(|r| tracker.apply_inference_result(r)).count();
        apply.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(applied, 512);
    }
    rows.push(Row {
        name: "Apply 512 inference results (UI thread)".into(),
        unit: "512",
        ms: median(apply),
        kb: 0.0,
        note: "text compared, fields stored".into(),
    });
}

pub fn map(rows: &mut Vec<Row>) {
    text_observer(rows);
    lotj_session(rows);
    inference(rows);
    let ctx = egui::Context::default();
    wandur_app::fonts::install(&ctx);
    let theme = Theme::preset("Hull");
    for (name, fitted, grid) in [
        ("2,000 rooms fitted on screen", true, false),
        ("2,000 rooms at zoom 1 with labels", false, false),
        ("2,000 rooms fitted, grid mode", true, true),
    ] {
        let mut tab = SessionTab::demo(1, &TabOptions::default());
        tab.map = MapSession::with_map(block(2_000));
        tab.map.tracker_mut().set_current_room("s:1025");
        if grid {
            tab.map
                .tracker_mut()
                .set_area_settings(MapAreaSettings::new("Block", true));
        }
        let mut state = MapViewState::default();
        let mut auto = true;
        let frames = 200;
        let mut draws = Vec::new();
        let mut builds = Vec::new();
        let mut allocated = Vec::new();
        let mut painted = 0;
        for frame in 0..frames + 20 {
            if frame == 2 {
                if fitted {
                    state.fit_now(tab.map.tracker());
                } else {
                    state.zoom = 1.0;
                }
            }
            let a0 = sysstat::allocated_bytes();
            let start = Instant::now();
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let mut cx = MapContext::new(&theme, &mut auto);
                    map_view::show(ui, Some(&mut tab), &mut state, &mut cx);
                });
            });
            out.textures_delta.clear();
            let built = start.elapsed().as_secs_f64() * 1000.0;
            let _ = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
            if frame >= 20 {
                builds.push(built);
                draws.push(start.elapsed().as_secs_f64() * 1000.0);
                allocated.push((sysstat::allocated_bytes() - a0) as f64 / 1024.0);
                painted = state.painted;
            }
        }
        draws.sort_by(f64::total_cmp);
        builds.sort_by(f64::total_cmp);
        let build = builds[builds.len() / 2];
        let mean = draws.iter().sum::<f64>() / draws.len() as f64;
        rows.push(Row {
            name: format!("Map frame, {name}"),
            unit: "one frame",
            ms: draws[draws.len() / 2],
            kb: allocated.iter().sum::<f64>() / allocated.len() as f64,
            note: format!(
                "whole panel, draw and tessellate: mean {mean:.3} ms, p95 {:.3} ms (shapes built {build:.3} ms); {painted} rooms drawn",
                draws[draws.len() * 95 / 100]
            ),
        });
    }
}
