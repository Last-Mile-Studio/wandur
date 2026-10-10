//! In-process measurements, named after the C# `micro` rows they compare with where one exists.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{RawInput, Rect, pos2, vec2};
use wandur_app::fonts::{self, FallbackFonts};
use wandur_app::sysstat;
use wandur_app::terminal_view::{self, TermFonts, TerminalViewState};
use wandur_app::theme::Theme;
use wandur_core::Endpoint;
use wandur_core::session::{Drained, Session, SessionConfig};
use wandur_term::{TermSize, Terminal};

use crate::generator::AnsiGenerator;
use crate::mud_server::MudServer;

pub struct Row {
    pub name: String,
    pub unit: &'static str,
    pub ms: f64,
    pub kb: f64,
    pub note: String,
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

pub fn run(label: &str) {
    let mut rows = Vec::new();
    if std::env::var_os("WANDUR_MICRO_ONLY_MACROS").is_some() {
        macros(&mut rows);
        print_rows(&rows, label);
        return;
    }
    if std::env::var_os("WANDUR_MICRO_ONLY_CHANNELS").is_some() {
        channels(&mut rows);
        print_rows(&rows, label);
        return;
    }
    if std::env::var_os("WANDUR_MICRO_ONLY_PANELS").is_some() {
        crate::micro_panels::panels(&mut rows);
        print_rows(&rows, label);
        return;
    }
    if std::env::var_os("WANDUR_MICRO_ONLY_MAP").is_some() {
        crate::micro_map::map(&mut rows);
        print_rows(&rows, label);
        return;
    }
    if std::env::var_os("WANDUR_MICRO_ONLY_AGENT").is_some() {
        crate::micro_agent::agent(&mut rows);
        print_rows(&rows, label);
        return;
    }
    if std::env::var_os("WANDUR_MICRO_ONLY_SCRIPTS").is_some() {
        scripts(&mut rows);
        print_rows(&rows, label);
        return;
    }
    terminal(&mut rows);
    macros(&mut rows);
    channels(&mut rows);
    scripts(&mut rows);
    crate::micro_panels::panels(&mut rows);
    crate::micro_map::map(&mut rows);
    crate::micro_agent::agent(&mut rows);
    render(&mut rows);
    sessions(&mut rows, 1, 200_000, false);
    sessions(&mut rows, 4, 100_000, false);
    crate::micro_dir::directory(&mut rows);
    crate::micro_dir::artwork(&mut rows);
    print_rows(&rows, label);
}

fn print_rows(rows: &[Row], label: &str) {
    let mut table =
        String::from("| Measurement | Unit | ms per unit | KB allocated per unit | Note |\n|---|---|---:|---:|---|\n");
    for r in rows {
        let _ = writeln!(
            table,
            "| {} | {} | {:.3} | {:.1} | {} |",
            r.name, r.unit, r.ms, r.kb, r.note
        );
    }
    println!("{table}");
    let dir = std::path::Path::new(".superpowers/perf");
    if std::fs::create_dir_all(dir).is_ok() {
        let _ = std::fs::write(dir.join(format!("micro-{label}.md")), &table);
    }
}

/// Macro triggers over the flood's lines: 100,000 lines through 1,000 triggers (the bench set),
/// the lines alone, and the case fold alone.
fn macros(rows: &mut Vec<Row>) {
    use wandur_core::db::scripts::LibraryEntry;
    use wandur_core::macros::{Gate, MacroRuntime, RuleSet};
    let chunks = AnsiGenerator::new(42, true).chunks(100_000, 4096);
    let mut t = Terminal::new(TermSize::new(120, 50), 2_000);
    t.set_line_events(true);
    let mut lines = Vec::new();
    for c in &chunks {
        t.feed(c.as_bytes());
        t.take_lines(&mut lines);
    }
    let texts: Vec<String> = lines.into_iter().map(|l| l.text).collect();
    let n = texts.len() as f64;
    let entries = crate::micro_shell::bench_macros(1_000);
    let library: Vec<_> = entries.iter().filter_map(LibraryEntry::as_saved_macro).collect();
    let mut measure = |name: &str, note: &str, f: &mut dyn FnMut()| {
        let mut times = Vec::new();
        let mut alloc = Vec::new();
        for _ in 0..5 {
            let a0 = sysstat::allocated_bytes();
            let start = Instant::now();
            f();
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            alloc.push((sysstat::allocated_bytes() - a0) as f64 / 1024.0);
        }
        let ms = median(times);
        rows.push(Row {
            name: name.into(),
            unit: "1,000 lines",
            ms: ms / n * 1000.0,
            kb: median(alloc) / n * 1000.0,
            note: format!("{n:.0} flood lines; {note}"),
        });
    };
    let build = Instant::now();
    let mut set = RuleSet::build(library.iter().map(|m| (m.id.as_str(), &m.definition)));
    let build_ms = build.elapsed().as_secs_f64() * 1000.0;
    measure(
        "RuleSet.match_line, 1,000 triggers (one DFA)",
        &format!("build {build_ms:.1} ms, automata {} KB", set.automata_bytes() / 1024),
        &mut || {
            let mut out = Vec::new();
            for line in &texts {
                set.match_line(line, &mut out);
                out.clear();
            }
        },
    );
    let mut runtime = MacroRuntime::new(library.clone());
    let now = std::time::Instant::now();
    measure(
        "MacroRuntime.on_line, 1,000 triggers",
        "gate, match, rate limit",
        &mut || {
            let mut out = Vec::new();
            for line in &texts {
                runtime.on_line(line, now, Gate::connected(), &mut out);
                out.clear();
            }
        },
    );
    let mut runtime2 = MacroRuntime::new(library.clone());
    measure(
        "Terminal.feed + take_lines + on_line + recycle, 1,000 triggers",
        "the session's per-chunk path, including the feed",
        &mut || {
            let mut t = Terminal::new(TermSize::new(120, 50), 2_000);
            t.set_line_events(true);
            let mut lines = Vec::new();
            let mut out = Vec::new();
            for c in &chunks {
                t.feed(c.as_bytes());
                t.take_lines(&mut lines);
                for l in &lines {
                    runtime2.on_line(&l.text, now, Gate::connected(), &mut out);
                    out.clear();
                }
                t.recycle_lines(&mut lines);
            }
        },
    );
    measure("Terminal.feed alone", "for the row above", &mut || {
        let mut t = Terminal::new(TermSize::new(120, 50), 2_000);
        for c in &chunks {
            t.feed(c.as_bytes());
        }
    });
    measure(
        "Case fold of each line",
        "ignore-case triggers search a folded copy",
        &mut || {
            let mut folded = String::new();
            for line in &texts {
                wandur_core::macros::rules::fold_into(line, &mut folded);
                std::hint::black_box(&folded);
            }
        },
    );
}

/// The script used by the shell bench's script scenario and below: a regex trigger over every
/// completed line and a line listener, both cheap callbacks that send nothing.
pub const BENCH_SCRIPT: &str = r#"let seen = 0, long = 0;
mud.trigger(/lantern river/i, () => { seen++; });
mud.on(Events.Line, e => { if (e.text.length > 4000) long++; });"#;

/// Scripts: what starting one costs (a runtime, the bootstrap, the source), and the engine
/// over the flood's lines (a trigger and a line listener per line).
fn scripts(rows: &mut Vec<Row>) {
    use std::sync::Arc;
    use wandur_core::scripting::engine::Engine;
    use wandur_core::scripting::engines::EngineSet;
    use wandur_core::scripting::session::{ScriptDefinition, SessionScripts};
    use wandur_core::scripting::{EventKind, ScriptEvent};
    let timed = |runs: usize, f: &mut dyn FnMut()| -> (f64, f64) {
        let mut times = Vec::new();
        let mut alloc = Vec::new();
        for _ in 0..runs {
            let a0 = sysstat::allocated_bytes();
            let start = Instant::now();
            f();
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            alloc.push((sysstat::allocated_bytes() - a0) as f64 / 1024.0);
        }
        (median(times), median(alloc))
    };
    for (name, source) in [
        ("Engine.load, an empty script", String::new()),
        (
            "Engine.load, Lantern watch (help example)",
            wandur_app::scene::LANTERN_WATCH.to_string(),
        ),
        (
            "Engine.load, 250 KB of functions",
            (0..6_000)
                .map(|i| format!("function f{i}(x) {{ return x + {i}; }}\n"))
                .collect(),
        ),
    ] {
        let (ms, kb) = timed(31, &mut || {
            let (engine, result) = Engine::load(&source, false, None);
            assert!(engine.is_some(), "{:?}", result.error);
        });
        rows.push(Row {
            name: name.into(),
            unit: "one start",
            ms,
            kb,
            note: format!("{} bytes of source", source.len()),
        });
    }
    let seed = format!(
        r#"{{"gmcp":{{"Char.Vitals":{{"hp":1}}}},"msdp":{{{}}}}}"#,
        (0..200)
            .map(|i| format!(r#""V{i}":"{i}""#))
            .collect::<Vec<_>>()
            .join(",")
    );
    let (ms, kb) = timed(31, &mut || {
        let _ = Engine::load(wandur_app::scene::LANTERN_WATCH, false, Some(&seed));
    });
    rows.push(Row {
        name: "Engine.load, Lantern watch with a 200-variable seed".into(),
        unit: "one start",
        ms,
        kb,
        note: format!("{} bytes of seed", seed.len()),
    });
    let (ms, kb) = timed(11, &mut || {
        let mut scripts = SessionScripts::new("bench-scripts", Arc::new(|| {}));
        scripts.set_library(vec![ScriptDefinition {
            id: "w".into(),
            name: "w".into(),
            source: wandur_app::scene::LANTERN_WATCH.into(),
            enabled: true,
            restricted_send: false,
            runtime: Default::default(),
        }]);
        let gate = wandur_core::macros::Gate::connected();
        scripts.update(gate, Instant::now(), &|| None);
        let (mut effects, mut commands) = (Vec::new(), Vec::new());
        while !scripts.entries[0].is_running() {
            scripts.poll(Instant::now(), &mut effects, &mut commands);
            std::thread::yield_now();
        }
    });
    rows.push(Row {
        name: "SessionScripts: switch on to running (new thread)".into(),
        unit: "one start",
        ms,
        kb,
        note: "thread start, load, reply".into(),
    });
    let chunks = AnsiGenerator::new(42, true).chunks(100_000, 4096);
    let mut t = Terminal::new(TermSize::new(120, 50), 2_000);
    t.set_line_events(true);
    let mut lines = Vec::new();
    for c in &chunks {
        t.feed(c.as_bytes());
        t.take_lines(&mut lines);
    }
    let events: Vec<ScriptEvent> = lines
        .iter()
        .map(|l| ScriptEvent::new(EventKind::Line, l.text.as_str()))
        .collect();
    let bytes: usize = chunks.iter().map(String::len).sum();
    let n = events.len() as f64;
    let mut set = EngineSet::default();
    assert!(set.load("bench", BENCH_SCRIPT, false).error.is_none());
    let (ms, kb) = timed(5, &mut || {
        for event in &events {
            std::hint::black_box(set.dispatch_one("bench", event));
        }
    });
    rows.push(Row {
        name: "EngineSet.dispatch, a trigger and a line listener".into(),
        unit: "1,000 lines",
        ms: ms / n * 1000.0,
        kb: kb / n * 1000.0,
        note: format!(
            "{n:.0} flood lines, {:.2} MB: {:.1} ms per MB on the script thread",
            bytes as f64 / 1_048_576.0,
            ms / (bytes as f64 / 1_048_576.0)
        ),
    });
    lua(rows, &events, bytes, &timed);
}

/// Runs a closure `n` times: median milliseconds and KB allocated.
type Timed = dyn Fn(usize, &mut dyn FnMut()) -> (f64, f64);

/// The bench script in Lua: the same trigger and listener through the shared host API.
pub const BENCH_LUA: &str = r#"local seen, long = 0, 0
mud.trigger(mud.regex("lantern river", "i"), function() seen = seen + 1 end)
mud.on(Events.Line, function(e) if #e.text > 4000 then long = long + 1 end end)"#;

/// Lua scripts (t11): what a start costs with and without the Mudlet layer, the flood's lines
/// through the bench script, and converting the Mudlet fixture profile.
fn lua(rows: &mut Vec<Row>, events: &[wandur_core::scripting::ScriptEvent], bytes: usize, timed: &Timed) {
    use wandur_core::scripting::Runtime;
    use wandur_core::scripting::engines::EngineSet;
    for (name, source, runtime) in [
        ("EngineSet.load, Lua, an empty script", "", Runtime::LUA),
        ("EngineSet.load, Lua, the bench script", BENCH_LUA, Runtime::LUA),
        (
            "EngineSet.load, Lua with the Mudlet layer, an empty script",
            "",
            Runtime::MUDLET,
        ),
    ] {
        let (ms, kb) = timed(31, &mut || {
            let mut set = EngineSet::default();
            let result = set.load_with("l", source, false, runtime);
            assert!(result.error.is_none(), "{:?}", result.error);
        });
        rows.push(Row {
            name: name.into(),
            unit: "one start",
            ms,
            kb,
            note: "a QuickJS host engine and a Lua state".into(),
        });
    }
    let n = events.len() as f64;
    let mut set = EngineSet::default();
    assert!(set.load_with("bench", BENCH_LUA, false, Runtime::LUA).error.is_none());
    let (ms, kb) = timed(5, &mut || {
        for event in events {
            std::hint::black_box(set.dispatch_one("bench", event));
        }
    });
    rows.push(Row {
        name: "EngineSet.dispatch, Lua, a trigger and a line listener".into(),
        unit: "1,000 lines",
        ms: ms / n * 1000.0,
        kb: kb / n * 1000.0,
        note: format!(
            "{:.1} ms per MB on the script thread",
            ms / (bytes as f64 / 1_048_576.0)
        ),
    });
    let profile = include_bytes!("../../wandur-core/tests/fixtures/mudlet/lantern-road-profile.xml");
    let (ms, kb) = timed(11, &mut || {
        let package = wandur_core::mudlet::parser::parse(profile).expect("the fixture parses");
        let source = wandur_core::mudlet::model::Source {
            name: "The Lantern Road".into(),
            packages: vec![package],
            ..Default::default()
        };
        let (after, _) = wandur_core::mudlet::importer::apply(&source, "w", "The Lantern Road", true, &[]);
        std::hint::black_box(after);
    });
    rows.push(Row {
        name: "Mudlet import of the fixture profile (parse, convert, check patterns)".into(),
        unit: "one import",
        ms,
        kb,
        note: format!("{} bytes of XML", profile.len()),
    });
}

/// Channel classification over the flood with channel chatter (15% of lines are `[chan] Name:
/// text`, which the generic and SMAUG families recognize in part), as the session runs it: the
/// raw text in 4 KB chunks, lines cut, stripped, matched, messages stored. Reported per MB and
/// per frame at 1 MB/s and 60 frames a second (17.5 KB of text a frame).
fn channels(rows: &mut Vec<Row>) {
    use wandur_core::channels::{ChannelRule, SessionChannels};
    let chunks = AnsiGenerator::new(42, true).chunks(100_000, 4096);
    let bytes: usize = chunks.iter().map(String::len).sum();
    let mb = bytes as f64 / 1_048_576.0;
    let taught: Vec<ChannelRule> = (0..20)
        .map(|i| {
            ChannelRule::new(
                &format!("taught{i}"),
                &format!(r"^\[Taught{i}\] (?<speaker>@?[A-Za-z]+): (?<text>.*)$"),
                Some("x"),
            )
        })
        .collect();
    let cases: [(&str, Vec<ChannelRule>, &str); 3] = [
        ("generic family", Vec::new(), ""),
        ("SMAUG family", Vec::new(), "SMAUG 1.4a"),
        ("SMAUG family + 20 world rules", taught, "SMAUG 1.4a"),
    ];
    // The parts: cutting and stripping lines alone, then the rule match on the stripped lines.
    let mut lines = Vec::new();
    for c in &chunks {
        lines.extend(c.split('\n').map(str::to_string));
    }
    let generic = wandur_core::channels::families::family(None);
    let mut plain = String::new();
    let mut part = |name: &str, f: &mut dyn FnMut()| {
        let mut times = Vec::new();
        for _ in 0..5 {
            let start = Instant::now();
            f();
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        let ms_per_mb = median(times) / mb;
        rows.push(Row {
            name: name.into(),
            unit: "MB of flood",
            ms: ms_per_mb,
            kb: 0.0,
            note: format!("{:.3} ms a frame at 1 MB/s and 60 fps", ms_per_mb / 60.0),
        });
    };
    part("channels: strip each line", &mut || {
        for l in &lines {
            wandur_core::channels::rules::strip_into(l, &mut plain);
            std::hint::black_box(&plain);
        }
    });
    let stripped: Vec<String> = lines.iter().map(|l| wandur_core::channels::rules::strip(l)).collect();
    part("channels: generic rules over stripped lines", &mut || {
        for l in &stripped {
            std::hint::black_box(generic.match_plain(l));
        }
    });
    for (name, world, codebase) in cases {
        let mut times = Vec::new();
        let mut alloc = Vec::new();
        let mut found = 0;
        for _ in 0..5 {
            let mut session = SessionChannels::new(world.clone(), codebase);
            let at = std::time::SystemTime::now();
            let a0 = sysstat::allocated_bytes();
            let start = Instant::now();
            found = 0;
            for c in &chunks {
                found += session.receive_text(c, at);
            }
            times.push(start.elapsed().as_secs_f64() * 1000.0);
            alloc.push((sysstat::allocated_bytes() - a0) as f64 / 1024.0);
        }
        let ms_per_mb = median(times) / mb;
        rows.push(Row {
            name: format!("SessionChannels.receive_text, {name}"),
            unit: "MB of flood",
            ms: ms_per_mb,
            kb: median(alloc) / mb,
            note: format!(
                "{found} messages in 100,000 lines; {:.3} ms a frame at 1 MB/s and 60 fps",
                ms_per_mb / 60.0
            ),
        });
    }
}

fn terminal(rows: &mut Vec<Row>) {
    let chunks = AnsiGenerator::new(42, true).chunks(100_000, 4096);
    let total_chars: usize = chunks.iter().map(|c| c.chars().count()).sum();
    let total_bytes: usize = chunks.iter().map(String::len).sum();
    let size = TermSize::new(120, 50);

    // 100,000 lines into a 2,000 row scrollback, the M2 grid and (for comparison) the M1 buffer.
    let mut append = |name: &str, note: &str, f: &mut dyn FnMut()| {
        let mut times = Vec::new();
        let mut alloc = Vec::new();
        for _ in 0..7 {
            let a0 = sysstat::allocated_bytes();
            let t = Instant::now();
            f();
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            alloc.push((sysstat::allocated_bytes() - a0) as f64 / 1024.0);
        }
        let ms = median(times);
        rows.push(Row {
            name: name.into(),
            unit: "run",
            ms,
            kb: median(alloc),
            note: format!(
                "{:.1} M chars in 4 KB chunks, {:.0} MB/s; {note}",
                total_chars as f64 / 1e6,
                total_bytes as f64 / 1048576.0 / (ms / 1000.0)
            ),
        });
    };
    append(
        "Terminal.feed (alacritty grid 120x50), 100k lines into 2,000 row scrollback",
        "M1 buffer 36.6 ms, 18,120 KB; C# AnsiTerminal.Append 54.9 ms, 42,964 KB",
        &mut || {
            let mut t = Terminal::new(size, 2_000);
            for c in &chunks {
                t.feed(c.as_bytes());
            }
            std::hint::black_box(&t);
        },
    );
    append(
        "Terminal.feed with line events on (taken every chunk)",
        "each completed line read back from the grid",
        &mut || {
            let mut t = Terminal::new(size, 2_000);
            t.set_line_events(true);
            let mut lines = Vec::new();
            for c in &chunks {
                t.feed(c.as_bytes());
                lines.clear();
                t.take_lines(&mut lines);
            }
            std::hint::black_box(&t);
        },
    );
    // One 4 KB chunk into a full scrollback.
    let mut t = Terminal::new(size, 2_000);
    for c in &chunks[..chunks.len() / 2] {
        t.feed(c.as_bytes());
    }
    let rest = &chunks[chunks.len() / 2..];
    let a0 = sysstat::allocated_bytes();
    let start = Instant::now();
    for c in rest {
        t.feed(c.as_bytes());
    }
    rows.push(Row {
        name: "Terminal.feed of one 4 KB chunk (full scrollback)".into(),
        unit: "chunk",
        ms: start.elapsed().as_secs_f64() * 1000.0 / rest.len() as f64,
        kb: (sysstat::allocated_bytes() - a0) as f64 / 1024.0 / rest.len() as f64,
        note: "M1 0.018 ms, 8.9 KB; C# 0.023 ms, 19.2 KB".into(),
    });

    // Plain text export of the full scrollback.
    let start = Instant::now();
    let a0 = sysstat::allocated_bytes();
    let n = 50;
    for _ in 0..n {
        std::hint::black_box(t.transcript());
    }
    rows.push(Row {
        name: "Terminal.transcript, full 2,000 row scrollback".into(),
        unit: "call",
        ms: start.elapsed().as_secs_f64() * 1000.0 / n as f64,
        kb: (sysstat::allocated_bytes() - a0) as f64 / 1024.0 / n as f64,
        note: "M1 0.012 ms; C# AnsiTerminal.PlainText 0.321 ms".into(),
    });

    // Reflow of a full scrollback on a width change (a window resize step).
    let start = Instant::now();
    let n = 20;
    for i in 0..n {
        t.resize(TermSize::new(if i % 2 == 0 { 100 } else { 120 }, 50));
    }
    rows.push(Row {
        name: "Terminal.resize with reflow, 2,000 rows, 120 <-> 100 columns".into(),
        unit: "resize",
        ms: start.elapsed().as_secs_f64() * 1000.0 / n as f64,
        kb: 0.0,
        note: "what one step of a window drag costs".into(),
    });

    // Retained memory of a full grid.
    drop(t);
    for (columns, label) in [(120, "120 columns"), (80, "80 columns")] {
        let live0 = sysstat::live_heap_bytes();
        let mut t = Terminal::new(TermSize::new(columns, 50), 2_000);
        for c in &chunks {
            t.feed(c.as_bytes());
        }
        let retained = sysstat::live_heap_bytes().saturating_sub(live0) as f64 / 1024.0;
        rows.push(Row {
            name: format!("Terminal retained, 2,000 history rows + 50 screen rows, {label}"),
            unit: "instance",
            ms: 0.0,
            kb: retained,
            note: format!(
                "retained, not allocated, incl. 2,048 KB vte reserves untouched; cell {} bytes; M1 buffer 328 KB; C# AnsiTerminal 914 KB + TranscriptDisplay 6,026 KB",
                std::mem::size_of::<wandur_term::alacritty_terminal::term::cell::Cell>()
            ),
        });
        drop(t);
    }
}

/// Headless egui frames of the terminal view over a full scrollback: layout plus tessellation,
/// no GPU. The window is 1200 x 780 points at 2 pixels per point (a Retina display).
fn render(rows: &mut Vec<Row>) {
    let chunks = AnsiGenerator::new(9, true).chunks(4_000, 4096);
    let mut term = Terminal::new(TermSize::new(120, 50), 2_000);
    for c in &chunks {
        term.feed(c.as_bytes());
    }
    let more = AnsiGenerator::new(10, true).chunks(20_000, 4096);
    let ctx = egui::Context::default();
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, Theme::ember().visuals());
    fonts::install(&ctx);
    let theme = Theme::ember();
    let term_fonts = TermFonts::new(13.0);
    let mut fallback = FallbackFonts::new();
    let mut view = TerminalViewState::default();
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1200.0, 780.0))),
        ..Default::default()
    };
    let mut frame = |term: &mut Terminal, view: &mut TerminalViewState| {
        let mut out = ctx.run_ui(input(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                terminal_view::show_terminal(
                    ui,
                    term,
                    1,
                    view,
                    &theme,
                    &term_fonts,
                    &mut fallback,
                    &terminal_view::PaintOptions::default(),
                );
            });
        });
        out.textures_delta.clear();
        let prims = ctx.tessellate(out.shapes, 2.0);
        std::hint::black_box(prims);
    };
    for _ in 0..30 {
        frame(&mut term, &mut view);
    }
    let measure = |name: &str, note: &str, rows: &mut Vec<Row>, f: &mut dyn FnMut()| {
        let n = 300;
        let a0 = sysstat::allocated_bytes();
        let t = Instant::now();
        for _ in 0..n {
            f();
        }
        rows.push(Row {
            name: name.into(),
            unit: "frame",
            ms: t.elapsed().as_secs_f64() * 1000.0 / n as f64,
            kb: (sysstat::allocated_bytes() - a0) as f64 / 1024.0 / n as f64,
            note: note.into(),
        });
    };
    measure(
        "Terminal frame at the tail, 2,000 row scrollback, no new output",
        "egui layout + tessellation, headless; M1 0.057 ms, 976 KB",
        rows,
        &mut || frame(&mut term, &mut view),
    );
    let drawn = view.rows_drawn;
    let mut i = 0;
    let runs = std::cell::Cell::new(0usize);
    measure(
        "Terminal frame with a 4 KB append each frame (240 KB/s at 60 fps)",
        &format!("{drawn} rows painted per frame; M1 0.386 ms, 2,045 KB"),
        rows,
        &mut || {
            term.feed(more[i % more.len()].as_bytes());
            i += 1;
            frame(&mut term, &mut view);
            runs.set(view.runs_drawn);
        },
    );
    if let Some(last) = rows.last_mut() {
        last.note
            .push_str(&format!(", {} text runs in the last frame", runs.get()));
    }
}

