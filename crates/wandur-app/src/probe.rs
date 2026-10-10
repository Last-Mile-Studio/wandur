//! Opt-in measurement, mirroring the C# client's `PerfProbe` so the same harness
//! (`bench/Wandur.Bench app --app <this binary>`) can drive both apps.
//!
//! Off unless `WANDUR_PERF_PROBE` names an output file. Then it writes JSON lines with the same
//! phases and field names as the C# probe: `usable` (process start to the first presented frame),
//! `opened`, `start`, `end`, `after-gc`, `assemblies`, `done`, and waits for `<file>.exit` before
//! closing the window. Columns without a Rust equivalent are mapped: `gcHeapMb` is the live heap
//! from the counting allocator; GC counts are 0; `assemblies` is empty.
//!
//! UI latency: a ticker thread posts a timestamp every 50 ms and requests a repaint; the frame that
//! picks it up records how long it waited. That forces up to 20 frames a second while idle, so
//! `WANDUR_PERF_NO_TICK=1` turns the ticker off for CPU measurements of idle scenarios.

use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use wandur_core::Endpoint;

use crate::sysstat;

/// What the probe asks the app to do this frame.
#[derive(Debug, PartialEq)]
pub enum ProbeAction {
    None,
    OpenSessions(Endpoint, usize),
    /// Show Find a MUD (the directory scenario).
    ShowDirectory,
    /// Scroll the directory results to this offset.
    ScrollDirectory(f32),
    Close,
}

/// What the app tells the probe at the start of each frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProbeInput {
    pub connected_sessions: usize,
    pub chars_total: u64,
    pub frames_presented: u64,
    /// Worlds in the directory catalog.
    pub catalog_worlds: usize,
    /// The directory list's scroll offset, viewport and extent.
    pub scroll: (f32, f32, f32),
    /// Directory cards laid out in the last frame (the C# probe's realized rows).
    pub cards_drawn: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    WaitFirstFrame,
    WaitConnected {
        since: Instant,
    },
    WaitCatalog {
        since: Instant,
    },
    Settle {
        until: Instant,
    },
    /// Paging through the directory: the next page is due at `next`.
    Scroll {
        next: Instant,
        pages: u32,
        most: usize,
        started: Instant,
    },
    BackToTop {
        until: Instant,
    },
    Measure {
        until: Instant,
    },
    WaitExit {
        since: Instant,
    },
    Finished,
}

#[derive(Debug, Clone, Copy)]
struct Snapshot {
    cpu: Duration,
    allocated: u64,
    chars: u64,
    frames: u64,
    at: Instant,
}

pub struct Probe {
    output: PathBuf,
    scenario: String,
    seconds: u64,
    settle: u64,
    sessions: usize,
    host: String,
    phase: Phase,
    ticks: Option<Arc<Mutex<Vec<Instant>>>>,
    latencies: Vec<f64>,
    started: Option<Snapshot>,
    scrolled: bool,
    cards: usize,
    /// When this frame's work began (start of `App::logic`).
    frame_began: Option<Instant>,
    frames: FrameTimes,
}

/// Upper bounds (ms) of the frame-time histogram buckets; the last bucket is everything above.
pub const FRAME_BUCKETS: [f64; 7] = [1.0, 2.0, 4.0, 8.0, 16.0, 33.0, 50.0];

/// How long the UI thread spent per frame (logic plus building the UI, not presenting it): a
/// histogram, so a long stall shows even when the average is low.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameTimes {
    pub counts: [u64; FRAME_BUCKETS.len() + 1],
    pub max_ms: f64,
}

impl FrameTimes {
    pub fn add(&mut self, ms: f64) {
        let i = FRAME_BUCKETS
            .iter()
            .position(|&b| ms <= b)
            .unwrap_or(FRAME_BUCKETS.len());
        self.counts[i] += 1;
        self.max_ms = self.max_ms.max(ms);
    }

