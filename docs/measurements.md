# Measurements

Parity task checks first (newest first), then Milestone 4 (with the final Rust against C# table), then Milestones 3, 2 and the Milestone 1
record.

## Script formatting (2026-10-09, on 93ff451)

Biome's JavaScript formatter in the script editor (`wandur-format`). Same machine, busy (load
average 4 to 15), release builds of 93ff451 and of this change run alternately, ten runs each,
idle with no session (probe `idle`, 4 s settle, 8 s; footprint read at 9 s; fresh data dir each).
Raw lines: `.superpowers/formatter/measure-idle.txt`.

| | Before | After | Change |
|---|---|---|---|
| Binary as built (line tables) | 38,106,904 B | 42,800,424 B | +4.69 MB (+12.3%) |
| Binary stripped | 28,714,976 B | 32,288,144 B | +3.57 MB (+12.4%) |
| Startup to usable, median ms | 271 | 255 | noise (spread 194 to 320 both) |
| Live heap at idle | 3.9 MB | 3.9 MB | none |
| Physical footprint at idle, median | 76.5 MB | 76.7 MB | none (spread 73.5 to 76.9 both) |
| Working set at idle, median | 89.2 MB | 89.8 MB | noise (spread 88.7 to 104 both) |

The first run of a freshly built binary takes about a second longer to start whichever build it
is (pages read from the exFAT disk); it is left out. Nothing of the formatter runs at start, so
idle memory is unchanged; a test checks in a fresh process that it is first used by the first
script shown.

Format times (release, `wandur_format::javascript`): the first script about 2.6 ms (code paged
in, options built); after that 0.06 to 0.19 ms for the starter, the help scripts and the pack
fixtures (220 to 810 bytes); a 60 KB script 11 ms; a cached script 0.1 to 0.2 µs. Formatting a
typical script raises peak resident memory by about 1.6 MB (standalone, code pages included).

Formatters compared (standalone binaries, stripped size over a 0.39 MB empty program; peak
resident memory formatting a 130 KB script; time for it):

| | Added size | Peak memory | 130 KB | Licences |
|---|---|---|---|---|
| biome_js_formatter 0.5.7 (chosen) | 3.7 MB | 11.6 MB | 33 ms | MIT OR Apache-2.0 |
| dprint-plugin-typescript 0.96.1 | 4.2 MB | 59.9 MB | 37 ms | MIT, Apache-2.0, and smartstring (MPL-2.0) |
| prettify-js 0.1.0 | 0.1 MB | 3.4 MB | 5 ms | BSD-2-Clause, MIT (output poor, no syntax check) |
| StyLua 2.5.2 (Lua) | 2.9 MB | | | MPL-2.0 (not used) |

## ui-chrome (2026-10-09, HEAD 3707895)

The window and panel chrome brought to the C# client's newest look (headers, drop preview, title
bars, the macOS menu bar, the menu button, the UI review polish). Same machine and day as the
final audit but busier (load average 7 to 9), so both builds were run alternately in the same
minutes: this build (`.superpowers/bin/wandur-uichrome`) against the final audit build
(`.superpowers/bin/wandur-final`), three runs each, through `.superpowers/scripts/steady-final.sh`
and `.superpowers/audit/idle.sh` (raw lines in `.superpowers/ui-chrome/steady1.txt`,
`steady4.txt`, `idle.txt`). Medians:

| Scenario | CPU % ui-chrome / final | Alloc MB/s | Working set MB | Frames a second |
|---|---:|---:|---:|---:|
| 1 session at 1 MB/s | 21.5 / 21.6 | 40 / 45 | 126 / 122 | 35.6 / 35.0 |
| 4 sessions at 250 KB/s | 19.3 / 22.5 | 46 / 58 | 133 / 127 | 41.0 / 42.4 |
| Idle, one quiet session | 1.8 / 1.5 | 2.8 / 3.4 | n/a | 3.1 / 3.2 |

No regression from the chrome work: CPU and frame rates are within this machine's run-to-run
spread (the final audit's own steady 1 MB/s figure was 10.3% on a quieter machine; today both
builds read about 21%), allocation is a little lower (the stretched dock tab and the header
plan allocate less than the old per-leaf header painting and tab titles), and the working set is
up by 2 to 6 MB (the native menu bar and the header state). Hover fades run once per change and
never continuously; idle frames are unchanged at about 3 a second.

## Final audit (2026-10-09, HEAD 9e55a7c)

The shared C# harness on three clients in one session, alternating: the final Rust build
(`.superpowers/bin/wandur-final`), Milestone 4 (`.superpowers/bin/wandur-m4`) and C# (27f66d2).
Commands: `.superpowers/final-harness.sh` (harness scenarios, then the steady stream through
`.superpowers/scripts/steady-final.sh`, which keeps its data directories under `.superpowers/`);
medians with `.superpowers/scripts/medians.py rust-final rust-m4again csharp-final` (and the
`-startup`, `-notick` labels). The machine was busy (load average 5 to 7).

Steady stream, no ticker:

| Scenario | CPU % final / M4 / C# | Alloc MB/s | Working set MB | Heap MB (Rust live / C# GC) |
|---|---:|---:|---:|---:|
| 1 session at 1 MB/s | 10.3 / 9.1 / 33.6 | 43 / 29 / 85 | 105 / 91 / 217 | 13.9 / 10.5 / 95 |
| 4 sessions at 250 KB/s | 24.2 / 15.0 / 61.7 | 56 / 39 / 125 | 158 / 122 / 313 | 37.6 / 31.1 / 171 |

(4 sessions: 5 runs final and M4, ranges 13.2 to 25.5 and 6.8 to 15.9; C# 2 runs, 58.7 to 64.7.
Final with history recording off: 8.1 to 22.0.)

Harness, ticker on, medians of 3 (startup 6):

| Scenario | Startup ms final / M4 / C# | Working set MB | Heap MB | CPU % | UI p95 ms |
|---|---:|---:|---:|---:|---:|
| startup | 377 / 370 / 1,046 | 89 / 86 / 157 | 3.9 / 3.8 / 26.9 | 10.9 / 10.7 / 2.5 | 0.4 / 0.4 / 0.1 |
| session-idle | 349 / 289 / 1,136 | 92 / 86 / 168 | 8.1 / 6.2 / 43.7 | 11.8 / 7.2 / 5.5 | 0.5 / 0.4 / 0.1 |
| flood-1m-nochat | 622 / 398 / 1,060 | 110 / 92 / 190 | 14.0 / 10.6 / 59.2 | 23.3 / 17.3 / 20.2 | 1.0 / 0.9 / 4.8 |
| multi-4-nochat | 472 / 342 / 1,152 | 117 / 107 / 203 | 36.1 / 31.1 / 114.5 | 14.7 / 6.1 / 29.6 | 1.5 / 0.3 / 0.3 |
| directory | 295 / 270 / 1,238 | 90 / 87 / 131 | 5.9 / 5.4 / 37.2 | 5.0 / 4.6 / 29.9 | 0.2 / 0.3 / 0.1 |

No ticker, final / M4: startup 0.5 / 0.3% CPU; flood-1m-nochat 14.4 / 3.9%; multi-4-nochat 8.8 /
4.1%; session-idle 7.9 / 0.8% in the harness (two of three final runs repainted for a while), but
seven separate idle runs (`.superpowers/audit/idle.sh`, fresh data dir each) read 0.8 to 1.4% at
3.4 frames a second, M4's frame rate.

