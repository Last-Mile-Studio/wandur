# Milestone 4 report

2026-10-08. Performance-focused implementation: the brief's ten Milestone 4 items, each verified
by measurement, changed where a measurement showed a gain, and recorded before and after in
`docs/measurements.md`. Plus three fixes from the Milestone 3 review and a final same-day
comparison with the C# client. Screenshots are in `.superpowers/shots/m4/` (not committed):
`m4-pinned-hull.png` (default layout, Hull, the light map and trimmed channel lines),
`m4-pinned-ember.png`, `m4-unpinned-strips.png` (Workspace and Map unpinned to the left and right
strips), `m4-unpinned-overlay.png` (Map slid out over the workspace, Ember). Everything ran on
local data only: loopback MUD and directory servers, generated art, no external host.

## Implemented

- **The grid's text as one reused mesh** (`grid_text.rs`). Profiling with a new whole-app
  headless bench (`wandur-bench shell`, time and allocation per panel) showed the cost under a
  flood was not the idle side panels but the terminal: every visible run was shaped and laid out
  again each frame because the rows on screen are new every frame (1.6 ms and 1.3 MB per frame).
  Now each character's glyph quad is cut once from a one-character galley and copied into a mesh
  whose buffers are reused: 0.2 ms and 11 KB per frame. Fallback, wide and combined characters
  still go through galleys at their cells; underline and strike are lines; no ligatures.
- **AtlasWatch**: galleys kept across frames (glyph quads, Channels rows) are dropped when egui
  rebuilds its font atlas; the Milestone 3 Channels cache could have drawn stale glyphs then.
- **Channels**: the selected tab's row list is rebuilt only when the log, the tab or the width
  changes (0.16 to 0.05 ms per frame with 500 messages); the world's printed tag and speaker are
  trimmed from the text ("Ann: [gossip] Ann: hi" becomes "Ann: hi"), with colours kept; the All
  tab names the channel.
- **JPEG thumbnails**: the cover is cropped before the final resize (same pixels; 30.7 to 27.8 ms,
  peak 13.8 to 7.8 MB). zune-jpeg and libjpeg-turbo were measured and not adopted (below).
- **Background work**: trimming the thumbnail cache moved from the UI thread at start (up to 14
  ms) to the first artwork worker. The probe records a histogram of UI-thread time per frame, and
  a test drives the whole app under a flood and fails on a long frame.
- **Map follows the theme**: a light theme with a dark preset map (Hull, the default) now draws a
  light map. Note: the C# client draws Hull's map dark (`#11171B` in `UserTheme.cs`); this is a
  deliberate difference, as asked.
- **Pin and auto-hide** for Workspace, Map and Channels (`autohide.rs`), which egui_dock does not
  have: unpin from the tab's context menu ("Auto hide") or View, Pinned; the panel moves to a
  labelled strip on its nearest edge; hovering its strip tab slides it out over the workspace,
  clicking keeps it out; Escape, Hide, a click elsewhere or moving away for 400 ms (when opened by
  hovering) hides it; Pin puts it back in its leaf, or split back beside what was on the other
  side with the same fraction. Saved in `layout.json` (`auto_hide`) and restored on start.
  Documents (sessions, Find a MUD, Settings) cannot be unpinned.
- **Documentation**: what runs on the UI thread, what starts at launch, and where scripting and
  room classification would plug in (traits, cargo features, worker threads), in
  `docs/architecture.md`. The overall assessment is `docs/verdict.md`.

### The brief's ten items

