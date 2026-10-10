# Milestone 1 report

2026-10-08. Proof of concept: can egui, eframe and egui_dock carry a MUD client?

## Implemented

- Cargo workspace: `wandur-core` (no UI, no dependencies), `wandur-app` (egui presentation,
  `wandur` binary), `wandur-bench` (loopback MUD server, micro measurements).
- Native window with a dark shell in the C# Ember colours; egui_dock layout with a Sessions panel
  and closable session tabs that can be rearranged, split and floated by the library.
- Connect bar (`host:port`, `host port`, `telnet://`, `[ipv6]:port`) and command line endpoints.
- Per session: a reader and a writer thread, telnet IAC handling (ECHO, SGA, EOR accepted; other
  options refused once; subnegotiations bounded and skipped), incremental UTF-8, a bounded inbox
  with backpressure, and a wake of the UI only when the inbox fills from empty.
- One terminal model per session: ANSI parser (SGR 16/256/truecolor, bold as bright, dim, italic,
  underline, inverse, strike; K, J, C, D, G; OSC and private sequences consumed) into a bounded
  buffer of compact lines (default 2,000 lines, 4,096 characters per line) with sequence counters.
- Virtualized, soft-wrapped terminal view (only visible rows are laid out), follow-tail with a
  Latest output button, input line with Enter, Up and Down history with draft restore, private
  input when the server echoes, optional grey local echo, status line with Disconnect and
  Reconnect, activity markers (tab title and unseen line counts in the Sessions panel).
- Opt-in perf probe compatible with the C# harness, opt-in window screenshot, footprint sampler.

## Remaining (Milestone 2 onward)

TLS, NAWS, TTYPE/MTTS, GMCP, MSDP, MSSP; Latin-1; prompt events to consumers; password prompt
detection; settings and persistence; saved worlds; directory; layout persistence; selection and
copy polish; everything listed as Deferred in `docs/parity.md`.

## Tests run

| Command | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets -- -D warnings` | clean (also with `--features wandur-app/glow`) |
| `cargo test` | 55 passed: core 45, app 8, bench 2 |
| `cargo build --release` | ok |

Core tests cover scrollback bounds and memory not growing with volume, line truncation, carriage
return and erase in line, style runs, local echo not disturbing a split server escape, ANSI
parsing (split sequences, defaults, private and string sequences, parameter overflow), SGR
colours and bold-bright, telnet (escaped IAC, commands split across reads, negotiation without
loops, prompt marks, bounded subnegotiation, every byte value at every split point), UTF-8
splits, endpoint parsing, and loopback session lifecycles (connect, receive, private echo, send,
server close; user disconnect; refused connection; backpressure with a 2 MB flood through an
8 KB inbox, nothing lost). App tests cover output reaching the buffer with activity marking,
private input not echoed or kept, history with draft restore, wrap layout and row jobs.

## Known limitations and suitability findings

- **Text selection**: rows are selectable egui labels, and selection can span the rows on screen.
  Because the terminal is virtualized, selecting beyond the visible rows (drag to auto-scroll) is
  not supported, a selection is lost when its rows scroll out, and a soft-wrapped line copies with
  a newline at each wrap. A proper transcript selection needs a custom selection model over the
  buffer (line and column anchors). Not exercised by a person in this milestone.
- **IME**: egui's text input receives composition events from winit, so the input line should
  accept IME text, but it was not tested (no CJK fonts are bundled, see below).
- **Accessibility**: eframe enables AccessKit by default and labels expose their text, but only
  the visible rows exist in the tree, so a screen reader cannot read back the scrollback.
  Untested with VoiceOver.
- **Font fallback**: only egui's bundled fonts (Ubuntu Light and Hack). There is no system font
  fallback: U+25CF (●) rendered as a box in the first screenshot, so the status dots are now
  painted. CJK and many symbols will show as boxes until fonts are added explicitly. Wrapping
  counts characters, so wide and combining characters misalign.
- **Bold**: no bold monospace face is bundled; bold shows only as the bright colour.
- **HiDPI**: handled by eframe; the 2x screenshots are sharp at 1200 x 780 points.
- **Theme**: egui follows the system theme unless forced; the first build rendered light panels
  on a light-mode Mac until the dark preference was set explicitly.
- **Graphics memory**: about 165 MB of the footprint appears whenever frames are drawn (IOSurface
  and Metal allocations from wgpu) and goes away when they stop. Not yet explained or tuned.
- **Redraw policy**: the app redraws at the display rate (about 60 Hz) while output flows; that
  is where nearly all of its CPU goes. Not yet capped.
- **Allocation churn**: egui rebuilds layout jobs and shapes each frame (1 to 2 MB a frame).
  Short-lived, not retained, but a high allocation rate.
- **Keyboard**: Enter, Up and Down are wired; no Cmd/Ctrl shortcuts yet (copy relies on egui's
  label copy). The UI keyboard path has no automated test; history and submit are tested at
  the model level.
- **Platforms**: built and run only on macOS (Apple Silicon). The code has no macOS-only paths
  outside the probe's process readings, but Linux and Windows are untested.
- **Docking**: egui_dock tabs, splits and floating windows come from the library; they were seen
  working in screenshots but not dragged around by a person here. Layout is not saved.

## Performance and comparison with C#

See `docs/measurements.md` for the tables and method. In short, with the same harness, server
and scenarios:

| | Rust | C# |
|---|---|---|
| Startup to first frame | 185 to 330 ms | about 1,100 to 1,200 ms (683 ms packaged) |
| Working set | 83 to 90 MB | 174 to 277 MB |
| Footprint, nothing drawing / drawing / 4 sessions flooding | 66 / about 230 / 236 MB | 224 / 258 to 363 / 520 to 533 MB |
| Heap | 2 to 5 MB | 27 to 134 MB (GC) |
| Terminal model, 2,000 lines | 328 KB | about 6.9 MB |
| CPU at 1 MB/s, at 4 x 50 KB/s | 21%, 22% | 29 to 30%, 36 to 42% |
| Idle CPU, no session / with sessions | 0.2% / 2.1 to 2.4% | 2.6% / 6.5 to 8.3% |
| Keeps up at 1 MB/s | yes | yes |
| Allocation rate under flood | 75 to 83 MB/s | 11 to 70 MB/s |
| UI latency | not comparable (display-paced frames) | |

## Verdict

**Better on startup, memory and CPU; equal on throughput; worse on allocation rate;
inconclusive on latency.** The memory advantage is real but smaller than the working set
suggests while the window is drawing, because about 165 MB of graphics memory appears with
rendering. egui, eframe and egui_dock look suitable for the workspace and the terminal. The open
risk is text selection over a virtualized transcript, which will need custom work; IME and
screen reader support are plausible but unproven.

**Recommendation: continue to Milestone 2**, and include early in it a selection spike (a
selection model over the buffer) and a redraw cap experiment. Those two decide whether the
terminal can match the C# xterm display without large custom effort.