    /// Frames that took longer than `ms` (a bucket bound).
    pub fn over(&self, ms: f64) -> u64 {
        let i = FRAME_BUCKETS
            .iter()
            .position(|&b| b >= ms)
            .map_or(FRAME_BUCKETS.len(), |i| i + 1);
        self.counts[i..].iter().sum()
    }

    fn json(&self) -> String {
        let counts: Vec<String> = self.counts.iter().map(u64::to_string).collect();
        format!(
            r#""frameMsMax":{:.2},"framesOver16ms":{},"framesOver50ms":{},"frameHist":[{}]"#,
            self.max_ms,
            self.over(16.0),
            self.over(50.0),
            counts.join(",")
        )
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn env_num<T: std::str::FromStr>(name: &str, fallback: T) -> T {
    env(name).and_then(|v| v.parse().ok()).unwrap_or(fallback)
}

impl Probe {
    /// The probe if `WANDUR_PERF_PROBE` is set; nothing runs or allocates otherwise.
    pub fn from_env(ctx: &egui::Context) -> Option<Probe> {
        let output = PathBuf::from(env("WANDUR_PERF_PROBE")?);
        let ticks = if env("WANDUR_PERF_NO_TICK").is_some() {
            None
        } else {
            let ticks = Arc::new(Mutex::new(Vec::new()));
            let posted = Arc::clone(&ticks);
            let ctx = ctx.clone();
            let _ = std::thread::Builder::new()
                .name("wandur-probe-tick".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(Duration::from_millis(50));
                        posted
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(Instant::now());
                        ctx.request_repaint();
                    }
                });
            Some(ticks)
        };
        Some(Probe {
            output,
            scenario: env("WANDUR_PERF_SCENARIO").unwrap_or_else(|| "idle".into()),
            seconds: env_num("WANDUR_PERF_SECONDS", 5),
            settle: env_num("WANDUR_PERF_SETTLE", 3),
            sessions: env_num("WANDUR_PERF_SESSIONS", 1),
            host: env("WANDUR_PERF_HOST").unwrap_or_else(|| "127.0.0.1:4400".into()),
            phase: Phase::WaitFirstFrame,
            ticks,
            latencies: Vec::new(),
            started: None,
            scrolled: false,
            cards: 0,
            frame_began: None,
            frames: FrameTimes::default(),
        })
    }

