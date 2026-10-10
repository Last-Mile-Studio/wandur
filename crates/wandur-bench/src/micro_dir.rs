//! In-process measurements of the directory and the artwork pipeline, named after the C# `micro`
//! rows they compare with (`docs/perf.md` in the C# repository).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use wandur_app::artwork::{self, ArtLoader, ArtRequest, ArtState, Target, decode};
use wandur_app::directory_view::{DirectoryView, art_request};
use wandur_app::sysstat;
use wandur_core::directory::client::{Fetcher, HttpFetcher, MAX_ART_BYTES};
use wandur_core::directory::{DirectoryStatus, Sort, catalog::Catalog, snapshot};
use wandur_core::settings::Settings;

use crate::directory_server::{self, DirectoryServer};
use crate::micro::Row;

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Run `f` while a thread samples the live heap; returns (ms, KB allocated, peak live rise in KB).
fn measure(f: &mut dyn FnMut()) -> (f64, f64, f64) {
    let base = sysstat::live_heap_bytes();
    let peak = Arc::new(AtomicU64::new(base));
    let stop = Arc::new(AtomicBool::new(false));
    let sampler = {
        let (peak, stop) = (Arc::clone(&peak), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                peak.fetch_max(sysstat::live_heap_bytes(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_micros(100));
            }
        })
    };
    let a0 = sysstat::allocated_bytes();
    let t = Instant::now();
    f();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let kb = (sysstat::allocated_bytes() - a0) as f64 / 1024.0;
    stop.store(true, Ordering::Relaxed);
    let _ = sampler.join();
    let rise = peak.load(Ordering::Relaxed).saturating_sub(base) as f64 / 1024.0;
    (ms, kb, rise)
}

pub fn directory(rows: &mut Vec<Row>) {
    let json = directory_server::listing_json(500, false, &directory_server::now_rfc3339());
    let parse_start = Instant::now();
    let catalog = Arc::new(Catalog::new(snapshot::parse(&json).unwrap()));
    let parse_ms = parse_start.elapsed().as_secs_f64() * 1000.0;
    rows.push(Row {
        name: format!(
            "Directory parse and index, {} worlds ({} KB JSON)",
            catalog.len(),
            json.len() / 1024
        ),
        unit: "load",
        ms: parse_ms,
        kb: 0.0,
        note: "background thread".into(),
    });
    let settings = Settings::default();
    for (label, varied) in [("", false), (" (varied listings)", true)] {
        let catalog = if varied {
            let json = directory_server::listing_json(500, true, &directory_server::now_rfc3339());
            Arc::new(Catalog::new(snapshot::parse(&json).unwrap()))
        } else {
            Arc::clone(&catalog)
        };
        let mut status = DirectoryStatus {
            catalog: Some(Arc::clone(&catalog)),
            revision: 1,
            ..Default::default()
        };
        let mut view = DirectoryView::default();
        view.refresh(&status, &settings, 0);
        let searches = ["", "bench", "world 4", "zzz"];
        let mut times = Vec::new();
        let mut kbs = Vec::new();
        for n in 0..200 {
            view.query.search = searches[n % searches.len()].into();
            let a0 = sysstat::allocated_bytes();
            let t = Instant::now();
            view.refresh(&status, &settings, 0);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let kb = (sysstat::allocated_bytes() - a0) as f64 / 1024.0;
            times.push(ms);
            kbs.push(kb);
        }
        rows.push(Row {
            name: format!("Directory filter, {} worlds, search text changes{label}", catalog.len()),
            unit: "query",
            ms: median(times),
            kb: median(kbs),
            note: "C# 2.256 ms, 772 KB".into(),
        });
        view.query.search.clear();
        let mut times = Vec::new();
        let mut kbs = Vec::new();
        for i in 0..200 {
            view.query.sort = Sort::ALL[i % 6];
            let a0 = sysstat::allocated_bytes();
            let t = Instant::now();
            view.refresh(&status, &settings, 0);
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            kbs.push((sysstat::allocated_bytes() - a0) as f64 / 1024.0);
        }
        rows.push(Row {
            name: format!("Directory sort, {} worlds, cycling 6 sorts{label}", catalog.len()),
            unit: "query",
            ms: median(times),
            kb: median(kbs),
            note: "C# 0.573 ms, 75 KB".into(),
        });
        status.revision += 1;
        std::hint::black_box(&status);
    }
}