| Item | Status in M4 | Evidence |
|---|---|---|
| 1. No transcript reconstruction for activity | Holds since M2 (line sequence numbers, revisions) | unchanged; `has_unseen` compares counters |
| 2. Bounded scrollback, old rows released | Confirmed: flat at the bound, lowering releases at once | memprobe: 8,166 KB before and after 20,000 more lines; 8,166 to 3,842 KB at 500 rows |
| 3. One canonical terminal model | Holds: the alacritty grid; line events and transcript read from it | the only per-frame second structure is the reused glyph mesh |
| 4. No needless per-character allocations | Done: the grid's text costs 11 KB per frame | 1,319 to 11 KB per frame (shell bench) |
| 5. Thumbnails near target size | Holds; crop-first JPEG | peak 13.8 to 7.8 MB |
| 6. Lazy optional features | Confirmed; plug-in points documented | startup heap 3.8 MB |
| 7. Bounded, cancellable image loading | Holds since M3 | 300-card run: textures within 24 MB |
| 8. Batch output, coalesce | Holds since M2 (30 output frames a second) | steady stream 33 frames a second, keeps up |
| 9. Background work off the UI thread | Audited; one move; stall probe | no UI frame over 16.7 ms in any harness scenario |
| 10. Measure before and after; drop what does not help | Done | directory card cache dropped (0.23 to 0.22 ms) |

## What remains

- Everything marked Deferred or Partial in `docs/parity.md`: the full mapper, scripting, macros,
  credentials and auto-login, pattern-based channel capture and its teaching dialog, history,
  skins, localization, update check, links in output, diagnostics, transcript export UI.
- A person trying docking, pin and auto-hide, the directory with a mouse, and a screen reader;
  IME input (CJK); Windows and Linux builds.
- Optional: a libjpeg-turbo path if JPEG decode time ever matters more than build simplicity.

## Tests run