    /// Call at the start of every frame (in `App::logic`).
    pub fn on_frame(&mut self, ctx: &egui::Context, input: ProbeInput) -> ProbeAction {
        let now = Instant::now();
        self.frame_began = Some(now);
        if let Some(ticks) = &self.ticks {
            let mut ticks = ticks.lock().unwrap_or_else(PoisonError::into_inner);
            self.latencies
                .extend(ticks.drain(..).map(|t| now.duration_since(t).as_secs_f64() * 1000.0));
        }
        let mut action = ProbeAction::None;
        self.cards = input.cards_drawn;
        match self.phase {
            Phase::WaitFirstFrame => {
                if input.frames_presented == 0 {
                    ctx.request_repaint();
                    return action;
                }
                let ms = sysstat::since_process_start().as_secs_f64() * 1000.0;
                self.write(&format!(r#"{{"phase":"usable","startupMs":{ms:.1},"assemblies":[]}}"#));
                if self.scenario == "sessions" {
                    match self.host.parse::<Endpoint>() {
                        Ok(endpoint) => {
                            action = ProbeAction::OpenSessions(endpoint, self.sessions);
                            self.phase = Phase::WaitConnected { since: now };
                        }
                        Err(e) => {
                            self.error(&format!("bad WANDUR_PERF_HOST: {e}"));
                            self.phase = Phase::WaitExit { since: now };
                        }
                    }
                } else if self.scenario == "directory" {
                    action = ProbeAction::ShowDirectory;
                    self.phase = Phase::WaitCatalog { since: now };
                } else {
                    self.phase = Phase::Settle {
                        until: now + Duration::from_secs(self.settle),
                    };
                }
            }
            Phase::WaitConnected { since } => {
                if input.connected_sessions >= self.sessions || now.duration_since(since) > Duration::from_secs(20) {
                    self.write(&format!(
                        r#"{{"phase":"opened","sessions":{}}}"#,
                        input.connected_sessions
                    ));
                    self.phase = Phase::Settle {
                        until: now + Duration::from_secs(self.settle),
                    };
                }
            }
            Phase::WaitCatalog { since } => {
                let waited = now.duration_since(since);
                if input.catalog_worlds > 0 || waited > Duration::from_secs(20) {
                    self.write(&format!(
                        r#"{{"phase":"catalog","worlds":{},"ms":{:.1}}}"#,
                        input.catalog_worlds,
                        waited.as_secs_f64() * 1000.0
                    ));
                    self.phase = Phase::Settle {
                        until: now + Duration::from_secs(self.settle),
                    };
                }
            }
            Phase::Settle { until } => {
                if now >= until {
                    if self.scenario == "directory" && !self.scrolled {
                        self.scrolled = true;
                        self.phase = Phase::Scroll {
                            next: now,
                            pages: 0,
                            most: input.cards_drawn,
                            started: now,
                        };
                    } else {
                        self.start_measure(input, now);
                    }
                }
            }
            Phase::Scroll {
                next,
                pages,
                most,
                started,
            } => {
                if now >= next {
                    let most = most.max(input.cards_drawn);
                    let (offset, viewport, extent) = input.scroll;
                    if offset + viewport < extent - 1.0 && pages < 500 {
                        action = ProbeAction::ScrollDirectory(offset + viewport);
                        self.phase = Phase::Scroll {
                            next: now + Duration::from_millis(30),
                            pages: pages + 1,
                            most,
                            started,
                        };
                    } else {
                        let down = now.duration_since(started).as_secs_f64() * 1000.0;
                        self.write(&format!(
                            r#"{{"phase":"scroll","pages":{pages},"mostRealized":{most},"extent":{extent:.0},"viewport":{viewport:.0},"downMs":{down:.0},"realizedAtTop":{}}}"#,
                            input.cards_drawn
                        ));
                        action = ProbeAction::ScrollDirectory(0.0);
                        self.phase = Phase::BackToTop {
                            until: now + Duration::from_millis(200),
                        };
                    }
                } else if let Phase::Scroll { most: m, .. } = &mut self.phase {
                    *m = (*m).max(input.cards_drawn);
                }
            }
            Phase::BackToTop { until } => {
                if now >= until {
                    self.start_measure(input, now);
                }
            }
            Phase::Measure { until } => {
                if now >= until {
                    let end = self.snapshot(input, now);
                    self.sample("end", end, self.started);
                    // No collector to run; the live heap is already exact.
                    self.sample("after-gc", end, None);
                    self.write(r#"{"phase":"assemblies","assemblies":[]}"#);
                    self.write(&format!(r#"{{"phase":"done","pid":{}}}"#, std::process::id()));
                    self.phase = Phase::WaitExit { since: now };
                }
            }
            Phase::WaitExit { since } => {
                let mut exit = self.output.clone().into_os_string();
                exit.push(".exit");
                if PathBuf::from(exit).exists() || now.duration_since(since) > Duration::from_secs(30) {
                    self.phase = Phase::Finished;
                    action = ProbeAction::Close;
                }
            }
            Phase::Finished => {}
        }
        // Wake exactly when the next phase is due, without forcing frames in between.
        match self.phase {
            Phase::Settle { until } | Phase::Measure { until } | Phase::BackToTop { until } => {
                ctx.request_repaint_after(until.saturating_duration_since(now));
            }
            Phase::Scroll { next, .. } => ctx.request_repaint_after(next.saturating_duration_since(now)),
            Phase::WaitConnected { .. } | Phase::WaitExit { .. } | Phase::WaitCatalog { .. } => {
                ctx.request_repaint_after(Duration::from_millis(100))
            }
            _ => {}
        }
        action
    }

    /// Call at the end of every frame (end of `App::ui`): records the frame's UI-thread time.
    pub fn frame_end(&mut self) {
        if let Some(began) = self.frame_began.take() {
            self.frames.add(began.elapsed().as_secs_f64() * 1000.0);
        }
    }

    fn start_measure(&mut self, input: ProbeInput, now: Instant) {
        self.latencies.clear();
        self.frames = FrameTimes::default();
        // This frame writes the start sample to the probe file: not the app's own time.
        self.frame_began = None;
        let s = self.snapshot(input, now);
        self.sample("start", s, None);
        self.started = Some(s);
        self.phase = Phase::Measure {
            until: now + Duration::from_secs(self.seconds),
        };
    }

    fn snapshot(&self, input: ProbeInput, at: Instant) -> Snapshot {
        Snapshot {
            cpu: sysstat::cpu_time(),
            allocated: sysstat::allocated_bytes(),
            chars: input.chars_total,
            frames: input.frames_presented,
            at,
        }
    }

    fn sample(&mut self, phase: &str, now: Snapshot, since: Option<Snapshot>) {
        let mut latencies = std::mem::take(&mut self.latencies);
        latencies.sort_by(f64::total_cmp);
        let pct = |p: f64| {
            if latencies.is_empty() {
                0.0
            } else {
                let i = ((p * latencies.len() as f64).ceil() as usize).clamp(1, latencies.len()) - 1;
                latencies[i]
            }
        };
        let (cpu, alloc, chars, fps) = match since {
            Some(s) => {
                let secs = now.at.duration_since(s.at).as_secs_f64().max(1e-9);
                (
                    (now.cpu - s.cpu).as_secs_f64() * 100.0 / secs,
                    (now.allocated - s.allocated) as f64 / 1048576.0 / secs,
                    (now.chars - s.chars) as f64 / secs,
                    (now.frames - s.frames) as f64 / secs,
                )
            }
            None => (0.0, 0.0, 0.0, 0.0),
        };
        let mb = |b: u64| b as f64 / 1048576.0;
        let mut line = String::new();
        let _ = write!(
            line,
            r#"{{"phase":"{phase}","scenario":"{}","workingSetMb":{:.2},"gcHeapMb":{:.2},"gcCommittedMb":0,"cpuPercent":{cpu:.2},"allocMbPerSec":{alloc:.2},"charsPerSec":{chars:.1},"uiP50":{:.2},"uiP95":{:.2},"uiMax":{:.2},"uiSamples":{},"gen0":0,"gen1":0,"gen2":0,"realizedRows":{},"threads":{},"framesPerSec":{fps:.1},{}}}"#,
            self.scenario,
            mb(sysstat::resident_bytes()),
            mb(sysstat::live_heap_bytes()),
            pct(0.5),
            pct(0.95),
            latencies.last().copied().unwrap_or(0.0),
            latencies.len(),
            self.cards,
            sysstat::thread_count(),
            self.frames.json(),
        );
        self.write(&line);
    }

    fn error(&self, message: &str) {
        let escaped = message.replace('\\', "\\\\").replace('"', "\\\"");
        self.write(&format!(r#"{{"phase":"error","error":"{escaped}"}}"#));
    }

    fn write(&self, line: &str) {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&self.output) {
            let _ = writeln!(f, "{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_histogram_counts_long_frames() {
        let mut f = FrameTimes::default();
        for ms in [0.5, 3.0, 12.0, 20.0, 75.0] {
            f.add(ms);
        }
        assert_eq!(f.counts, [1, 0, 1, 0, 1, 1, 0, 1]);
        assert_eq!(f.over(16.0), 2);
        assert_eq!(f.over(50.0), 1);
        assert_eq!(f.max_ms, 75.0);
        assert!(f.json().contains(r#""framesOver50ms":1"#));
    }
}