Every flood kept up (1.0 M chars/s; 200 K for four sessions). Against C# the final build keeps
the Milestone 4 lead on startup, memory and CPU under sustained output; against Milestone 4 it
pays about +9 CPU points and +17 MB/s allocation with four busy sessions, the sum of the per-line
work added in t03 to t12 (each task's check stayed within its limit against the build before it).
Discussion: `docs/parity-report.md`.

## Word wrapping (ui/reading)

2026-10-09. What the change adds to a frame: the transcript's rows on screen are laid out as
display rows (word wrapped) before painting; the layout is kept while the grid's revision, the
scroll offset and the size do not change, so under a flood it is redone every output frame and
when idle never. Painting reads the same cells as before, in one or two slices per row.

Shell bench, release, `wandur-bench shell --frames 360`, the base (main 07e617f) and this branch
with word wrapping on and off (`WANDUR_BENCH_WRAP=0`), six runs each in alternating rounds
(`.superpowers/reading/perf/run.sh`, raw output beside it), medians. The Session column is the
session panel's own time (the transcript, composer and footer), the part this change touches:

| Scenario | UI ms base / on / off | Session ms base / on / off |
|---|---:|---:|
| flood 1 MB/s, panels empty (harness-like) | 1.469 / 1.566 / 1.498 | 0.150 / 0.181 / 0.142 |
| flood 1 MB/s, panels with data | 1.616 / 1.746 / 1.381 | 0.161 / 0.170 / 0.140 |
| 4 sessions x 250 KB/s, panels with data | 1.598 / 1.680 / 1.440 | 0.126 / 0.155 / 0.135 |

Word wrapping costs 0.01 to 0.03 ms a frame in the session panel under a flood (the generator's
room descriptions and long lines wrap at the bench's 100 or so columns); the whole frame moves
within the run-to-run spread of this machine (the logic column, which the change does not
touch, moved as much). Off, the session panel is at or under the base.

## Full map and mini map (ui/full-map)

2026-10-09. What the change adds to a frame: the docked Map panel is a mini map (fewer rooms in
view, no status line); a session on its Map page draws its full map over a strip of the newest
grid rows (the transcript is not drawn); side by side draws a narrower transcript and the full
map. A hidden full map is not drawn at all (a test counts its frames).

Shell bench, release, `WANDUR_BENCH_SESSION_VIEW=play|map|split wandur-bench shell --frames 360`,
three alternating rounds per view (`.superpowers/full-map/perf/run.sh`, raw output beside it),
medians; the flood scenarios with panel data map 80 rooms, the empty-panels one none:

| Scenario | Play ms | Map page ms | side by side ms | logic ms play / map / split | CPU % play / map / split |
|---|---:|---:|---:|---:|---:|
| flood 1 MB/s, panels empty (harness-like) | 1.490 | 1.461 | 1.133 | 0.956 / 0.973 / 0.721 | 12.2 / 11.6 / 10.8 |
| flood 1 MB/s, panels with data | 1.444 | 1.522 (+5.4%) | 1.276 | 0.884 / 0.925 / 0.763 | 12.1 / 12.4 / 10.9 |
| 4 sessions x 250 KB/s, panels with data | 1.649 | 1.709 (+3.6%) | 1.394 | 1.006 / 1.064 / 0.880 | 16.8 / 17.8 / 14.2 |
| idle session, panels with data (frames forced) | 0.665 | 0.812 (+22%) | 0.732 | 0.037 / 0.050 / 0.032 | 1.7 / 2.7 / 1.6 |

Play with the mini map is the like-for-like row against main: the t16 figures above read 1.675,
1.687 to 1.752, 1.760 to 1.850 and 0.756 ms for these scenarios on a busier day; the frame time
here is at or under them. The Map page costs at most 0.08 ms more under a flood (its 80 rooms
and labels replace the transcript's rows), and 0.15 ms on an idle frame that is forced to
repaint (the idle app does not repaint, so this is not a running cost). Side by side is cheaper
than Play under a flood: the narrower transcript has fewer glyphs, and the mini map and full map
together stay under 0.2 ms (Session panel 0.11 to 0.20 ms, mini map 0.02 to 0.05 ms). The C#
harness scenarios were not rerun: they drive the app through the perf probe, which this change
does not touch, and the shell bench covers the same frames in process.

## Map editor (ui/map-editor)

2026-10-09. What the change adds outside Edit mode: nothing per room; the editor's highlights,
exit hit lines, overlays, toolbar and inspector run only while Edit is on. Shell bench, release,
`WANDUR_BENCH_SESSION_VIEW=map wandur-bench shell --frames 360 --only "panels with data"`, the
main build (07e617f) against this branch, three alternating rounds, medians
(`.superpowers/map-editor/perf/`):

| Scenario (Map page, Edit off) | before ms | after ms | Session panel ms before / after | CPU % before / after |
|---|---:|---:|---:|---:|
| flood 1 MB/s, panels with data | 1.589 | 1.701 | 0.170 / 0.173 | 14.4 / 14.3 |
| 4 sessions x 250 KB/s, panels with data | 1.635 | 1.608 | 0.146 / 0.152 | 18.8 / 19.0 |
| idle session, panels with data (frames forced) | 0.780 | 0.821 | 0.183 / 0.207 | 2.9 / 3.0 |

The rounds spread widely (the flood's before rounds read 1.53 to 2.23 ms), so the idle page was
measured again with 600 frames and five alternating rounds: 0.814 ms before and 0.819 ms after
(medians), the Session panel that draws the full map 0.193 and 0.200 ms, allocation the same
(42 KB a frame). The Map page outside Edit mode costs within 0.01 ms of main.

## Parity t16 check (final run: settings, updates, accessibility)

2026-10-09. What t16 adds to a frame: the update schedule (one look a frame at a deadline, the
check itself on its own thread), the update strip when one is offered, and accessible names. The
names cost nothing without a screen reader: egui builds no AccessKit tree then, and the work only
a screen reader needs (dock tab titles, separator rectangles) is skipped after one check. The
text size default moved from 13 to the C# 15 points, so the bench was run at both.

Final shell bench, every scenario, three alternating rounds of the t15 binary
(`.superpowers/bin/wandur-bench-t15`), t16 at 13 points (`WANDUR_BENCH_FONT_SIZE=13`, like for
like) and t16 at its 15 point default (`.superpowers/t16-shell.sh`, raw output in
`.superpowers/perf/t16/`, summary `.superpowers/t16-summary.py`), means:

| Scenario | t15 ms | t16 at 13 pt ms | change | t16 at 15 pt ms | change | logic t15 / t16-13 / t16-15 | CPU % t15 / t16-13 / t16-15 |
|---|---:|---:|---:|---:|---:|---:|---:|
| directory page, 300 worlds with art (frames forced) | 0.636 | 0.613 | -3.6% | 0.638 | +0.4% | 0.023 / 0.025 / 0.026 | 2.4 / 2.3 / 2.3 |
| flood 1 MB/s, panels empty (harness-like) | 1.635 | 1.647 | +0.7% | 1.675 | +2.5% | 1.031 / 1.034 / 1.059 | 14.2 / 14.4 / 14.5 |
| flood 1 MB/s, 1,000 macro triggers, panels empty | 1.750 | 1.759 | +0.5% | 1.891 | +8.1% | 1.140 / 1.144 / 1.244 | 15.5 / 15.3 / 15.7 |
| flood 1 MB/s, a script trigger, panels empty | 1.856 | 1.859 | +0.2% | 1.882 | +1.4% | 1.237 / 1.226 / 1.268 | 20.3 / 20.1 / 19.7 |
| flood 1 MB/s, panels with data | 1.700 | 1.687 | -0.8% | 1.847 | +8.6% | 1.019 / 1.021 / 1.117 | 14.7 / 14.5 / 15.1 |
| flood 1 MB/s, session tab only | 1.419 | 1.618 | +14.0% | 1.633 | +15.1% | 0.975 / 1.112 / 1.113 | 12.9 / 14.0 / 14.4 |
| 4 sessions x 250 KB/s, panels with data | 1.748 | 1.898 | +8.6% | 1.850 | +5.9% | 1.056 / 1.161 / 1.140 | 19.7 / 19.2 / 19.6 |
| idle session, panels with data (frames forced) | 0.720 | 0.747 | +3.7% | 0.756 | +5.0% | 0.037 / 0.042 / 0.043 | 2.8 / 2.7 / 2.8 |
| idle session, session tab only (frames forced) | 0.495 | 0.504 | +1.7% | 0.487 | -1.7% | 0.038 / 0.042 / 0.039 | 1.8 / 1.8 / 1.7 |

The session-tab-only row (+14%) came from naming the dock's tabs every frame whether or not a
screen reader was listening; after making that work wait for an AccessKit tree, three more
alternating rounds of the rows that moved (`.superpowers/bin/wandur-bench-t16b`,
`.superpowers/perf/t16b/`) read:

| Scenario | t15 ms | t16 ms | change | logic ms t15 / t16 | CPU % t15 / t16 |
|---|---:|---:|---:|---:|---:|
| flood 1 MB/s, session tab only | 1.449 | 1.513 | +4.4% | 0.996 / 1.044 | 13.3 / 14.0 |
| 4 sessions x 250 KB/s, panels with data | 1.817 | 1.760 | -3.1% | 1.099 / 1.089 | 20.0 / 18.8 |
| flood 1 MB/s, panels with data | 1.768 | 1.752 | -0.9% | 1.047 / 1.043 | 14.8 / 15.4 |

Every row is within the plan's 20% line and, after the fix, within 10% of t15 (the larger 15
point text costs up to +8% on two flood rows: more glyphs a frame). The absolute frame times
vary with the machine's load from day to day (t14 read 0.80 to 1.08 ms for the flood rows, t12
1.46 to 1.94 ms), so each task's check compares against the binary before it; the checks above
show where the time went across t01 to t16, none over its target.

The C# harness scenarios (startup, steady stream, bursty server, footprint, directory scroll)
were not rerun for t16: they drive the app through the perf probe, which t16 does not touch, and
the shell bench covers the same frames in process.

## Parity t15 check (skins, world themes, custom themes)

2026-10-09. The skins draw on the UI thread every frame: the title band, frame and plate are
baked once into meshes (keyed by the window, the plate and the colours) and reused; the panel
header gradients, the plate's icon and title, and the title buttons are drawn each frame.

Shell bench A/B against the t14 binary (base 22fecf8), `WANDUR_BENCH_SKIN=Fleet|Armored|System
wandur-bench shell --frames 480 --only ...`, five alternating rounds, release, medians of the
mean UI ms per frame (`.superpowers/perf/shell-t15.tsv`, `.superpowers/t15-shell.sh`):

| Scenario | t14 (no skin) | Fleet | Armored | System |
|---|---:|---:|---:|---:|
| idle session, panels with data (frames forced) | 0.897 | 0.986 (+9.9%) | 0.976 (+8.8%) | 0.938 (+4.6%) |
| flood 1 MB/s, panels with data | 2.072 | 2.131 (+2.8%) | 2.110 (+1.8%) | 2.152 (+3.9%) |
| tessellation, idle (ms) | 0.288 | 0.296 | 0.391 | 0.274 |

Every skin is within the plan's 15%. Armored's tessellation is about 0.1 ms more (its baked
plates are large meshes copied into the frame); the first unbaked version cost +115% idle, which
is why the frame is baked. The machine was noisy (single runs varied two to three times); the
medians of five rounds are what the table shows.

Theme switch: resolving the theme, the skin's colours and the egui visuals takes well under 10 ms
for every preset in every skin, with and without a world theme
(`app::tests::a_theme_switch_takes_under_ten_milliseconds` asserts the worst case under 10 ms;
it runs in debug builds too).

## Parity t14 check (local model agents)

2026-10-09. Model calls (and the credential read) run on threads of their own; the UI thread only
polls the runner (a channel read) and feeds public output into the agent's observation, for every
session with an agent whether or not a run is going.

`WANDUR_MICRO_ONLY_AGENT=1 wandur-bench micro --label t14-agent` (`.superpowers/perf/micro-t14-agent.md`,
release, three runs, medians of 41 and 201):

| Measurement | ms | Where |
|---|---:|---|
| Agent observation feed, 1 MB of flood | 0.12 to 0.20 per MB | UI thread |
| Agent observation text (60 lines, chat filtered) | 0.013 to 0.020 per build | UI thread, once per decision (cached by revision while a run waits) |

At 1 MB/s the feed is about 0.02% of a core (the text room observer is 1.75 ms per MB). Shell
bench flood rows, one run (`.superpowers/perf/shell-t14.md`): 0.80 to 1.08 ms a frame, logic 0.59
to 0.68 ms, against t12's 1.46 to 1.94 and 1.05 to 1.31 on a busier machine; not an A/B (no t13
binary was kept), but no sign of a regression. Polls never wait on the model: a test polls the
runner against a fake server slower than the response timeout (slowest poll under 50 ms) and a
loopback session test pumps 40 frames while a model call is out.

## Parity t13 check (map editor and terrain inference)

2026-10-09. Room terrain inference must not hash or classify on the UI thread (the C# client's
follow-up: its `RoomsNeedingInference` computed every room's SHA-256 inference key on the UI
thread). Here the UI thread only copies the text of rooms without terrain and compares room
revisions; keys and classification run on the session's inference worker.

`WANDUR_MICRO_ONLY_MAP=1 wandur-bench micro --label t13-map` (`.superpowers/perf/micro-t13-map.md`,
release, a map of 10,000 rooms without terrain, 200-character descriptions):

| Measurement | ms | Where |
|---|---:|---|
| Inference scan (512 queued) | 0.06 | UI thread |
| Scan with every room already checked | 0.16 | UI thread |
| Apply 512 results | 0.05 | UI thread |
| Whole map end to end with the keyword stand-in: worst frame | 0.42 | UI thread (32 frames, 7.0 ms in all, 54 ms wall) |
| Inference keys of all 10,000 rooms (what C# did per scan) | 27.5 | worker |

The ONNX encoder (feature `classifier`, the 0.1.1 package, release): verifying the package (SHA-256
of 90 MB) and optimizing the encoder 0.93 s, once, on the worker; about 30 ms a room (23 parity
fixtures in 0.69 s). Map frames are unchanged from t12 (1.04 / 0.11 / 0.75 ms against 1.04 / 0.10 /
0.72); the text observer row is 1.82 ms per MB (t12 1.75; a 5-run sample earlier read 2.76, noise).
The default build does not contain the classifier (`cargo tree -p wandur-app` lists no tract crate).

## Parity t12 check (the mapper)

2026-10-09. The map tracks rooms from GMCP, MSDP and room text, saves them per world on a worker
thread and draws the floor every frame the Map panel is visible.

Map frames (`WANDUR_MICRO_ONLY_MAP=1 wandur-bench micro --label t12-map`,
`.superpowers/perf/micro-t12-map.md`, medians of 200 frames, release): the whole panel with a
2,000-room map (a 50 by 40 block, every room linked east and north, four terrains), drawn
headless at 800 x 600 and tessellated:

| Frame | ms | KB allocated | rooms drawn |
|---|---:|---:|---:|
| all 2,000 rooms fitted on screen | 1.04 (p95 1.14) | 11,923 | 2,000 |
| zoom 1 with labels | 0.10 | 1,212 | 77 |
| all fitted, grid mode | 0.72 | 6,048 | 2,000 |

The first version took 3.9 ms fitted. What brought it under 2 ms: a per-floor index of rooms and
exits by tracker position, rebuilt only when the map, area or floor changes (it was four hash
maps a frame, 1.5 ms); one shape per room for fill and border; one path per arrowhead, and no
arrowheads on lines shorter than 10 points; exit lights drawn as small squares when rooms are
smaller than 16 points (they are a pixel or two across there). The KB column is mostly egui's
tessellated mesh.

Text room observer over the flood (`micro` row "Text room observer"): 1.75 ms per MB of server
text (three runs, 1.74 to 1.80). Every line of a world without room data goes through it, as in
C#. The first version cost 5.2 ms per MB; the changes: plain text copied a run at a time between
control bytes (memchr), whole colour sequences read at once without allocating, the exits heading
and direction words matched by hand before any pattern, the failed-move search only while a move
or a walk waits for its answer, line buffers reused.

Shell bench, three alternating runs each of t11 (`.superpowers/bin/wandur-bench-t11`) and t12
(`.superpowers/bin/wandur-bench-t12`), `.superpowers/perf/shell-t12-base*.md` and
`shell-t12-run*.md`, means:

| Scenario | t11 ms/frame | t12 ms/frame | change | logic ms t11 / t12 | process CPU % t11 / t12 |
|---|---:|---:|---:|---:|---:|
| directory page, 300 worlds with art (frames forced) | 0.482 | 0.616 | +27.9% | 0.018 / 0.023 | 1.8 / 2.3 |
| flood 1 MB/s, panels empty (harness-like) | 1.425 | 1.713 | +20.2% | 0.896 / 1.121 | 13.6 / 14.3 |
| flood 1 MB/s, 1,000 macro triggers, panels empty | 1.539 | 1.935 | +25.8% | 1.009 / 1.296 | 14.7 / 15.8 |
| flood 1 MB/s, a script trigger, panels empty | 1.615 | 1.942 | +20.3% | 1.084 / 1.313 | 19.3 / 19.7 |
| flood 1 MB/s, panels with data | 1.471 | 1.688 | +14.8% | 0.911 / 1.075 | 13.9 / 14.4 |
| flood 1 MB/s, session tab only | 1.470 | 1.459 | -0.7% | 0.997 / 1.048 | 13.5 / 13.3 |
| 4 sessions x 250 KB/s, panels with data | 1.748 | 1.841 | +5.3% | 1.066 / 1.161 | 20.7 / 21.3 |
| idle session, panels with data (frames forced) | 0.653 | 0.643 | -1.6% | 0.039 / 0.037 | 2.3 / 2.4 |
| idle session, session tab only (frames forced) | 0.482 | 0.434 | -10.1% | 0.038 / 0.038 | 1.6 / 1.5 |

These runs were noisy: the directory row does nothing new and moved 28%, and the earlier set of
three runs (`.superpowers/perf/t12-first/`, before the last two observer changes) gave +15% to
+26% on the flood rows and -3% to +29% on the idle rows. What is real is the logic column on the
flood rows: +0.05 to +0.29 ms a frame (about +0.17 ms on average) for the text room observer;
taking it out (an experiment build, one run, numbers in the ledger) brought flood logic back to
t11's 0.92 ms.
At 1 MB/s that is about 0.5% of a core. The Map panel's own share in the flood rows is 0.03 ms
(empty) to 0.07 ms (80 rooms). See the ledger's ruling on the observer.

## Parity t11 check (Lua scripts, Mudlet import)

2026-10-09. Lua scripts run in their own Lua state bound to a QuickJS engine that runs the host
API; JavaScript scripts are unchanged apart from the bootstrap's hook ids (`mud.remove`,
`mud.after`). Release build, `WANDUR_MICRO_ONLY_SCRIPTS=1 wandur-bench micro`, medians of 31,
11 or 5 runs (`.superpowers/perf/micro-t11-*.md`), two alternating runs against the t10 binary
(`.superpowers/bin/wandur-bench-t10`; this build kept as `.superpowers/bin/wandur-bench-t11`):

| Measurement | t10 ms | t11 ms | KB (Rust heap) |
|---|---:|---:|---:|
| `Engine::load`, an empty JavaScript script | 0.574 to 0.579 | 0.589 to 0.591 | 47 |
| `Engine::load`, Lantern watch | 0.612 to 0.622 | 0.650 to 0.653 | 51 |
| `SessionScripts`: switched on to running | 0.759 to 0.831 | 0.694 to 0.711 | 60 |
| JavaScript dispatch, a trigger and a line listener, per 1,000 lines | 1.689 to 1.724 | 1.721 to 1.735 | 0 |
| Lua load, an empty script (host engine and Lua state) | | 0.717 | 119 |
| Lua load, the bench script (`micro::BENCH_LUA`) | | 0.682 | 125 |
| Lua load with the Mudlet layer, an empty script | | 1.017 | 314 |
| Lua dispatch, the same trigger and listener, per 1,000 lines | | 2.601 | 288 |
| Mudlet import of the fixture profile (parse, convert, check every pattern in QuickJS) | | 0.459 | 340 |

JavaScript rows move by 0 to 6%, inside run-to-run noise (the first run of the session was
0.92 ms for an empty load, cold caches). A Lua script on the bench flood costs 31.8 ms per MB of
server text on the script thread (JavaScript: 21 ms), about 3% of a core at 1 MB/s: every line
crosses from QuickJS into Lua and back, and the match table is copied into Lua. The UI thread
does nothing new for Lua (lines reach the script thread exactly as for JavaScript), so the shell
bench was not rerun for this task (the plan reruns it after t12). Lua's own heap is in the KB
column only in part (the Lua state allocates through mlua's allocator, the QuickJS host through
the C allocator).

## Parity t10 check (script panels, the rail, bars)

2026-10-09. Script panels are drawn every frame in the session rail; their coloured text is laid
out once per widget revision, and an update with the same values moves no revision.

Rail frames (`WANDUR_MICRO_ONLY_PANELS=1 wandur-bench micro --label t10-panels-N`,
`.superpowers/perf/micro-t10-panels-{1,2,3}.md`, run 3 shown, medians of 400 frames): ten
panels from two scripts, the open one with eleven widgets (three gauges, coded label, text,
button, toggle, list, a six-row table, separator, input), drawn headless at 260 x 760 and
tessellated.

| Frame | draw and tessellate ms | parse and apply ms | layouts built in 400 frames |
|---|---:|---:|---:|
| no updates | 0.033 | 0 | 0 |
| every widget of all ten panels re-sent unchanged each frame (130 instructions) | 0.035 | 0.150 | 0 |
| the same with the gauge values changed each frame | 0.040 | 0.149 | 1,200 |

Unchanged values cost the rail nothing (0.033 against 0.035 ms, inside run to run spread: run 1
measured 0.062 ms for no updates on a cold start); the remaining cost of a re-sent instruction is
checking its JSON again (about 1.2 us each), which C# also does for every instruction.

Shell bench, three alternating runs each of t09 (`.superpowers/bin/wandur-bench-t09`) and t10
(`.superpowers/bin/wandur-bench-t10`), `.superpowers/perf/shell-t10-base*.md` and
`shell-t10-run*.md`, means. Sessions without panels pay for an empty panel host and the strip's
script cards; nothing moved past the run to run spread:

| Scenario | t09 ms/frame | t10 ms/frame | change | logic ms t09 / t10 | process CPU % t09 / t10 |
|---|---:|---:|---:|---:|---:|
| directory page, 300 worlds with art (frames forced) | 0.453 | 0.433 | -4.5% | 0.015 / 0.014 | 1.7 / 1.6 |
| flood 1 MB/s, panels empty (harness-like) | 1.381 | 1.406 | +1.8% | 0.917 / 0.930 | 12.6 / 12.9 |
| flood 1 MB/s, 1,000 macro triggers, panels empty | 1.548 | 1.506 | -2.7% | 1.058 / 1.042 | 13.6 / 13.7 |
| flood 1 MB/s, a script trigger, panels empty | 1.616 | 1.616 | +0.0% | 1.136 / 1.134 | 17.8 / 18.1 |
| flood 1 MB/s, panels with data | 1.449 | 1.389 | -4.1% | 0.941 / 0.912 | 12.8 / 12.4 |
| flood 1 MB/s, session tab only | 1.287 | 1.292 | +0.4% | 0.927 / 0.941 | 12.1 / 12.4 |
| 4 sessions x 250 KB/s, panels with data | 1.500 | 1.513 | +0.8% | 0.976 / 0.975 | 18.7 / 18.4 |
| idle session, panels with data (frames forced) | 0.466 | 0.504 | +8.2% | 0.024 / 0.027 | 1.8 / 1.8 |
| idle session, session tab only (frames forced) | 0.342 | 0.336 | -1.8% | 0.026 / 0.024 | 1.1 / 1.1 |

The idle row with panels is 0.04 ms slower on average (two of three t10 runs at 0.53 ms against
0.45 to 0.48 ms); the session panel's own share there is 0.097 to 0.113 ms against 0.094 to
0.100 ms, at most 0.015 ms, and the idle session tab alone, which holds the same code, is
unchanged. The rest is outside the session panel and within what the other panels vary.

## Parity t09 check (JavaScript scripts)

2026-10-09. A session with a running script now collects completed lines from the grid (as
macros do) and hands them, one batch a frame, to the session's script thread, where each line
is dispatched into the script's QuickJS engine.

Script start (`WANDUR_MICRO_ONLY_SCRIPTS=1 wandur-bench micro --label t09-scripts`,
`.superpowers/perf/micro-t09-scripts.md`, medians of 31 or 11 runs):

| Measurement | ms | KB (Rust heap) |
|---|---:|---:|
| `Engine::load`, an empty script (runtime, context, bootstrap) | 0.60 | 44 |
| `Engine::load`, Lantern watch (help example) | 0.61 | 48 |
| `Engine::load`, Lantern watch with a 200-variable seed | 0.81 | 51 |
| `Engine::load`, 232 KB of functions (near the 256 KiB limit) | 6.2 | 497 |
| `SessionScripts`: switched on to running, new script thread included | 0.71 | 57 |

On the script thread a regex trigger and a line listener cost 1.6 ms per 1,000 flood lines,
19.7 ms per MB of server text (2% of a core at 1 MB/s). QuickJS allocates through the C
allocator, so its heap is not in the KB column.

Shell bench, three alternating runs each of the base build (`.superpowers/bin/wandur-bench-t08final`,
built from 82c8fb3 in a removed scratch worktree) and t09 (`.superpowers/bin/wandur-bench-t09`),
`.superpowers/perf/shell-t09-base*.md` and `shell-t09-run*.md`, means:

| Scenario | t08 ms/frame | t09 ms/frame | change | logic ms t08 / t09 | process CPU % t08 / t09 |
|---|---:|---:|---:|---:|---:|
| flood 1 MB/s, panels empty (harness-like) | 1.404 | 1.375 | -2.1% | 0.929 / 0.910 | 12.6 / 12.5 |
| flood 1 MB/s, 1,000 macro triggers, panels empty | 1.504 | 1.538 | +2.2% | 1.031 / 1.045 | 13.4 / 13.4 |
| flood 1 MB/s, a script trigger, panels empty (new) | | 1.570 | | / 1.096 | / 17.8 |
| flood 1 MB/s, panels with data | 1.401 | 1.427 | +1.8% | 0.906 / 0.939 | 12.6 / 12.8 |
| flood 1 MB/s, session tab only | 1.305 | 1.346 | +3.2% | 0.931 / 0.963 | 12.3 / 12.3 |

Sessions without scripts are unchanged. With the bench script running (`micro::BENCH_SCRIPT`: a
`mud.trigger` regex and a `mud.on(Events.Line)` listener over all 14,000 lines a second) a flood
frame takes 1.570 ms: +11.8% over the t08 flood frame and +14.2% over the same build's frame
without the script, inside the 15% target. The UI thread pays for reading lines back from the
grid and copying them into the queue (+0.17 ms logic, +55 KB a frame); the script thread adds
about 5 points of a core. A first run showed every script stopping on a 1 MB/s flood: the
1,024-event bound counted batches until the UI read their replies; it now counts events until
the thread takes them up, as the C# worker counts until its pump dequeues them.

## Parity t08 check (channel rules)

2026-10-09. Every public line is now classified for the Channels panel on the UI thread, as in
C#: the applied server text is cut into lines, stripped of escape sequences into a reused
buffer (eight bytes at a time between control characters) and matched against the session's
rule set in one `RegexSet` pass; only a line that matches pays for captures and its coloured
copy.

Micro bench (`WANDUR_MICRO_ONLY_CHANNELS=1 wandur-bench micro --label t08-channels`,
`.superpowers/perf/micro-t08-channels.md`), the flood with channel chatter (100,000 lines, 4,898
recognized), per MB of server text:

| Measurement | ms per MB | ms a frame at 1 MB/s, 60 fps |
|---|---:|---:|
| strip each line | 0.90 | 0.015 |
| generic rules over the stripped lines | 0.72 | 0.012 |
| `SessionChannels.receive_text`, generic family | 2.06 | 0.034 |
| the same, SMAUG family | 1.99 | 0.033 |
| the same, SMAUG family + 20 world rules | 1.96 | 0.033 |

In the whole app (shell bench, flood 1 MB/s at the default 30 output frames a second, panels
empty), four alternating runs each of the t07 binary (`.superpowers/bin/wandur-bench-t07`,
built from 5243d17) and t08 (`.superpowers/bin/wandur-bench-t08`, before the eight-byte strip):
UI frame 1.266 against 1.414 ms, logic 0.795 against 0.928 ms (`shell-yy-*`). The same t08
build with classification switched off measured 0.807 ms logic against 0.840 ms for t07 in
four more alternating runs (`shell-zz-*`), so classification costs about 0.11 to 0.13 ms a
frame there: under the 0.3 ms target, and more than the micro bench's 0.067 ms at 30 frames a
second (colder caches between the grid feed and the classifier). Whole shell bench, three
alternating runs (`shell-t08-base*`, `shell-t08-run*`), means:

| Scenario | t07 ms/frame | t08 ms/frame | change | logic ms t07 / t08 |
|---|---:|---:|---:|---:|
| flood 1 MB/s, panels empty (harness-like) | 1.253 | 1.384 | +10.5% | 0.791 / 0.925 |
| flood 1 MB/s, 1,000 macro triggers, panels empty | 1.386 | 1.525 | +10.0% | 0.923 / 1.048 |
| flood 1 MB/s, panels with data | 1.300 | 1.412 | +8.6% | 0.808 / 0.923 |
| flood 1 MB/s, session tab only | 1.221 | 1.270 | +4.0% | 0.834 / 0.918 |
| 4 sessions x 250 KB/s, panels with data | 1.386 | 1.508 | +8.9% | 0.852 / 0.979 |
| idle session, panels with data | 0.488 | 0.453 | -7.1% | 0.023 / 0.020 |
| directory page, 300 worlds | 0.417 | 0.434 | +4.0% | 0.014 / 0.015 |

Under the 20% line everywhere; the flood rows are about 10% slower. If that matters later, the
classifier can move to a thread per session as completion did in t05 (the GMCP duplicate check
would then travel with the text in one queue).

## Parity t07 check (session history)

2026-10-09. Every public line and sent command is now recorded into `wandur.db` (history is on
by default, as in C#). The UI thread only copies the text it applied into the recorder's bounded
queue; a thread per connection reads lines, applies the privacy rules and writes batches.

Shell bench, three alternating runs of the same t07 binary with history on and off
(`WANDUR_BENCH_HISTORY=0`), means (`.superpowers/perf/shell-t07-on*`, `shell-t07-off*`). The new
process CPU column is CPU time over wall time of the whole bench process (app threads and the
in-process loopback server), in percent of one core:

| Scenario | UI ms/frame on / off | logic ms on / off | process CPU % on / off | added CPU |
|---|---:|---:|---:|---:|
| flood 1 MB/s, panels empty (harness-like) | 1.245 / 1.276 | 0.768 / 0.808 | 11.8 / 7.9 | +3.9 |
| flood 1 MB/s, 1,000 macro triggers, panels empty | 1.405 / 1.413 | 0.916 / 0.929 | 13.0 / 8.6 | +4.4 |
| flood 1 MB/s, panels with data | 1.247 / 1.334 | 0.769 / 0.833 | 12.0 / 8.2 | +3.8 |
| flood 1 MB/s, session tab only | 1.157 / 1.170 | 0.803 / 0.813 | 11.7 / 7.4 | +4.3 |

UI frame time and logic did not grow (the slightly lower numbers with history on are a busier
CPU clocking higher, not less work). "UI KB/frame" in the shell bench rises by about 140 KB with
history on: that counter is the process-wide allocator, so it includes the recorder thread's
lines and SQLite; the UI thread's own share is the text copy, about 33 KB a frame at 1 MB/s.

`wandur-bench history --mb 10` (the flood text, 30 hand-overs a second at 1 MB/s,
`.superpowers/perf/history-t07.txt`): reading lines and the privacy rules 7.6 to 8.2 ms CPU per
MB (0.8% of a core, including the feeding thread); into `wandur.db` with the index 42.6 to 43.9
ms per MB (4.3% of a core); handing text over 0.3 to 1.3 ms per MB on the caller's thread.
The database grows by about 2.3 MB per MB of server text (rows, two indexes, FTS5).

How it got there (each step measured with the micro bench, 1 MB/s): the C# schema and its
insert trigger cost about 20% of a core, because FTS5 run from a trigger writes its pending terms
out at every row (each trigger statement is a savepoint); indexing new rows from the store in the
same transaction: 9.7%; batches up to 2 s instead of 250 ms (fewer commits, fewer segment
merges): 7.5%; then 5.5% with 24,000-line batches; 64 rows per INSERT statement and one
`INSERT ... SELECT` into the index per batch: 4.3%. An integer session key and `columnsize=0`
cut the file by a third. A reused write connection and no foreign key checks on appends (both
kept), a later WAL checkpoint and SQLite memory statistics off (both dropped) changed nothing
measurable. Dropping the entries' time index (C# has it; prune uses it) would save about 0.3
points; the index stays.

## Parity t06 check (protocols, diagnostics, vitals)

2026-10-09. Every batch of server text now also goes into the session's console ring, and each
GMCP or MSDP message is formatted for Diagnostics, cached and mapped. Shell bench, three
alternating runs of the t05 binary (`.superpowers/bin/wandur-bench-t05`) and t06
(`.superpowers/bin/wandur-bench-t06`, built from c3c1e1a), means (`.superpowers/perf/shell-t06-base*`,
`shell-t06-run*`):

| Scenario | t05 ms/frame | t06 ms/frame | change | logic ms t05 / t06 | KB/frame t05 / t06 |
|---|---:|---:|---:|---:|---:|
| flood 1 MB/s, panels empty (harness-like) | 1.267 | 1.317 | +3.9% | 0.791 / 0.837 | 150 / 227 |
| flood 1 MB/s, 1,000 macro triggers, panels empty | 1.390 | 1.494 | +7.5% | 0.914 / 0.978 | 152 / 229 |
| flood 1 MB/s, panels with data | 1.311 | 1.362 | +3.9% | 0.791 / 0.853 | 165 / 243 |
| flood 1 MB/s, session tab only | 1.166 | 1.236 | +6.0% | 0.787 / 0.841 | 65 / 142 |
| 4 sessions x 250 KB/s, panels with data | 1.415 | 1.502 | +6.2% | 0.837 / 0.901 | 201 / 278 |
| idle session, panels with data | 0.477 | 0.462 | -3.3% | 0.021 / 0.021 | 165 / 165 |
| directory page, 300 worlds | 0.440 | 0.436 | -0.8% | 0.015 / 0.014 | 173 / 173 |

The +77 KB per flood frame and about +0.05 ms of logic are the console: the C# ring keeps a copy
of every received chunk (2,000 chunks or 512 KiB), and so does this one. The idle session tab
only row read +26% in the mean, from one t06 run with a 15.5 ms stall; the other two runs
(0.318, 0.298 ms) match t05 (0.295, 0.328, 0.290 ms). Within the 10% target everywhere.

## Parity t05 check (composer, live view, links)

2026-10-08. Completion learns every public line. Shell bench, three alternating runs each of the
t04 binary built from ec93aff (`.superpowers/bin/wandur-bench-t04`), t05 with completion learning
on (the default) and t05 with it off (`WANDUR_BENCH_SUGGESTIONS=0`), means
(`.superpowers/perf/shell-t05-base*`, `-run*`, `-off*`):

| Scenario | t04 ms/frame | t05 on | change | t05 off | change | logic ms t04 / on / off | KB/frame t04 / on / off |
|---|---:|---:|---:|---:|---:|---:|---:|
| flood 1 MB/s, panels empty (harness-like) | 1.413 | 1.426 | +1.0% | 1.329 | -5.9% | 0.835 / 0.845 / 0.799 | 144 / 150 / 150 |
| flood 1 MB/s, 1,000 macro triggers, panels empty | 1.438 | 1.543 | +7.3% | 1.513 | +5.2% | 0.928 / 0.969 / 0.963 | 146 / 152 / 152 |
| flood 1 MB/s, panels with data | 1.432 | 1.445 | +0.9% | 1.338 | -6.6% | 0.833 / 0.837 / 0.797 | 159 / 165 / 165 |
| flood 1 MB/s, session tab only | 1.323 | 1.303 | -1.5% | 1.124 | -15.1% | 0.858 / 0.848 / 0.761 | 59 / 65 / 65 |

4 sessions x 250 KB/s with panels (one run each): t04 1.364 ms, t05 1.351 ms. The +6 KB per
frame is the composer (Look, Commands, the command box drawn through `TextEdit::show`, Send).

What changed on the way: learning on the UI thread from the grid's line events cost +23% to +60%
(about 0.6 ms of trie inserts and 0.2 ms of reading completed lines back from the grid per frame,
`shell-t05-*` first runs, not kept). Learning now runs on a thread per session
(`completion::Vocabulary`): the UI thread copies the server text it applied into one reused
buffer per frame and hands it over; the thread strips escape sequences and splits lines itself
(as the C# learner does with its own two-line terminal), so the grid's line events stay off unless
triggers need them. The composer reads the trie under a lock held for one line at a time.

## Parity t04 check (login)

2026-10-08. Every batch of server text now also feeds the prompt line (login and password prompt
matching) and runs the password pattern once. Shell bench against the t03 binary built from
4e3398d (`.superpowers/bin/wandur-bench-t03`), alternating runs, mean of three
(`.superpowers/perf/shell-t04b-*`, `shell-t04c-*`):

| Scenario | t03 ms/frame | t04 ms/frame | change | logic ms t03 / t04 |
|---|---:|---:|---:|---:|
| flood 1 MB/s, panels empty | 1.351 | 1.378 | +2.0% | 0.806 / 0.817 |
| flood 1 MB/s, 1,000 triggers | 1.516 | 1.477 | -2.5% | 0.939 / 0.934 |
| flood 1 MB/s, panels with data | 1.399 | 1.393 | -0.4% | 0.801 / 0.803 |
| flood 1 MB/s, session tab only | 1.183 | 1.189 | +0.5% | 0.769 / 0.778 |
| 4 sessions x 250 KB/s | 1.434 | 1.436 | +0.1% | 0.798 / 0.812 |

Allocation per frame unchanged (143, 145, 159, 59, 195 KB). The first version walked every
character of the flood through the prompt line: +11% frame time, +0.12 ms logic per frame
(`shell-t04-run*`). It now jumps to the last line with text by searching for newlines from the
end. The C# harness was not rerun for this task.

## Parity t03 check (macros)

2026-10-08. New shell bench scenario "flood 1 MB/s, 1,000 macro triggers, panels empty": the
harness-like flood on a saved world with 1,000 enabled triggers (more than a world may save,
64; `bench_macros` in `micro_shell.rs`: word pairs with a number that never occur, a third
ignoring case, some anchored, one that does occur so the send path and the rate limit run).
Five alternating runs (`.superpowers/perf/shell-t03-macros-5runs.md`):

| Scenario | UI ms per frame (mean of 5 means) | median of 5 | logic ms | UI KB/frame |
|---|---:|---:|---:|---:|
| flood 1 MB/s, panels empty | 1.450 | 1.456 | 0.830 | 143 |
| the same with 1,000 triggers | 1.534 (+5.8%) | 1.578 (+8.4%) | 0.939 | 145 |

What it took (measured on the way, same bench):
- Matching on the UI thread first cost +55% (2.39 ms): line extraction from the grid, a String
  per line, and two Aho-Corasick passes (NFA, 0.45 µs a line in the micro bench).
- One DFA that folds ASCII case (case-sensitive hits checked exactly), reused line buffers
  (no allocation per line): matching 0.08 µs a line in the micro bench, but still +25% in the
  shell bench, because in-app work runs about 3.5 times slower than the same work in a hot
  micro loop (bursty frames: cold caches and clocks).
- Matching on a worker thread per session (`TriggerWorker`; the UI thread only takes the line
  texts and sends their commands): +6 to 8%. Results arrive with the next frame.

In-process (`WANDUR_MICRO_ONLY_MACROS=1 wandur-bench micro --label t03-macros`), per 1,000 flood
lines: RuleSet.match_line with 1,000 triggers 0.084 ms (automaton 985 KB, built in 2 to 4 ms);
MacroRuntime.on_line 0.089 ms; feed with line events and triggers 0.717 ms against feed alone
0.503 ms; case fold 0.021 ms. No allocation per line.

Whole shell bench `--label t03` (`.superpowers/perf/shell-t03.md`) against t02: flood 1 MB/s,
panels empty 1.22 ms / 143 KB (t02 1.45 / 139); 4 x 250 KB/s 1.36 / 195 (1.48 / 190); idle
session with panels 0.55 / 159 (0.61 / 154); directory page 0.49 / 173 (0.61 / 173). The 4 to
5 KB more per frame is the session footer. The C# harness was not rerun for this task.

## Parity t02 check (menus, toolbar, localization)

2026-10-08. Every frame now builds the menu state (three times: toolbar, menu bar, shortcuts),
draws the toolbar and the menu bar, and looks strings up in the table. Shell bench
(`wandur-bench shell --label t02`, `.superpowers/perf/shell-t02.md`) against t01: flood 1 MB/s,
panels empty 1.45 ms / 139 KB per frame (t01 1.45 / 139); 4 x 250 KB/s 1.48 / 190 (1.63 / 190);
idle session with panels 0.61 / 154 (0.59 / 154); directory page 0.61 / 173 (0.57 / 173). Logic
per frame 0.03 ms (t01 0.004 to 0.01): the menu state and the shortcut checks. Within 7% everywhere;
the C# harness was not rerun for this task.

## Parity t01 check (database, world ids, scenes)

2026-10-08. The app now opens `wandur.db` at start and runs a writer thread. Shell bench
(`wandur-bench shell --label t01`, `.superpowers/perf/shell-t01.md`) against the M4 table below:
flood 1 MB/s, panels empty 1.43 ms / 139 KB per frame (M4 1.85 / 139); 4 x 250 KB/s 1.56 / 190
(M4 1.88 / 190); idle session with panels 0.62 / 154 (M4 0.82 / 154); directory page 0.59 / 173
(M4 0.59 / 173). Allocation identical, time equal or lower (machine variance).

C# harness, same day, the t01 build against the saved M4 binary, medians of 3
(`app-rust-t01.md`, `app-rust-m4-again-t01.md`):

| Scenario | Startup ms t01 / M4 | Working set MB | Footprint MB | CPU % | Alloc MB/s | Chars/s |
|---|---:|---:|---:|---:|---:|---:|
| startup (ticker) | 347 / 330 | 87 / 101 | 235 / 235 | 8.8 / 10.2 | 26.0 / 26.0 | |
| session-idle (ticker) | 360 / 444 | 88 / 87 | 235 / 235 | 10.2 / 10.9 | 28.8 / 29.1 | |
| flood-1m-nochat | 451 / 459 | 93 / 92 | 240 / 240 | 16.7 / 16.9 | 43.5 / 43.6 | 1.00 M / 1.00 M |
| multi-4-nochat | 419 / 371 | 108 / 107 | 257 / 257 | 16.4 / 19.5 | 51.9 / 51.9 | 201 K / 201 K |

Memory, CPU and allocation within 10% of M4 in every scenario; startup moves both ways by more
than that run to run, as in every earlier milestone (175 to 465 ms), with no trend (opening the
database adds a few milliseconds on the UI thread at start, not measured separately).

## Milestone 4

2026-10-08, same machine and toolchain. Every optimization below was measured before and after;
the ones that did not move a number were dropped. Raw files: `.superpowers/perf/` (`shell-*.md`,
`micro-m4*.md`, `steady/`) and `.superpowers/csharp-ref/.superpowers/perf/logs/` (`app-rust-m4*`,
`app-rust-m3-again*`, `app-csharp-m4*`, `probe-*`). Medians:
`.superpowers/scripts/medians.py <label>...`.

### New instrument: the shell bench

`cargo run --release -p wandur-bench -- shell --label NAME` drives the whole app headless (top
bar, dock with every panel, status bar; `eframe::Frame::_new_kittest`) at 1600 by 1000 points and
2 pixels per point, with sessions on the loopback server, 30 frames a second, 300 frames
measured. It reports UI-thread time and allocation per frame, the share of `App::logic` (draining
sessions into the grids), tessellation, and time and allocation per panel (`PanelStats`, on only
in the bench). This is how "what is actually expensive" was answered.

M3 build (`shell-m3-before.md`), 1 MB/s flood, panels empty:

| Part of the frame | ms per frame | KB allocated per frame |
|---|---:|---:|
| Whole UI pass | 3.14 | 1,455 |
| Session panel (the terminal) | 1.64 | 1,319 |
| Workspace, Map, Channels panels together | 0.13 | 50 |
| Tessellation (after the UI pass) | 0.27 | 1,103 |

So the idle side panels were not the main cost (about 4% of the frame, 3% of the allocation); the
terminal's text layout was: every row on screen is new in every frame under a flood, so each
visible run was shaped and laid out again (egui 0.36 shapes with harfrust), about 1.3 MB of
glyph lists and meshes per frame.

### 1. Drawing the grid's text as one reused mesh (kept)

Each character's glyph quad is cut once from a one-character galley and copied to its cell into a
mesh whose buffers are reused (`grid_text.rs`). Plain runs no longer allocate or lay out anything.

| | M3 | M4 |
|---|---:|---:|
| Shell bench, flood 1 MB/s, panels empty: UI pass ms / KB per frame | 3.14 / 1,455 | 1.85 / 139 |
| same, session panel ms / KB per frame | 1.64 / 1,319 | 0.19 to 0.25 / 11 |
| same, tessellation ms / KB per frame | 0.27 / 1,103 | 0.25 / 664 |
| Micro: terminal frame with a 4 KB append (2,000 row scrollback) ms / KB | 0.547 / 1,993 | 0.097 / 252 |
| Micro: terminal frame at the tail, no output ms / KB | 0.068 / 555 | 0.076 / 263 |
| Harness flood-1m-nochat (bursty), CPU % / alloc MB/s, ticker | 18.4 / 66.0 | 17.8 / 43.5 |
| same, no ticker | 10.6 / 32.6 | 9.4 / 18.0 |
| Harness multi-4-nochat, ticker | 18.2 / 74.5 | 16.9 / 51.4 |
| same, no ticker | 10.3 / 38.7 | 8.9 / 22.7 |
| Steady stream, 1 session at 1 MB/s (2 runs, no ticker), CPU % / alloc MB/s | 16.1, 18.2 / 70 | 15.1, 16.3 / 30 |
| Steady stream, 4 sessions at 250 KB/s | 20.9, 22.5 / 78 | 19.2, 19.1 / 40 |

The M3 rows were rerun the same day with the saved M3 binary (`rust-m3-again*`). Allocation under
output halves; UI-thread time per frame falls by 40%; process CPU falls by 1 to 3 points only,
because most of the process CPU under output is not the UI pass (about 2 ms of a 33 ms frame
interval) but presenting frames (wgpu, Metal, the window server's share accounted to the
process). The headless numbers are the UI thread's work; the app numbers include the GPU path.

What is left in a flooding frame: `App::logic`, about 1.0 ms per frame at 1 MB/s with a
160-column window, is the terminal model itself (alacritty's parser and its row reset on every
line feed, seen with `sample`), not drawing. Without output, the whole frame is 0.6 to 0.8 ms.

### 2. Side panels when idle (measured; one kept, one dropped)

| | Before | After | Kept |
|---|---:|---:|---|
| Channels, 500 messages, per frame ms / KB (the row list rebuilt only when the log, tab or width changes) | 0.164 / 30 | 0.049 / 26 | yes (also no per-frame walk of every message) |
| Directory page, card text cached by world and width, per frame ms / KB | 0.230 / 66 | 0.219 / 62 | no: within noise, dropped |
| Workspace panel per frame ms / KB | 0.08 / 29 | unchanged | not attempted: below 5% of a frame |
| Map panel per frame ms / KB | 0.02 to 0.05 / 5 | unchanged | not attempted |

The directory page's "1.5 MB per frame" quoted in Milestone 3 was the whole process divided by
frames (ticker frames included). Measured per part: the UI pass is 173 KB and 0.6 to 0.7 ms per
frame, of which Find a MUD is 66 KB and 0.2 ms; tessellation is 776 KB (egui's output meshes, made
fresh every frame). Caching card layouts cannot reach the tessellation part, and the part it can
reach did not move, so it was dropped. Caching whole panels' shapes was considered and not built:
the side panels together are 0.1 to 0.2 ms per frame.

Galleys kept across frames are now dropped when egui rebuilds its font atlas (`AtlasWatch`); the
Milestone 3 Channels cache could have drawn stale glyphs after an atlas rebuild.

### 3. JPEG decoding (crop-first kept; zune-jpeg and libjpeg-turbo not adopted)

Same 4000 by 3000 JPEG (1.4 MB, baseline), median of 7, counting allocator, scratch crate built
offline (`.superpowers` scratch, not in the repo):

| Path | 800x320 cover: ms, peak MB, MB allocated | 80x60: ms, peak MB |
|---|---|---|
| M3: jpeg-decoder scaled IDCT (1/4 or 1/8), resize, crop | 27.8 (30.7 in the app's micro), 16.0, 90 | 22.1, 1.7 |
| **M4: same decode, crop first, resize only the shown part, RGB until the end** | **25.3 (27.8 to 29.3 in the micro), 7.8, 82** | 22.1 (23.1), 1.1 |
| zune-jpeg 0.5.15 full decode, box reduce, same final resize | 34.6, 36.4, 37 | 28.8, 34.5 |
| zune-jpeg into a reused buffer | 35.6, 3.0 plus a 36 MB buffer per worker kept for good | 28.0 |
| zune-jpeg through `image`, resize from full size | 51.9, 84.4, 85 | 46.1, 49.6 |
| libjpeg-turbo 3.1.4 (`djpeg -scale`, Homebrew, minus process start) | 1/4: about 17.1; 1/8: 15.8; full: 26.4 | 1/8: 15.8 |
| C# (Skia, libjpeg-turbo) | 15.8, peak RSS +15 | 9.3, +8 |

- zune-jpeg (MIT, Apache-2.0 or Zlib; NEON and x86 SIMD) decodes a full image twice as fast as
  jpeg-decoder (26.8 against 54.0 ms) but has no scaled decoding and no row output, so the full
  36 to 48 MB image always exists and the result is slower than the scaled path at every target
  size. Not adopted, and no split by target size either.
- A libjpeg-turbo binding (`turbojpeg` crate, MIT or Unlicense; the library BSD-style, IJG, zlib)
  would save about 8 to 10 ms per plate but needs cmake plus nasm to build from source (nasm is
  not installed here, no network was used), or a system library per platform (Homebrew on macOS,
  vcpkg or a bundled DLL on Windows, `libturbojpeg` on Linux). Not built. The brief's bar was
  "pure Rust within about 1.5 times C#'s 16 ms" (about 24 ms): crop-first pure Rust is at 25 to
  29 ms, just over it, and four workers hide most of it in the app (the 300-card paging run takes
  5.1 s either way).
- In the app: the 300-card paging run's peak live heap went from 52 MB to 39 to 50 MB (two runs).
- jpeg-decoder without its `rayon` feature starts its own threads per decode (one per component);
  one decode uses more than one core. Left as is: it does not change throughput with four workers.

### 4. Terminal memory (measured, no change)

`cargo run --release -p wandur-bench --example memprobe`:

| | Value |
|---|---:|
| Empty terminal, live heap | 2,333 KB (2,048 KB of it vte's synchronized-update buffer) |
| 16 empty terminals: live heap / resident | +37,374 KB / +288 KB (18 KB each) |
| Retained, history full, 120 columns: 500 / 1,000 / 2,000 / 5,000 / 10,000 rows | 3.8 / 5.2 / 8.0 / 16.7 / 31.0 MB |
| 2,000 rows full, then 20,000 more lines with combining marks and true colour | 8,166 KB before and after |
| Scrollback lowered from 2,000 to 500 rows | 8,166 to 3,842 KB |

- vte's 2 MB buffer is `Vec::with_capacity(0x20_0000)` inside `Processor::new`; there is no
  configuration to shrink it and alacritty drives the grid only through that processor. It is
  never written unless a server uses synchronized updates, so its pages are never resident (18
  KB resident per empty terminal). It counts in the live heap figures, not in memory used.
- alacritty releases old rows and their formatting at the bound: rows are rotated and reset, and
  extra cell data (combining marks) goes with them; retained memory is flat after the bound.
  Lowering the scrollback releases rows at once.
- Default scrollback stays at 2,000 rows: the C# display keeps 2,000 rows, about 6 MB resident per
  session at 120 columns (24-byte cells), the same order as C#'s 6.9 MB per session.

### 5. Background work (one move off the UI thread; a stall probe)

- Moved: trimming the thumbnail disk cache at start (listing up to 64 MB of thumbnails, 2,600
  files: 3.5 to 13.8 ms, run to run, on this exFAT volume) now runs on the first artwork worker.
- Already off the UI thread (audited): settings and layout writes, directory fetch, parse, index
  and cache write, artwork fetch and decode and the thumbnail cache, font indexing and search.
  See `docs/architecture.md`, "What runs on the UI thread".
- The probe now writes a histogram of UI-thread time per frame (logic plus UI, not presenting):
  `frameHist` (buckets up to 1, 2, 4, 8, 16, 33, 50 ms and above), `frameMsMax`,
  `framesOver16ms`, `framesOver50ms`. In the M4 harness runs no frame took more than 16.7 ms
  in any scenario (one frame at 16.7 ms in multi-4 with the ticker); the maximum shown there,
  14 to 16 ms, was the probe's own file write in the first measured frame, now excluded (after:
  1.05 ms maximum at 1 MB/s steady). The test `no_long_ui_thread_stalls_under_a_flood` drives
  the whole app under a flood and fails on a frame over 250 ms (unoptimized test build).

### 6. Lazy optional features (confirmed)

Nothing optional starts at launch (startup live heap 3.8 MB, unchanged since M2). The plug-in
points for scripting and room classification are documented in `docs/architecture.md`.

### Final comparison, Rust M4 against C#, same day

Both clients driven by the C# harness (`.superpowers/m4-harness.sh`), medians of 3 runs (startup
6), the C# client built from the exported reference at 27f66d2, the Rust binary from 3f337b4 (the later commits change only the probe's frame histogram and documentation). Steady-stream rows from
`.superpowers/scripts/steady.sh` (the probe directly, no ticker, 2 runs). Rust CPU and
allocation are given with the probe's 50 ms ticker (comparable with the C# runs, which have it
too) and without it (what the app does on its own; C# has no such switch).

| Scenario | Startup ms Rust / C# | Working set MB | Footprint MB | Heap MB (Rust live / C# GC) | CPU % Rust (no ticker) / C# | Alloc MB/s Rust (no ticker) / C# | UI p95 ms Rust / C# |
|---|---:|---:|---:|---:|---:|---:|---:|
| startup (idle) | 282 / 1,126 | 85 / 206 | 234 / 225 | 3.8 / 26.9 | 9.9 (0.3) / 2.8 | 26.0 (0.3) / 0.0 | 0.6 / 0.1 |
| session-idle | 253 / 1,056 | 86 / 174 | 235 / 266 | 6.2 / 43.8 | 10.9 (1.8) / 7.5 | 29.3 (2.8) / 0.1 | 0.6 / 0.2 |
| flood-100k | 373 / 1,045 | 91 / 260 | 240 / 496 | 10.4 / 82.1 | 16.4 / 37.3 | 43.5 / 23.4 | 1.2 / 3.9 |
| flood-1m-nochat | 373 / 1,041 | 91 / 201 | 241 / 472 | 10.7 / 56.0 | 17.8 (9.4) / 30.7 | 43.5 (18.0) / 73.7 | 1.2 / 12.1 |
| multi-4-nochat | 321 / 1,113 | 107 / 260 | 257 / 519 | 31.1 / 114.4 | 16.9 (8.9) / 37.1 | 51.4 (22.7) / 20.6 | 1.5 / 0.8 |
| multi-8-idle | 326 / 1,099 | 87 / 262 | 238 / 409 | 21.8 / 65.5 | 12.7 (1.8) / 8.9 | 41.4 (3.8) / 0.6 | 1.2 / 0.2 |
| directory (300 worlds, 4000x3000 art) | 264 / 1,064 | 87 / 249 | 276 / 295 | 5.5 / 37.2 | 11.6 / 21.9 | 32.1 / 0.0 | 0.7 / 0.1 |
| steady 1 session at 1 MB/s | n/a | 91 / 225 | n/a | 10.5 / 97 | (15.7) / 35.3 | (30) / 84 | n/a |
| steady 4 sessions at 250 KB/s | n/a | 108 / 318 | n/a | 31.1 / 167 | (19.1) / 70.4 | (40) / 127 | n/a |

Every flood kept up in both clients (1.0 M chars/s; 200 K chars/s for four sessions). Directory:
at most 4 cards alive in Rust, 3 in C#.

In-process (Rust `micro --label m4`, C# `docs/perf.md` micro):

| Brief's micro benchmark | Rust M4 | C# |
|---|---:|---:|
| ANSI parsing throughput (100,000 lines, 8.6 M chars, 4 KB chunks, into a 2,000 row grid) | 47.3 ms, 173 MB/s, 8.1 MB allocated | 54.9 ms, 42.0 MB allocated |
| Appending 100,000 lines with line events (each line read back) | 67.2 ms, 25.1 MB | n/a |
| One 4 KB chunk into a full scrollback | 0.023 ms, 0 KB | 0.023 ms, 19.2 KB |
| Rendering 2,000 lines of scrollback: frame at the tail / with a 4 KB append (headless, incl. tessellation) | 0.076 ms, 263 KB / 0.097 ms, 252 KB | n/a (no headless equivalent) |
| Multiple sessions: 4 at 100 KB/s, network and grid only | 1.8% CPU incl. server, 0 MB/s | 32% CPU, 32 MB/s (with a headless window) |
| Directory filter / sort, 500 worlds | 0.023 / 0.017 ms | 2.256 / 0.573 ms |
| Large artwork: 4000x3000 JPEG / PNG to 800x320 | 27.8 ms, +7.8 MB / 77.6 ms, +2.8 MB | 15.8 ms, +15 MB / 89.4 ms, +68 MB |
| Many large artwork files: 300 cards paged 5 at a time, decoded to 800x320 | 5.1 s, textures within 24 MB, peak heap +39 to 50 MB | n/a |

Method notes:

- The harness server is bursty (ten writes a second), so the steady rows are the realistic flood.
- The ticker forces up to 20 frames a second; Rust idle CPU and allocation are only meaningful
  without it. C# idle allocation is near zero because its UI does not redraw when nothing
  changes; Rust redraws only on output, input, timers or the ticker.
- Footprint is read once at the end by the harness; the Rust figure includes about 160 MB of
  graphics memory that exists whenever frames are drawn (see Milestone 1).
- UI p95 is not the same quantity (Rust: wait for the next display-paced frame; C#: dispatcher
  wait), as in every milestone.
- The C# micro numbers are from its own repository's `docs/perf.md`, not rerun.

### What the numbers say (Milestone 4)

- **Rendering under output: much better than M3** in UI-thread time (40% less) and allocation
  (about half), slightly better in process CPU (1 to 3 points). The remaining CPU is presenting
  frames and the terminal model, not drawing.
- **Against C#, same day: better** in startup (4 times faster), working set (less than half),
  heap (5 to 8 times smaller), footprint under output (half), CPU under sustained output (about
  half with one session, a quarter with four, steady stream), directory memory and queries;
  **worse** in idle CPU with the probe's ticker (it forces frames C# does not draw; without it
  1.8% against C#'s 7.5 to 8.9%), allocation rate when idle with the ticker, and JPEG decode time
  (1.8 times). Footprint when idle with nothing drawn is about equal (graphics memory).
 Same machine throughout: Apple M3 Pro (11 cores),
18 GB, macOS 26.5, a 120 Hz (ProMotion) display. Rust 1.99.0, release profile, eframe 0.36.2
with the wgpu renderer (Metal).

## Milestone 3

2026-10-08. The full workspace: Find a MUD is the first page, Map and Channels panels, the status
bar. Same machine and method as Milestone 2 (the C# harness drives the Rust binary with its own
loopback servers; `WANDUR_DIRECTORY_URL` points at its loopback directory or a closed port). Raw
files: `.superpowers/csharp-ref/.superpowers/perf/logs/` (`app-rust-m3*`, `app-csharp-m3-dir*`,
`probe-rust-m3*`) and `.superpowers/perf/micro-m3.md`.

```sh
cd .superpowers/csharp-ref
B=bench/Wandur.Bench/bin/Release/net10.0/Wandur.Bench.dll
APP=<path to wandur-client-rust>/target/release/wandur
dotnet $B app directory --runs 3 --label rust-m3-dir24 --app "$APP"
dotnet $B app directory --runs 3 --label csharp-m3-dir
WANDUR_PERF_NO_TICK=1 dotnet $B app directory --runs 3 --label rust-m3-dir24-notick --app "$APP"
dotnet $B app startup --runs 6 --label rust-m3-startup --app "$APP"
dotnet $B app session-idle flood-1m-nochat multi-4-nochat multi-8-idle --runs 3 --label rust-m3 --app "$APP"
WANDUR_PERF_NO_TICK=1 dotnet $B app startup session-idle multi-8-idle --runs 3 --label rust-m3-notick --app "$APP"
cd ../.. && cargo run --release -p wandur-bench -- micro --label m3
```

### Directory scenario (300 listings, 4000x3000 artwork, full scroll and back)

The probe shows Find a MUD, waits for the catalog, settles 6 s, pages to the end every 30 ms and
back, then measures 5 s. Medians of 3 runs; the C# client was rerun the same day.

| | Startup ms | Working set MB | Footprint MB | Heap MB (C#: GC heap) | CPU % | Alloc MB/s | Cards alive at most | Art requests (run 1) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Rust M3, 24 MB texture budget (final) | 319 | 112 | 264 | 5.4 | 12.3 | 34 | 5 | 296 |
| Rust M3, same, no ticker | 189 | 110 | 274 | 5.0 | 0.6 | 0.5 | 5 | 300 |
| Rust M3, first build, 96 MB texture budget | 265 | 90 | 366 | 5.5 | 12.8 | 34.6 | 5 | 300 |
| C# same day | 1525 | 199 | 301 | 37.3 | 12.2 | 0.0 | 5 | 286 |
| C# cited (`docs/perf.md`, after its optimization) | 1197 | 212 (255 in its table) | 297 | 37.1 | 19.3 | 0.0 | 5 | n/a |

- The first build kept up to 96 MB of textures. Textures live in GPU memory, which macOS counts in
  the footprint, so it measured 358 to 381 MB, more than C#. With a 24 MB budget (about 24 plates
  of 800 by 320 pixels; five are on screen) the footprint is 263 to 290 MB, below C#'s 292 to 317.
  Measured, then changed: the budget is now 24 MB.
- Runs 2 and 3 found most thumbnails in the disk cache (60 to 77 requests instead of 300); C#
  likewise keeps downloaded art on disk (13 requests in its run 2).
- Allocation with the ticker on (34 MB/s against C#'s 0): egui lays out the visible cards again
  on every frame the ticker forces. Without the ticker, nothing is redrawn and it is 0.5 MB/s.

### Session scenarios with the full shell, next to Milestone 2

Medians of 3 (startup 6). "M2" is the Milestone 2 final row for the same scenario.

| Scenario | Startup ms M3 / M2 | Working set MB M3 / M2 | Footprint MB M3 / M2 | Heap MB M3 / M2 | CPU % M3 / M2 | Alloc MB/s M3 / M2 | C# CPU % |
|---|---:|---:|---:|---:|---:|---:|---:|
| startup (ticker) | 256 / 268 | 100 / 99 | 235 / 231 | 3.8 / 2.5 | 10.8 / 10.6 | 27.4 / 9.9 | 2.7 |
| startup (no ticker) | 238 / 202 | 96 / 83 | 234 / 229 | 3.8 / 2.5 | 0.2 / 0.2 | 0.3 / 0.1 | 2.7 |
| session-idle (ticker) | 258 / 247 | 94 / 86 | 236 / 231 | 6.2 / 5.1 | 11.4 / 10.5 | 31.5 / 13.2 | 6.5 |
| session-idle (no ticker) | 348 / 248 | 89 / 86 | 235 / 231 | 6.2 / 5.1 | 1.6 / 1.6 | 3.0 / 1.2 | 6.5 |
| flood-1m-nochat | 294 / 308 | 93 / 94 | 242 / 241 | 10.7 / 11.2 | 19.0 / 17.7 | 68.1 / 48.8 | 29.6 |
| multi-4-nochat | 363 / 274 | 110 / 109 | 259 / 258 | 31.3 / 31.7 | 18.8 / 15.9 | 77.2 / 56.3 | 36.3 |
| multi-8-idle (ticker) | 350 / 333 | 103 / 87 | 238 / 234 | 21.8 / 20.8 | 13.1 / 11.2 | 44.4 / 24.7 | 8.3 |
| multi-8-idle (no ticker) | 430 / 291 | 88 / 86 | 238 / 233 | 21.8 / 20.8 | 1.8 / 2.0 | 4.0 / 2.2 | 8.3 |

Every flood kept up (1.00 M chars/s; 200 K chars/s for four sessions). Startup varies between
175 and 465 ms run to run in both milestones; no trend. Idle CPU without the ticker is unchanged
(1.6 to 1.8%). Under output and with the ticker, CPU is one to three points higher and allocation
20 to 40% higher than M2: the Workspace, Map and Channels panels are drawn on every frame (egui
redraws everything visible), not only the terminal. Still about two thirds of C#'s CPU under
flood. The steady-stream runs of Milestone 2 were not repeated.

### In-process (`micro --label m3`)

| Measurement | Rust M3 | C# |
|---|---:|---:|
| Directory filter, 500 worlds, search text changes | 0.023 ms, 24 KB (0.043 ms with varied listings) | 2.256 ms, 772 KB |
| Directory sort, 500 worlds, cycling 6 sorts | 0.017 ms, 31 KB (0.024 ms varied) | 0.573 ms, 75 KB |
| Directory parse and index, 500 worlds (610 KB JSON), worker thread | 3.4 ms | n/a |
| Thumbnail 4000x3000 JPEG to 800x320 cover | 30.7 ms, peak live heap +13.8 MB | 15.8 ms, peak RSS +15 MB |
| Thumbnail 4000x3000 PNG to 800x320 cover | 78.5 ms, peak live heap +2.8 MB | 89.4 ms, peak RSS +68 MB |
| Thumbnail 4000x3000 JPEG to 80x60 | 23.6 ms, peak +1.3 MB | 9.3 ms, peak RSS +8 MB |
| Same JPEG decoded at full size then resized (not used) | 75.6 ms, peak +84 MB | n/a |
| 300 cards paged 5 at a time over loopback HTTP, 4 workers | 5.1 s; textures never above the 24 MB budget (276 evicted); peak heap +52 MB | n/a |

The JPEG and PNG sources differ from C#'s (same size and kind of content, generated by our own
bench: 1.4 MB JPEG, 1.6 MB PNG against 0.9 and 2.3 MB). The peak columns are not the same
quantity: Rust samples its own live heap, C# reads the process RSS rise.

- **Filter and sort: about 100 times faster than C#**, because every world's searchable text is
  normalized once when the catalog is built, not per query.
- **PNG: about the same time, a 25th of the memory**: rows are averaged into the reduced image as
  they are decoded, so the 48 MB full bitmap never exists (C# decodes PNG whole).
- **JPEG: about twice as slow as C#** (Skia uses libjpeg-turbo with SIMD; `jpeg-decoder` is plain
  Rust), with similar peak memory. `jpeg-decoder` also allocates 70 to 90 MB per decode in short
  lived buffers. Scaled decoding is still 2.5 times faster than decoding at full size (zune-jpeg
  through `image`, 75 ms, peak +84 MB). A libjpeg-turbo binding would close the gap; not tried.

### What the numbers say (Milestone 3)

- **Directory memory: better than C#** after one measured change: footprint 264 against 301 MB,
  working set 112 against 199 MB, heap 5 against 37 MB. With the first texture budget it was worse
  (366 MB); GPU textures count in the footprint, so the budget matters more than the heap.
- **Directory CPU: equal** with the ticker (12.3 against 12.2%), near zero without it.
- **Directory queries: much better** (0.02 against 0.6 to 2.3 ms).
- **Artwork decoding: mixed**: PNG better on memory, JPEG worse on time.
- **Session scenarios: Milestone 2's numbers hold** within a few points of CPU; drawing the extra
  panels every frame costs some allocation.
- **Startup: unchanged**, 0.2 to 0.4 s against C#'s 1.2 to 1.5 s.

## Milestone 2

2026-10-08. The terminal model is now an `alacritty_terminal` grid; output frames are capped at 30
a second; the caret does not blink. Raw files: `.superpowers/csharp-ref/.superpowers/perf/logs/`
(`app-rust-m2*.json`, `app-rust-m2*.md`, `probe-rust-m2*.jsonl`) and `.superpowers/perf/micro-m2.md`.

### Method

As in Milestone 1: the C# harness drives the Rust binary with its own loopback server, and the
Rust probe writes the C# probe's JSON lines. Commands:

```sh
cd .superpowers/csharp-ref
B=bench/Wandur.Bench/bin/Release/net10.0/Wandur.Bench.dll
APP=<path to wandur-client-rust>/target/release/wandur
dotnet $B app session-idle flood-100k flood-100k-nochat flood-1m flood-1m-nochat multi-4 multi-4-nochat multi-8-idle --runs 3 --label rust-m2 --app "$APP"
dotnet $B app session-idle flood-1m-nochat multi-4-nochat multi-8-idle --runs 3 --label rust-m2-final --app "$APP"
dotnet $B app startup --runs 6 --label rust-m2-startup --app "$APP"
WANDUR_PERF_NO_TICK=1 dotnet $B app startup session-idle flood-1m-nochat multi-4-nochat multi-8-idle --runs 3 --label rust-m2-notick --app "$APP"
WANDUR_PERF_NO_TICK=1 dotnet $B app session-idle multi-8-idle --runs 3 --label rust-m2-noblink --app "$APP"
```

Two things learned about the method this milestone:

- **The harness server is bursty.** It writes ten times a second, so a client never has output
  for more than about 26 frames a second, and a redraw cap cannot show its effect. A steady
  stream (`wandur-bench mud-server --write-ms 2`, the same byte rate in 500 writes a second, like a
  busy real MUD) does. Those runs used the probe directly (`WANDUR_PERF_SCENARIO=sessions`, no
  ticker, 3 s settle, 8 s measured); the C# client was run the same way with its own probe.
- **The probe's 50 ms ticker drives frames.** With it on, M1 and M2 both draw about 60 frames a
  second under output whatever the cap, so the ticker rows show CPU of a frame rate the ticker
  chose. The no-ticker rows show what the app itself does.

### Steady stream (the most realistic flood): M1, M2 and C#

Medians of 2 runs, no ticker. Throughput was 1.00 M chars/s in every run.

| Scenario | Rust M2 CPU % | M1 CPU % | C# CPU % | M2 alloc MB/s | M1 alloc MB/s | C# alloc MB/s | M2 frames/s | M1 frames/s | Working set MB M2 / M1 / C# | Heap MB M2 / M1 / C# GC |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 1 session at 1 MB/s | **18.8** | 42.4 | 37.6 | 60 | 252 | 84 | 34 | 119 | 94 / 88 / 221 | 11.1 / 3.6 / 140 |
| 4 sessions at 250 KB/s | **19.7** | 40.6 | 76.0 | 65 | 228 | 127 | 38 | 120 | 111 / 90 / 312 | 31.7 / 4.7 / 212 |

The cap on its own (same M2 build, 1 MB/s steady): 30 a second gives 18 to 25% CPU and 67 to
100 MB/s allocated; no cap gives 120 frames a second, 38 to 46% CPU and 206 MB/s. M1's
`request_repaint()` also cost two frames per wake (egui answers a zero-delay request with two).

### Harness scenarios (bursty server)

Medians of 3 runs (startup 6). M2 floods from the `rust-m2` run; the `final` rows rerun four
scenarios with the final build (steady caret).

| Scenario | Startup ms M2 / M1 / C# | Working set MB M2 / M1 / C# | Footprint MB M2 / M1 / C# | Heap MB M2 / M1 / C# GC | CPU % M2 / M1 / C# | Alloc MB/s M2 / M1 / C# | UI p95 ms M2 / M1 / C# |
|---|---:|---:|---:|---:|---:|---:|---:|
| startup (ticker) | 268 / 234 / 1149 | 99 / 83 / 202 | 231 / 229 / 226 | 2.5 / 2.4 / 26.9 | 10.6 / 9.3 / 2.7 | 9.9 / 7.5 / 0.0 | 0.4 / 0.4 / 0.1 |
| startup (no ticker) | 202 / 185 / 1149 | 83 / 85 / 202 | 229 / 225 / 226 | 2.5 / 2.4 / 26.9 | 0.2 / 0.2 / 2.7 | 0.1 / 0.1 / 0.0 | n/a |
| session-idle (final, ticker) | 247 / 218 / 1293 | 86 / 85 / 174 | 231 / 231 / 265 | 5.1 / 2.6 / 43.7 | 10.5 / 11.0 / 6.5 | 13.2 / 13.4 / 0.1 | 0.5 / 4.5 / 0.1 |
| session-idle (steady caret, no ticker) | 248 / 326 / 1293 | 86 / 84 / 174 | 231 / 230 / 265 | 5.1 / 2.6 / 43.7 | **1.6** / 2.1 / 6.5 | 1.2 / 2.9 / 0.1 | n/a |
| flood-100k | 234 / 281 / 1213 | 106 / 89 / 270 | 240 / 235 / 498 | 11.0 / 3.7 / 81.8 | 19.1 / 20.8 / 35.7 | 54.7 / 75.3 / 23.1 | 2.4 / 6.7 / 3.9 |
| flood-100k-nochat | 260 / 275 / 1197 | 109 / 88 / 266 | 242 / 235 / 484 | 10.9 / 3.7 / 60.1 | 17.9 / 20.4 / 24.9 | 53.9 / 74.8 / 11.1 | 3.1 / 5.5 / 0.1 |
| flood-1m | 210 / 278 / n/a | 105 / 89 / n/a | 241 / 235 / n/a | 11.2 / 3.9 / n/a | 19.6 / 22.5 / n/a | 52.8 / 79.6 / n/a | 3.3 / 6.0 / n/a |
| flood-1m-nochat (final) | 308 / 333 / 1234 | 94 / 88 / 245 | 241 / 234 / 472 | 11.2 / 3.8 / 56.0 | 17.7 / 21.1 / 29.6 | 48.8 / 77.9 / 69.8 | 1.2 / 5.6 / 12.8 |
| flood-1m-nochat (no ticker) | 252 / n/a / n/a | 94 | 240 | 11.2 | 11.8 | 30.7 | n/a |
| multi-4 | 203 / 307 / 1158 | 112 / 90 / 262 | 258 / 236 / 533 | 31.6 / 5.0 / 134.0 | 17.4 / 22.4 / 42.0 | 58.7 / 82.0 / 22.6 | 1.4 / 5.2 / 0.6 |
| multi-4-nochat (final) | 274 / 309 / 1225 | 109 / 90 / 277 | 258 / 236 / 520 | 31.7 / 4.9 / 114.4 | 15.9 / 22.3 / 36.3 | 56.3 / 83.2 / 20.6 | 1.0 / 5.8 / 0.2 |
| multi-4-nochat (no ticker) | 247 / n/a / n/a | 111 | 257 | 31.7 | 10.7 | 35.4 | n/a |
| multi-8-idle (final, ticker) | 333 / 290 / 1213 | 87 / 84 / 189 | 234 / 232 / 406 | 20.8 / 3.0 / 65.5 | 11.2 / 12.4 / 8.3 | 24.7 / 28.4 / 0.6 | 0.5 / 2.0 / 0.1 |
| multi-8-idle (steady caret, no ticker) | 291 / 296 / 1213 | 86 / 85 / 189 | 233 / 231 / 406 | 20.8 / 3.0 / 65.5 | **2.0** / 2.4 / 8.3 | 2.2 / 5.7 / 0.6 | n/a |

Every scenario kept up with the server. UI p95 is lower than M1 because fewer frames compete
with the ticker; it is still a display-paced number, not comparable with the C# dispatcher wait.

### Footprint, sampled 40 times over 10 s (`scripts/footprint-sample.sh`, absolute binary path)

| Situation | M2 min / median / max MB | M2 peak | M1 min / median / max MB | C# median MB |
|---|---:|---:|---:|---:|
| No session, nothing redrawing | 69 / 70 / 70 | 252 | 65 / 66 / 66 | 224 |
| One idle session (prompt every 2 s) | 70 / 226 / 227 | 227 | 71 / 230 / 230 | 363 |
| One session at 1 MB/s (bursty) | 240 / 240 / 240 | 263 | 230 / 231 / 231 | 514 |

The roughly 160 MB of graphics memory that appears whenever frames are drawn is unchanged from
M1 (wgpu and Metal surfaces); it dominates the footprint, not the grid.

### In-process (`cargo run --release -p wandur-bench -- micro --label m2`)

| Measurement | M2 | M1 | C# |
|---|---:|---:|---:|
| 100,000 lines (8.6 M chars, 4 KB chunks) into a 2,000 row scrollback | 47.9 ms, 8.1 MB allocated (171 MB/s) | 36.6 ms, 18.1 MB | 54.9 ms, 42.0 MB |
| Same with line events on (each line read back from the grid) | 67.5 ms, 25.1 MB | n/a | n/a |
| One 4 KB chunk into a full scrollback | 0.023 ms, 0 KB | 0.018 ms, 8.9 KB | 0.023 ms, 19.2 KB |
| Plain text of the full scrollback | 0.57 ms | 0.012 ms | 0.321 ms |
| Reflow of 2,000 rows on a width change | 0.15 ms | n/a (no reflow) | n/a |
| Retained per terminal, 2,000 history rows + 50 screen rows, 120 columns | 8,023 KB (of which 2,048 KB untouched vte reservation) | 328 KB | 914 KB line model + 6,026 KB display |
| Same at 80 columns | 6,054 KB | 328 KB | n/a |
| Terminal frame at the tail, no new output (headless layout and tessellation) | 0.067 ms, 555 KB | 0.057 ms, 976 KB | n/a |
| Terminal frame with a 4 KB append each frame | 0.536 ms, 1,993 KB | 0.386 ms, 2,045 KB | n/a |
| 1 session at 200 KB/s, core and grid only | 0.7% CPU incl. server | 0.5% | n/a |
| 4 sessions at 100 KB/s each, core and grid only | 1.4% CPU incl. server | 1.1% | n/a |

Memory per terminal (`cargo run --release -p wandur-bench --example memprobe`): 2.3 MB when
empty (2 MB of it vte's untouched synchronized-update buffer), 5.2 MB after 100 lines, 8.0 MB once
the 2,000 rows of history are full; it does not grow after that. alacritty allocates history
1,000 rows at a time.

### Fonts

| | Value |
|---|---|
| Bundled faces (in the binary) | JetBrains Mono Regular 274 KB and Bold 278 KB |
| Index system fonts (fontdb, once, worker thread) | 221 to 356 ms |
| Search character maps for 20 to 50 missing characters | 63 to 108 ms |
| Faces added for the Unicode page | Arial Unicode MS (22.2 MB file, memory mapped) and Apple Symbols (0.9 MB) |
| Frame after adding the faces (atlas rebuild, UI thread) | 11 to 21 ms, once |
| Live heap cost | the fontdb index on the worker (a few hundred KB); the font files are mapped, not read |

The index and search run off the UI thread; the only pause on the UI thread is the frame that
rebuilds the font atlas.

### What the numbers say (Milestone 2)

- **CPU under sustained output: better than M1 and much better than C#.** Under a steady stream
  M2 uses 19 to 20% against M1's 41 to 42% and C#'s 38% (one session) and 76% (four sessions).
  Under the harness's bursty server it is 16 to 20% against 21 to 22% and 25 to 42%.
- **Allocation rate: better than M1, now at or below C#.** 49 to 65 MB/s under flood against 75
  to 252 MB/s (M1) and 84 to 127 MB/s (C#, steady).
- **Idle CPU: better.** 1.6% with a session and 2.0% with eight, against 2.1 to 2.4% (M1) and
  6.5 to 8.3% (C#).
- **Memory: worse than M1, still better than C#.** Each session's grid holds about 6 MB at 120
  columns and 2,000 rows (plus 2 MB vte reserves but does not touch), against 328 KB for the M1
  line buffer. Four sessions: heap 32 MB against M1's 5 MB and C#'s 114 to 212 MB GC heap;
  working set 107 to 112 MB against 90 MB and 262 to 313 MB.
- **Throughput: equal.** Every scenario kept up.
- **Startup: unchanged.** 200 to 330 ms to the first frame.
- **Transcript export is slower** (0.57 ms against 0.012 ms) because it walks grid cells; it is only
  built on request.

## Milestone 1 (record)

Milestone 1, 2026-10-08. Same machine as the C# baseline: Apple M3 Pro (11 cores), 18 GB, macOS
26.5. Rust 1.99.0, release profile (`opt-level 3`, line tables only), eframe 0.36.2 with the wgpu
renderer (Metal) unless noted. The C# figures come from `docs/baseline.md`, plus the same-day
C# re-run described below.

### Method

The app-level scenarios use the C# harness **unchanged**, so both apps meet the same loopback MUD
server, the same scenarios, the same footprint reading and the same summarizer. The Rust probe
(`crates/wandur-app/src/probe.rs`, on only when `WANDUR_PERF_PROBE` is set) writes the same JSON
lines as `PerfProbe.cs`. The harness runs from an exported copy of the C# repository under
`.superpowers/csharp-ref/` (gitignored); the owner's checkout was not built or run.

```sh
# once: export and build the C# reference (see .superpowers/progress.md for the exact steps)
cd .superpowers/csharp-ref
B=bench/Wandur.Bench/bin/Release/net10.0/Wandur.Bench.dll
APP=<path to wandur-client-rust>/target/release/wandur
dotnet $B app session-idle flood-100k flood-100k-nochat flood-1m flood-1m-nochat multi-4 multi-4-nochat multi-8-idle --runs 3 --label rust-m1 --app "$APP"
dotnet $B app startup --runs 6 --label rust-m1-startup --app "$APP"
WANDUR_PERF_NO_TICK=1 dotnet $B app startup session-idle multi-8-idle --runs 3 --label rust-m1-notick --app "$APP"
dotnet $B app startup session-idle flood-100k-nochat flood-1m-nochat multi-4-nochat --runs 3 --label csharp-sameday
python3 bench/summarize.py .superpowers/perf/logs/app-rust-m1.json
```

Mapped columns for Rust: **GC heap** is the live heap from a counting global allocator (exact,
no collector); **Alloc MB/s** comes from the same allocator; GC counts are 0.

**UI latency is not the same quantity in both apps.** The C# probe measures how long a job posted
to the Avalonia dispatcher waits. The Rust probe posts a timestamp every 50 ms and requests a
repaint; the next frame records the wait. egui frames are paced by the display (about 16.7 ms
apart), so the Rust number includes waiting for the next frame slot, not just a busy UI thread.
The ticker also forces up to 20 frames a second, which inflates idle CPU. The rows marked
"no ticker" turn it off (`WANDUR_PERF_NO_TICK=1`) and are the ones to read for idle CPU.

The loopback server content has the same shape (word list, proportions, SGR mix) but not the
same bytes as the C# generator, because .NET's `Random` sequence is not reproduced. The harness
uses its own C# server for the app scenarios, so the app-level rows below saw identical traffic.
The "chat" scenarios matter only for C# (its Channels panel); the Rust app has no channel
classifier yet, so `flood-100k` and `flood-100k-nochat` do the same work there.

### App scenarios: Rust next to C#

Medians of 3 runs (startup: 6). C# columns are the cited post-optimization numbers
(`docs/baseline.md`); the same-day C# re-run is in the next table.

| Scenario | Startup ms Rust / C# | Working set MB Rust / C# | Footprint MB Rust / C# | Heap MB Rust / C# GC | CPU % Rust / C# | Alloc MB/s Rust / C# | Chars/s Rust / C# | UI p95 ms Rust / C# | UI max ms Rust / C# |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| startup (ticker) | 234 / 1149 | 83 / 202 | 229 / 226 | 2.4 / 26.9 | 9.3 / 2.7 | 7.5 / 0.0 | 0 / 0 | 0.4 / 0.1 | 11 / 17 |
| startup (no ticker) | 185 / 1149 | 85 / 202 | 225 / 226 | 2.4 / 26.9 | **0.2** / 2.7 | 0.1 / 0.0 | 0 / 0 | n/a | n/a |
| session-idle (ticker) | 218 / 1293 | 85 / 174 | 231 / 265 | 2.6 / 43.7 | 11.0 / 6.5 | 13.4 / 0.1 | 14 / 14 | 4.5 / 0.1 | 10 / 30 |
| session-idle (no ticker) | 326 / 1293 | 84 / 174 | 230 / 265 | 2.6 / 43.7 | **2.1** / 6.5 | 2.9 / 0.1 | 14 / 14 | n/a | n/a |
| flood-100k | 281 / 1213 | 89 / 270 | 235 / 498 | 3.7 / 81.8 | 20.8 / 35.7 | 75.3 / 23.1 | 100,116 / 100,437 | 6.7 / 3.9 | 11 / 19 |
| flood-100k-nochat | 275 / 1197 | 88 / 266 | 235 / 484 | 3.7 / 60.1 | 20.4 / 24.9 | 74.8 / 11.1 | 100,201 / 100,594 | 5.5 / 0.1 | 12 / 5 |
| flood-1m | 278 / n/a | 89 / n/a | 235 / n/a | 3.9 / n/a | 22.5 / n/a | 79.6 / n/a | 999,501 / n/a | 6.0 / n/a | 11 / n/a |
| flood-1m-nochat | 333 / 1234 | 88 / 245 | 234 / 472 | 3.8 / 56.0 | 21.1 / 29.6 | 77.9 / 69.8 | 998,762 / 1,001,576 | 5.6 / 12.8 | 10 / 26 |
| multi-4 | 307 / 1158 | 90 / 262 | 236 / 533 | 5.0 / 134.0 | 22.4 / 42.0 | 82.0 / 22.6 | 200,211 / 200,603 | 5.2 / 0.6 | 9 / 6 |
| multi-4-nochat | 309 / 1225 | 90 / 277 | 236 / 520 | 4.9 / 114.4 | 22.3 / 36.3 | 83.2 / 20.6 | 200,109 / 200,485 | 5.8 / 0.2 | 10 / 9 |
| multi-8-idle (ticker) | 290 / 1213 | 84 / 189 | 232 / 406 | 3.0 / 65.5 | 12.4 / 8.3 | 28.4 / 0.6 | 112 / 112 | 2.0 / 0.1 | 11 / 20 |
| multi-8-idle (no ticker) | 296 / 1213 | 85 / 189 | 231 / 406 | 3.0 / 65.5 | **2.4** / 8.3 | 5.7 / 0.6 | 112 / 112 | n/a | n/a |
| directory | not implemented | | | | | | | | |

Every Rust session scenario kept up with the server (100 KB/s, 1 MB/s, 4 x 50 KB/s) and rendered at
about 62 frames a second throughout (`framesPerSec` in the probe files).

#### Same-day C# re-run (sanity check)

Run immediately after the Rust runs, same harness, 3 runs each. It agrees with the cited numbers
(startup is about 150 ms faster today), so the comparison above stands.

| Scenario | Startup ms | Working set MB | Footprint MB | GC heap MB | CPU % | Alloc MB/s | Chars/s | UI p95 ms | UI max ms |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| startup | 1108 | 198 | 244 | 26.9 | 2.6 | 0.0 | 0 | 0.1 | 15 |
| session-idle | 986 | 183 | 258 | 43.8 | 6.6 | 0.1 | 14 | 0.1 | 5 |
| flood-100k-nochat | 1004 | 212 | 483 | 60.1 | 24.4 | 11.0 | 100,128 | 0.1 | 7 |
| flood-1m-nochat | 954 | 211 | 471 | 57.3 | 28.8 | 69.9 | 1,004,401 | 12.0 | 23 |
| multi-4-nochat | 1003 | 244 | 522 | 114.5 | 35.7 | 20.6 | 200,277 | 0.5 | 5 |

#### Footprint needs more than one sample for the Rust app

The harness reads the footprint once, at the end. For the Rust app that single reading depends
on whether frames are being drawn at that instant. Sampled 40 times over 10 s (after 5 s
settling, probe on without the ticker, `scripts/footprint-sample.sh`):

| Situation | Rust min / median / max MB | Rust peak MB | C# min / median / max MB | C# peak MB |
|---|---:|---:|---:|---:|
| No session, nothing redrawing | 65 / 66 / 66 | 225 | 224 / 224 / 225 | 388 |
| One idle session (prompt every 2 s, caret blinking) | 71 / 230 / 230 | 253 | 261 / 363 / 424 | 424 |
| One session at 1 MB/s | 230 / 231 / 231 | 231 | 476 / 514 / 521 | 522 |

So the Rust process holds about 65 MB when idle and about 230 MB while it renders. The extra
roughly 165 MB appears with drawing and goes away when frames stop: it is graphics memory
(IOSurface and Metal allocations from the wgpu backend), not application data. The live heap
stays at 3 to 5 MB in every scenario. A glow (OpenGL) build showed the same startup
footprint (214 to 225 MB, 3 runs); its other glow runs were discarded because the window was not
drawing (0 to 9 frames a second, probably occluded), so the renderer question is open.

### In-process measurements (`wandur-bench micro`)

`cargo run --release -p wandur-bench -- micro --label m1`. The C# column is `docs/perf.md`'s
`micro` after its optimizations.

| Measurement | Rust | C# |
|---|---:|---:|
| 100,000 lines (8.6 M chars, 4 KB chunks) into a 2,000 line scrollback | 36.6 ms, 18.1 MB allocated (224 MB/s) | 54.9 ms, 42.0 MB |
| One 4 KB chunk into a full scrollback | 0.018 ms, 8.9 KB | 0.023 ms, 19.2 KB |
| Plain text of the full scrollback | 0.012 ms | 0.321 ms |
| Retained, 2,000 lines | 328 KB (the only terminal model) | 914 KB line model + 6,026 KB display (xterm) |
| Wrap layout rebuild, 2,000 lines | 0.002 ms | n/a |
| Terminal frame at the tail, full scrollback, no new output (headless layout and tessellation) | 0.057 ms, 976 KB allocated | n/a |
| Terminal frame with a 4 KB append each frame | 0.386 ms, 2,045 KB allocated, 52 rows laid out | n/a |
| 1 session at 200 KB/s, core only, drained every 16 ms | 0.5% CPU incl. server, 0.4 MB/s allocated | 31% CPU, 17 MB/s (headless window) |
| 4 sessions at 100 KB/s each, core only | 1.1% CPU incl. server, 0.9 MB/s allocated | 32% CPU, 32 MB/s (headless window) |

The session rows are not like-for-like: the C# rows include a headless window and its display
model, the Rust rows are the network, telnet, decoding and buffer path alone. They show that
the session and parsing path costs about 1% of a core; everything else in the app-level CPU
figures is drawing.

### What the numbers say

- **Startup: better.** 185 to 330 ms to the first presented frame against about 1.1 to 1.2 s
  (683 ms for the packaged ReadyToRun C# build).
- **Memory: better, with a caveat.** Working set 83 to 90 MB against 174 to 277 MB; heap 2 to 5 MB
  against 27 to 134 MB GC heap; terminal model 328 KB against about 6.9 MB per session. Footprint
  under load is about 230 MB against 470 to 530 MB, and it does not grow with sessions (4
  sessions: 236 MB against 520 to 533 MB). Idle without redraws, 65 MB against 224 MB. But
  about 165 MB of the Rust footprint is graphics memory that appears whenever it draws, so
  "idle with a session" (230 MB) is close to the C# idle figure (224 to 265 MB).
- **Sustained output: about equal throughput, somewhat less CPU.** Both apps keep up at 1 MB/s and
  4 x 50 KB/s. CPU is 20 to 22% against 25 to 42%. The Rust app redraws at 60 frames a second
  while output flows, where the C# app flushes every 60 ms; the micro numbers show the parsing
  path is about 1% of a core, so nearly all of the Rust CPU is egui drawing.
- **Idle CPU: better.** 0.2% with no session, 2.1 to 2.4% with sessions (caret blink causes
  about 9 frames a second), against 2.6% and 6.5 to 8.3%.
- **Allocation rate: worse.** 75 to 83 MB/s under flood against 11 to 70 MB/s. It is short-lived
  (no collector, live heap stays at 4 MB), and the micro rows show where: egui rebuilds the
  visible rows' layout jobs and shapes every frame (about 1 to 2 MB a frame).
- **UI latency: inconclusive.** Rust p95 of 5 to 7 ms and max of 9 to 12 ms are dominated by waiting
  for the next display-paced frame; C# p95 is 0.1 to 12.8 ms with maxima up to 26 ms. The two
  probes do not measure the same thing.

### Follow-ups (not done in Milestone 1)

- Cap redraws under flood (for example a 30 Hz output repaint) and measure CPU against
  responsiveness. The C# app's 60 ms flush is the reference point.
- Cache layout jobs per visible line to cut per-frame allocation; keep only if the allocation rate and
  CPU numbers move.
- Find out what the roughly 165 MB of graphics memory is (wgpu surface configuration, frames in
  flight, present mode) and measure glow with the window kept visible.
- Optionally disable caret blink (or slow it) and measure idle CPU.
- A Rust-side equivalent of the `directory` scenario once the directory exists (Milestone 3).

Raw files: `.superpowers/csharp-ref/.superpowers/perf/logs/` (`app-rust-m1*.json`,
`app-csharp-sameday.json`, `probe-*.jsonl`) and `.superpowers/perf/micro-m1.md`.