| Command | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets -- -D warnings` | clean |
| `cargo test` | 167 passed: core 80, term 22, app 60, bench 5 (including the stall test) |
| `cargo build --release` | ok |
| `cargo check --target wasm32-unknown-unknown -p wandur-core --no-default-features` | ok |
| `wandur-bench micro --label m4`, `wandur-bench shell --label m4`, `--example memprobe` | `.superpowers/perf/` |
| C# harness, both clients, same day: startup, session-idle, flood-100k, flood-1m-nochat, multi-4-nochat, multi-8-idle, directory; Rust also without the ticker; the M3 binary again for before and after; steady-stream runs for M3, M4 and C# | `docs/measurements.md` |

New tests: the glyph mesh (one quad per glyph, each character laid out once, no layouts for known
glyphs); channel prefix trimming (tags, verbs, colours kept, no false trims); the map's lightness
for every theme; the crop-first JPEG (no stretch for small pictures, centre preserved); the frame
histogram; the stall test; auto-hide in the model (edges, exact restore for every tool panel,
shared leaves, missing neighbours, hover and click hiding rules), in the layout file (round trip,
restart, conflicting and invalid entries) and headlessly through the app (unpin, strip tab click,
Escape, restart, pin back).

## Known limitations

- Process CPU under output moved less than the UI pass (1 to 3 points against 40% less UI-thread
  time): presenting frames through wgpu and Metal is now most of it, and the terminal model's
  parsing is about 1 ms per frame at 1 MB/s. Neither is reachable from the UI code.
- The idle Rust client allocates more than C# when frames are forced (the probe's ticker): egui
  rebuilds the UI each frame it draws. Without forced frames it is near zero.
- The glyph mesh places glyphs on whole pixels and draws no ligatures (as most terminals); text
  outside the plain set still goes through egui's layout per frame (rare in MUD output).
- Auto-hide is ours, not egui_dock's: an unpinned panel cannot be dragged from its strip into the
  dock (pin it first), and the strips do not follow egui_dock's own tab styling exactly.
- JPEG decoding stays 1.8 times slower than C# (libjpeg-turbo there).

## Performance and comparison

Before and after (from `docs/measurements.md`):

| Optimization | Before | After |
|---|---:|---:|
| Grid text as one mesh: UI pass under a flood, ms / KB per frame | 3.14 / 1,455 | 1.85 / 139 |
| same: steady 1 MB/s, CPU % / alloc MB/s | 16.1 to 18.2 / 70 | 15.1 to 16.3 / 30 |
| same: steady 4 x 250 KB/s | 20.9 to 22.5 / 78 | 19.1 to 19.2 / 40 |
| Channels row list, ms / KB per frame | 0.164 / 30 | 0.049 / 26 |
| Directory card cache (dropped) | 0.230 / 66 | 0.219 / 62 |
| JPEG crop-first, 4000x3000 to 800x320: ms, peak | 30.7, +13.8 MB | 27.8, +7.8 MB |
| Thumbnail cache trim at start, on the UI thread | 3.5 to 13.8 ms | 0 (worker) |

Final, same day, Rust M4 against C# (medians; Rust no-ticker values in brackets):

| | Rust M4 | C# |
|---|---:|---:|
| Startup to first frame (ms) | 253 to 373 | 1,041 to 1,126 |
| Working set: idle / 1 MB/s / 4 sessions / directory (MB) | 85 / 91 / 107 / 87 | 206 / 201 / 260 / 249 |
| Footprint: idle / 1 MB/s / 4 sessions / directory (MB) | 234 / 241 / 257 / 276 | 225 / 472 / 519 / 295 |
| Heap: idle / 1 MB/s / 4 sessions (MB) | 3.8 / 10.7 / 31.1 | 26.9 / 56.0 / 114.4 |
| CPU, idle session (%) | 10.9 (1.8) | 7.5 |
| CPU, flood-1m-nochat / multi-4-nochat, bursty (%) | 17.8 (9.4) / 16.9 (8.9) | 30.7 / 37.1 |
| CPU, steady 1 x 1 MB/s / 4 x 250 KB/s (%) | 15.7 / 19.1 | 35.3 / 70.4 |
| Directory filter / sort (ms) | 0.023 / 0.017 | 2.256 / 0.573 |
| JPEG / PNG 4000x3000 to a plate (ms, peak) | 27.8, +7.8 MB / 77.6, +2.8 MB | 15.8, +15 MB / 89.4, +68 MB |

### Better, worse or inconclusive

- **Terminal drawing: better** than M3 (UI-thread time 40% less, allocation halved under output).
- **Against C# under sustained output: better** (CPU half to a quarter, working set less than
  half, footprint half).
- **Startup and memory at rest: better** (4 times faster; working set less than half; footprint
  equal when idle).
- **Idle CPU: better without forced frames** (1.8% against 7.5 to 8.9%), worse with the probe's
  ticker, which forces frames C# does not draw.
- **JPEG decode time: worse** (1.8 times); memory better.
- **Pin and auto-hide: equal in behaviour** to Dock.Avalonia's for the three tool panels; it had
  to be written (about 400 lines with tests).
- **Accessibility, IME, Windows and Linux: inconclusive** (untested).

## Verdict

See `docs/verdict.md`. In short: the Rust engine is the better foundation on every measured axis
except JPEG decode time; the UI costs more effort to build and leaves accessibility, IME and the
other platforms unproven. Continue only if the owner wants a long-term replacement; otherwise
carry the lessons into the C# client.

## Try it

```sh
cd "/Volumes/Extreme SSD/workspace/wandur/wandur-client-rust"
cargo build --release

# A small GMCP world (rooms for the Map, chatter for the Channels) and a local directory
./target/release/wandur-bench mud-server --port 4403 --rate 0 --page gmcp &
./target/release/wandur-bench directory-server --port 4402 --worlds 300 --varied &

WANDUR_DIRECTORY_URL=http://127.0.0.1:4402 ./target/release/wandur 127.0.0.1:4403 --data-dir /tmp/wandur-try
```

Pinning: right-click the Map tab, Auto hide (or View, Pinned, untick Map). Map moves to the right
strip. Hover its strip tab to slide it out, move away to hide it; click the strip tab to keep it
out, Escape or Hide to put it away; Pin in its header to dock it back above Channels. Quit and
start again: it is still unpinned. `--show unpinned:map` slides it out at start. Reset layout
(View) pins everything back into the default layout.

A flood with the new renderer: `./target/release/wandur-bench mud-server --port 4400 --rate 1000000
--write-ms 2 &` then connect to `127.0.0.1:4400`. Measurements: `cargo run --release -p
wandur-bench -- shell --label mine` (per-panel costs), `-- micro --label mine`, and
`--example memprobe`. Stop the servers with `kill %1 %2`.
