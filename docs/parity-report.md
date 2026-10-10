# Parity report

Final audit of the parity run (tasks t01 to t16), 2026-10-09, at commit 9e55a7c plus this audit's
documentation commit. The goal was feature parity with the C# client (main at 27f66d2 plus its
`feature/mudlet-import` branch). Feature by feature status: `docs/parity.md`. Screens side by
side: `docs/screens/README.md`. How to change the look: `docs/ui-tweaking.md`.

## Verdict in one paragraph

The Rust client does what the C# client does, on macOS, for nearly every feature: sessions,
protocols, login, macros, JavaScript and Lua scripts with panels and packs, Mudlet import, the
mapper and map editor, channels, history search, local model agents, skins and themes, settings,
localization in five languages. Of 58 feature rows, 38 are Done, 16 Partial (a named piece missing
or untested), 3 Deferred (importing C# data, the address lookup on The Mud Connector, and the
update check, which waits for Rust releases) and 1 out of scope. It is still faster and much lighter than
C# under output, but it gave back part of the Milestone 4 lead: with four busy sessions it now
uses about 1.6 times the CPU it did at Milestone 4 (still about 2.5 times less than C#). The main
risks are things nobody has run: Windows and Linux, a real screen reader, and the owner's own
MUDs. My recommendation is below.

## What was built (t01 to t16)

| Task | What | Review result |
|---|---|---|
| t01 | Client database (`wandur.db`, migrations, WAL), world ids, headless scene capture | passed |
| t02 | Localization (the C# resx tables, five languages), menu bar, Help, About, offline demo world | passed after a fix |
| t03 | Macros: triggers, aliases, timers, F-keys, per world, live switch | passed |
| t04 | Password vault, auto-login (text and GMCP), private input, session character | passed |
| t05 | Composer (Look, Commands, Send), live tail split, link confirmation, completion, MUD colours | passed |
| t06 | MSDP, protocol state cache, Diagnostics page (messages, observed fields, console, server details), mapped vitals | passed |
| t07 | Session history with FTS5 search, retention, History window | passed |
| t08 | Channel rule families, Mark as channel teaching dialog, world rules | passed |
| t09 | JavaScript scripts (QuickJS) with the C# `mud` API, script library, code editor with completion | passed |
| t10 | Script panels in the session rail, bars gauges, directory script packs | passed |
| t11 | Lua 5.4 on the same host API, the clean-room Mudlet layer, Mudlet profile and package import | passed |
| t12 | Mapper: tracking, saved maps, search, routes, double-click walking | passed |
| t13 | Map editor, map import and export, room terrain inference (ONNX, opt-in feature) | passed |
| t14 | Local model agents (LM Studio and OpenAI-compatible), goals, runner | passed |
| t15 | Fleet, Armored and System skins with drawn title bars, world themes, custom themes | passed |
| t16 | Settings sweep, update check, install id, accessibility names, packaging (macOS) | passed |

Every task kept the app runnable and was reviewed against its acceptance list and the C#
reference screens. No review found a blocking problem.

## Checks at HEAD (this audit)

| Check | Result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy --all-targets -- -D warnings` | pass |
| `cargo test` | pass: 913 tests (core 587 + 3, term 28, app 251 + 3 UI string tests, bench 41), 0 failed |
| `cargo build --release` | pass |
| `scripts/capture-scenes.sh` | all 70 scenes captured; every request stayed on 127.0.0.1 |

The bench timing test `no_long_ui_thread_stalls_under_a_flood` failed once during the t12 review
on a busy machine and passed every run since; it is the test most likely to flake.

## What is left

Partial and Deferred rows, short form (details in `docs/parity.md`):

- **Platforms**: Windows and Linux never ran (caption buttons, drag and resize, the credential
  stores, the packaging scripts). No signing or notarization (owner's certificates).
- **Shell look**: since ui-chrome (2026-10-09) the C# headers, drop preview, title bars, Saved
  worlds panel and the macOS menu bar are in, plus a title bar menu button; the UI font is egui's default, not Inter, with no semibold; world
  theme pictures not drawn; preset swatches draw faded; the Hull map is light where C# is dark.
- **Saved worlds and directory**: recency-weighted order logged but not used; no address lookup
  in the world editor (needs mudconnect.com); a saved world at a listing's address shows In my
  worlds where C# offers Add to my worlds.
- **Session**: last character per world not remembered; OSC 8 links not read; Edit menu enable
  rules looser than C#.
- **Channels**: only saved worlds can be taught; regex crate syntax.
- **Scripts**: login-time values not replayed to running scripts; no catalog refresh before a
  world opens; QuickJS limits are not Jint's exactly.
- **Mapper**: no protocol knowledge store.
- **Terrain inference**: off in the default build; no Download button (install from a local file).
- **Accessibility**: every control named and tested, but no one has listened with VoiceOver, NVDA
  or Orca; no text navigation inside the transcript.
- **Updates**: the check is off unless the build is packaged with a release version, because
  wandur.net lists only C# releases.
- **Data**: since 2026-10-09 File > Import from Wandur (C#)... and `wandur --import-csharp` bring
  the C# client's worlds, preferences, scripts, macros, channel rules, agent settings, maps,
  history and (when asked) saved passwords over, read-only on the C# side
  (`docs/csharp-import.md`). Both clients still keep separate data directories and Keychain
  entries.

## Open findings

From the per-task reviews, not fixed and not all listed in the parity table. None blocks use;
the first four are the ones I would fix first.

1. **Ad hoc session stays unteachable after Save world** (t08). `AppAction::SaveWorld` in
   `crates/wandur-app/src/app.rs` never sets `tab.world` on the open session, so Mark as channel
   keeps saying "Open a saved world" until a reconnect.
2. **History refresh misses recent text** (t07). Search / refresh does not flush open recorders,
   and batches wait up to 2 s, so the last couple of seconds may be missing from a refresh.
3. **Scenes touch the data directory's database** (t01). `WandurApp::new` opens `wandur.db` even
   for ephemeral scene runs, which can create, migrate, or move aside a damaged file there.
4. **Agent Stop can leave two requests on one model server** (t14). A cancelled request is
   abandoned while still running, and its server slot is released at once; repeated Play and Stop
   also leaves threads alive up to 120 s.
5. Changing a world's host or port keeps its old codebase (C# clears it) (t08).
6. Two saved worlds at one address share one world id (t01).
7. The terrain confidence slider does not reclassify rooms already classified (t13).
8. Vault key JSON is not the C# digest (keys sorted). Harmless because the Keychain services are
   separate; the misleading code comment was corrected in this audit (t04).
9. Credentials: the vault entry can be deleted before `settings.json` is written; keychain prompts
   on save block the UI; passwords are not zeroed after login (t04).
10. Test gaps: no 30 s GMCP login timeout test; no app-level login-gating test for triggers; the
    8-table MSDP depth boundary is untested and its comment is wrong; several C# theory cases
    ported as one test; no test for the native file pickers.
11. Performance margins that are close: 1,000 triggers about +6 to +14% per flood frame (t03); a
    script trigger +14% (limit 15%) (t09); text room observer +15 to +26% on some flood rows
    (t12, a ruling).

## Performance: final against Milestone 4 and C#

Same machine (Apple M3 Pro, macOS 26.5), same session, the shared C# harness
(`.superpowers/final-harness.sh`), alternating clients. The final Rust binary is HEAD
(`.superpowers/bin/wandur-final`), Milestone 4 is `.superpowers/bin/wandur-m4`, C# is the exported
reference at 27f66d2. The machine was busy (load average 5 to 7), so single runs varied by up to
two times; medians are shown and the spread is given where it matters.

**Steady stream** (the realistic flood: 500 writes a second, the probe directly, no ticker):

| Scenario | CPU % final / M4 / C# | Alloc MB/s final / M4 / C# | Working set MB final / M4 / C# | Heap MB final / M4 / C# GC |
|---|---:|---:|---:|---:|
| 1 session at 1 MB/s (2 runs each) | 10.3 / 9.1 / 33.6 | 43 / 29 / 85 | 105 / 91 / 217 | 13.9 / 10.5 / 95 |
| 4 sessions at 250 KB/s (5 runs final and M4, 2 C#) | 24.2 / 15.0 / 61.7 | 56 / 39 / 125 | 158 / 122 / 313 | 37.6 / 31.1 / 171 |

Four-session spread: final 13.2 to 25.5, M4 6.8 to 15.9, C# 58.7 to 64.7. With history
recording off, final read 8.1 to 22.0 (median 18.4), so on this noisy machine history recording
is not clearly the cause of the extra CPU; it does account for a few MB/s of the allocation.

**Harness scenarios** (bursty server, probe ticker on, medians of 3; startup 6):

| Scenario | Startup ms final / M4 / C# | Working set MB | Heap MB (Rust live / C# GC) | CPU % | UI p95 ms |
|---|---:|---:|---:|---:|---:|
| startup | 377 / 370 / 1,046 | 89 / 86 / 157 | 3.9 / 3.8 / 26.9 | 10.9 / 10.7 / 2.5 | 0.4 / 0.4 / 0.1 |
| session-idle | 349 / 289 / 1,136 | 92 / 86 / 168 | 8.1 / 6.2 / 43.7 | 11.8 / 7.2 / 5.5 | 0.5 / 0.4 / 0.1 |
| flood-1m-nochat | 622 / 398 / 1,060 | 110 / 92 / 190 | 14.0 / 10.6 / 59.2 | 23.3 / 17.3 / 20.2 | 1.0 / 0.9 / 4.8 |
| multi-4-nochat | 472 / 342 / 1,152 | 117 / 107 / 203 | 36.1 / 31.1 / 114.5 | 14.7 / 6.1 / 29.6 | 1.5 / 0.3 / 0.3 |
| directory (300 worlds, 4000x3000 art) | 295 / 270 / 1,238 | 90 / 87 / 131 | 5.9 / 5.4 / 37.2 | 5.0 / 4.6 / 29.9 | 0.2 / 0.3 / 0.1 |

Without the ticker (what the app does by itself), flood-1m-nochat: 14.4% final against 3.9%
M4 (runs 9.8 to 15.0 against 3.3 to 8.1); multi-4: 8.8% against 4.1%. Idle: the harness showed
two of three final runs repainting for a while (19% and 8% CPU); a separate rerun of seven idle
runs (`.superpowers/audit/idle.sh`) gave 0.8 to 1.4% CPU at 3.4 frames a second, the same frame
rate as M4 (0.7 to 0.8%). I could not reproduce the idle burst; it is worth watching.

What the numbers say:

- **Against C#: still better** where it counts: startup about 3 times faster, working set half,
  heap 3 to 5 times smaller, CPU under sustained output about 3 times lower (1 session) and 2.5
  times lower (4 sessions). **Worse** at idle with the probe's ticker (it forces frames C# does
  not draw), and allocation rate when idle.
- **Against Milestone 4: a real regression under output.** Every feature added per line of server
  text (triggers on a worker, channel classification and the text room observer on the UI thread,
  completion learning, history recording, the protocol cache) costs a little; together, with four
  sessions, about +9 points of CPU, +17 MB/s allocation and +36 MB working set. Each task's own
  check stayed inside its limit against the build before it; the sum is the cost of parity.
  Startup with a connecting session is slower (622 against 398 ms in the flood scenario) and was
  not looked into.
- The cheapest wins if this matters: move the text room observer and channel classification off
  the UI thread (as completion and triggers already are), and look at history batching.

Raw data: `.superpowers/csharp-ref/.superpowers/perf/logs/app-{rust-final,rust-m4again,csharp-final}*`,
`.superpowers/perf/steady/steady{1,4}-*`, `.superpowers/audit/`.

## Risks

- **Untested platforms.** Windows and Linux code compiles but never ran. Window chrome is custom
  drawn there, so the first run may show real problems.
- **Real MUDs.** All testing was against loopback bench pages and fixtures. Real servers vary
  more (odd telnet, charsets, MSDP shapes, prompts).
- **Accessibility** not checked by a person with a screen reader.
- **Two clients, two data sets.** The importer (2026-10-09) moves the C# data over; it was tested
  only on synthetic fixtures made by the C# stores, never on a real C# data folder.
- **Performance drift.** The margin over C# is still large, but Milestone 4's gains were partly
  spent, and three per-frame costs sit close to their limits.
- **Translations** of strings only this client has were written by agents, not reviewed.
- **Dependencies**: QuickJS (rquickjs), Lua (mlua, vendored C), tract-onnx (opt-in). The script
  engines run in-process on threads; a native hang would leave a stray thread rather than a killed
  worker process as in C#.

## Recommendation

Honest view: this is a credible replacement candidate on macOS, not yet a release. The feature
gap is small and named, the code is tested and clean (913 tests, clippy clean), and it is clearly
lighter than the C# client. What it has not had is time in front of a person.

I would do, in this order, before deciding to switch:

1. Play on your own MUDs with it for a week on macOS (`target/release/wandur`), and tweak the look
   with `docs/ui-tweaking.md` while you do. Most of what is left is look and feel, and that is
   where you will have opinions.
2. Fix open findings 1 to 4 above (small).
3. Win back the output CPU (observer and classifier off the UI thread) if the numbers matter to you.
4. Run it once on Windows and Linux.
5. Import your C# data once (`docs/csharp-import.md`; a dry run first) and check it in the app.

If after step 1 it does not feel better than the C# client to use, it is fine to throw it away:
the C# client is complete and the Rust one taught us where its costs are.

## How to run and try it

```sh
cd wandur-client-rust
cargo build --release

# Your own data directory, real MUDs (outside this audit's no-network rule):
target/release/wandur

# A throwaway directory and the offline demo (File > Open Offline Demo):
target/release/wandur --data-dir .superpowers/try-data

# Any screen in a window, against the loopback Lantern Road:
target/release/wandur-bench mud-server --port 4400 --page lantern &
target/release/wandur --data-dir .superpowers/try-data --scene session-play

# All screenshots:
scripts/capture-scenes.sh .superpowers/shots/try

# macOS app bundle and disk image (unsigned):
scripts/package-macos.sh
```

The room terrain classifier is opt-in: `cargo build --release --features wandur-app/classifier`,
then install the model package from a local file in Map tools.