pub fn artwork(rows: &mut Vec<Row>) {
    let dir = std::path::Path::new(".superpowers/perf/art");
    let (jpeg, png) = directory_server::artwork(dir, 4000, 3000);
    let plate = Target {
        width: 800,
        height: 320,
        cover: true,
    };
    let thumb = Target {
        width: 80,
        height: 60,
        cover: false,
    };
    let cases: [(&str, &[u8], Target, &str); 3] = [
        (
            "Thumbnail 4000x3000 JPEG to 800x320 cover",
            &jpeg,
            plate,
            "C# 15.8 ms, peak RSS rise 15 MB",
        ),
        (
            "Thumbnail 4000x3000 PNG to 800x320 cover",
            &png,
            plate,
            "C# 89.4 ms, peak RSS rise 68 MB",
        ),
        (
            "Thumbnail 4000x3000 JPEG to 80x60",
            &jpeg,
            thumb,
            "C# 9.3 ms, peak RSS rise 8 MB",
        ),
    ];
    for (name, bytes, target, note) in cases {
        let mut times = Vec::new();
        let mut kbs = Vec::new();
        let mut peaks = Vec::new();
        for _ in 0..5 {
            let (ms, kb, peak) = measure(&mut || {
                std::hint::black_box(decode::decode_near(bytes, target).unwrap());
            });
            times.push(ms);
            kbs.push(kb);
            peaks.push(peak);
        }
        rows.push(Row {
            name: name.into(),
            unit: "image",
            ms: median(times),
            kb: median(kbs),
            note: format!(
                "peak live heap rise {:.1} MB; source {} KB; {note}",
                median(peaks) / 1024.0,
                bytes.len() / 1024
            ),
        });
    }
    // For comparison: decoding at full size, then resizing (what near-size decoding avoids).
    let (ms, kb, peak) = measure(&mut || {
        let img = image::load_from_memory(&jpeg).unwrap().to_rgba8();
        std::hint::black_box(image::imageops::resize(
            &img,
            800,
            600,
            image::imageops::FilterType::Triangle,
        ));
    });
    rows.push(Row {
        name: "Same JPEG decoded at full size then resized (not used; for comparison)".into(),
        unit: "image",
        ms,
        kb,
        note: format!("peak live heap rise {:.1} MB", peak / 1024.0),
    });

    // Loading many large artwork files: 300 cards through the real pipeline over loopback HTTP.
    let json = directory_server::listing_json(300, false, &directory_server::now_rfc3339());
    let catalog = Catalog::new(snapshot::parse(&json).unwrap());
    let server = DirectoryServer::start(0, json, jpeg.clone(), png.clone()).unwrap();
    let base = format!("{}/", server.address());
    let http: Arc<dyn Fetcher> = Arc::new(HttpFetcher::new());
    let fetch: artwork::FetchArt = Arc::new(move |url: &str| http.get(url, MAX_ART_BYTES).map(|r| r.body));
    let ctx = egui::Context::default();
    let mut art = ArtLoader::new(
        artwork::DEFAULT_WORKERS,
        fetch,
        None,
        artwork::DEFAULT_BUDGET,
        Arc::new(|| {}),
    );
    let requests: Vec<ArtRequest> = catalog
        .worlds
        .iter()
        .filter_map(|w| art_request(w, &base, "card", plate, Some("400"), None))
        .collect();
    let base_heap = sysstat::live_heap_bytes();
    let base_rss = sysstat::resident_bytes();
    let mut peak_heap = base_heap;
    let start = Instant::now();
    // Page through the cards five at a time, as a scroll that waits for each page to load.
    let mut max_bytes = 0usize;
    for page in requests.chunks(5) {
        loop {
            art.begin_frame(&ctx);
            let ready = page
                .iter()
                .filter(|r| matches!(art.request(r), ArtState::Ready { .. } | ArtState::Failed))
                .count();
            art.end_frame();
            let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
            out.textures_delta.clear();
            peak_heap = peak_heap.max(sysstat::live_heap_bytes());
            max_bytes = max_bytes.max(art.textures().bytes());
            if ready == page.len() || start.elapsed() > Duration::from_secs(300) {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    rows.push(Row {
        name: "300 cards paged 5 at a time, 4000x3000 artwork (every fifth PNG), fetched over loopback, decoded to 800x320".into(),
        unit: "run",
        ms,
        kb: 0.0,
        note: format!(
            "{} art requests; textures at most {:.0} MB, kept {} ({:.0} MB of a {:.0} MB budget, {} evicted); peak live heap rise {:.0} MB; RSS {:.0} to {:.0} MB",
            server.art_requests(),
            max_bytes as f64 / 1048576.0,
            art.textures().len(),
            art.textures().bytes() as f64 / 1048576.0,
            art.textures().budget() as f64 / 1048576.0,
            artwork::ArtStats::get(&art.stats.evicted),
            (peak_heap - base_heap) as f64 / 1048576.0,
            base_rss as f64 / 1048576.0,
            sysstat::resident_bytes() as f64 / 1048576.0,
        ),
    });
    drop(art);
}