/// Sessions against the Rust loopback server, drained every 16 ms into buffers on this thread,
/// as the app's frame loop would. CPU is of the whole process (server threads included).
fn sessions(rows: &mut Vec<Row>, count: usize, rate: u64, chat: bool) {
    let server = MudServer::start(0, rate, chat).expect("loopback server");
    let endpoint = Endpoint::new("127.0.0.1", server.port);
    let mut open: Vec<(Session, Terminal)> = (0..count)
        .map(|_| {
            (
                Session::connect(SessionConfig::new(endpoint.clone()), Arc::new(|| {})),
                Terminal::new(TermSize::new(120, 50), 2_000),
            )
        })
        .collect();
    let mut drained = Drained::default();
    let settle = Instant::now();
    while settle.elapsed() < Duration::from_secs(2) {
        for (s, b) in &mut open {
            s.drain(&mut drained);
            b.feed(drained.text.as_bytes());
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    let seconds = 8.0;
    let cpu0 = sysstat::cpu_time();
    let a0 = sysstat::allocated_bytes();
    let t = Instant::now();
    let mut chars = 0usize;
    while t.elapsed().as_secs_f64() < seconds {
        for (s, b) in &mut open {
            s.drain(&mut drained);
            chars += drained.text.len();
            b.feed(drained.text.as_bytes());
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    let secs = t.elapsed().as_secs_f64();
    let cpu = (sysstat::cpu_time() - cpu0).as_secs_f64() * 100.0 / secs;
    let mb = chars as f64 / 1048576.0;
    rows.push(Row {
        name: format!(
            "{count} session(s) at {} KB/s each{}, drained into terminal grids every 16 ms",
            rate / 1000,
            if chat { "" } else { ", no channel lines" }
        ),
        unit: "MB of output",
        ms: secs * 1000.0 / mb.max(1e-9),
        kb: (sysstat::allocated_bytes() - a0) as f64 / 1024.0 / mb.max(1e-9),
        note: format!(
            "{:.2} MB/s taken in ({:.1} MB sent by the server in all), CPU {cpu:.1}% of one core incl. server, {:.1} MB/s allocated",
            mb / secs,
            server.bytes_sent() as f64 / 1048576.0,
            (sysstat::allocated_bytes() - a0) as f64 / 1048576.0 / secs
        ),
    });
    drop(open);
    drop(server);
}
