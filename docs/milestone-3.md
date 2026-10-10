# Milestone 3 report

2026-10-08. Workspace and application UI, matching the C# client's behaviour and information
hierarchy first. Screenshots are in `.superpowers/shots/m3/` (not committed): `m3-shell-light.png`
(Hull), `m3-shell-dark.png` (Ember), `m3-directory.png`, `m3-directory-dark.png` (Midnight),
`m3-world-detail.png`, `m3-settings.png` (Paper), and `m3-real-directory-offline.png` (the real
wandur.net snapshot, fetched once and read offline).

## Implemented

- **Workspace shell** (egui_dock), laid out as the C# client: Workspace on the left (0.18), the
  documents in the middle (Find a MUD and one tab per session), Map over Channels on the right
  (0.58 / 0.42); a top bar (View menu, connect box, recent addresses, Settings) and a status bar
  (`n sessions · m connected`, the active session's state, the C# hint). Every panel closes and
  comes back from View; tabs drag to rearrange or float; Ctrl+Tab and Ctrl+Shift+Tab move through
  Find a MUD and the sessions.
- **Layout persistence**: `layout.json` in the data directory, saved when it changes (checked
  every two seconds, written on a background thread) and on exit, restored on start. It is our own
  small description (splits with fractions, leaves with panel names), rebuilt through egui_dock's
  split calls, so any file that parses gives a valid dock. A corrupt or unsupported file is moved
  to `layout.json.bad` with a note in the top bar; unknown panels are dropped; panels that floated
  come back docked; Reset layout in View and Settings. Sessions are not saved.
- **Workspace panel**: Find a MUD; Open sessions with a connected (filled) or not (hollow) dot,
  "New activity" for unseen output, a close button, a context menu (Open, Rename, Reconnect,
  Close) and inline rename; Saved worlds (collapsible) with Connect, Add, Edit, Delete (two-step,
  naming the world) and Find, 40 by 30 thumbnails from the directory's artwork, initials while
  there is none, most recently used first, double-click or Enter to connect.
- **Saved worlds**: add and edit in a modal form (name, host, port, TLS, text encoding, automatic
  reconnect) with validation; delete; connect; usage (last connected, count) and the directory
  listing they came from, all in `settings.json` (older files still load).
- **World directory**: fetched on a background thread the first time Find a MUD is shown (and on
  Refresh, at most every five minutes otherwise), cached in `directory.json` for offline use, read
  from the cache at start. Search with the C# scoring (name, address, tags and features,
  description, near misses), the online and sort selects (best match, name, players, rating,
  recently updated, newest), further filters (genre, game type, language, roleplaying, player
  killing, codebase, development stage, world size, location, tag, player range, rating, TLS,
  connection kind, adult worlds), the count line, the empty states, and the footer status. Cards
  follow the C# row in its three width modes (plate, name, pills with the online count, blurb,
  bottom pill, Explore world, Add to my worlds). The list is virtualized: only cards in view are
  laid out and ask for artwork.
- **World page** (Explore world): breadcrumbs, the hero picture with the name and tagline over a
  scrim, chips, the online count, Connect and Add to my worlds, About (description, Find your
  place) beside World details (facts, the address with Copy and Use TLS, links, the quieter facts,
  attribution); one column on a narrow tab; Escape goes back to the list where it was.
- **Artwork pipeline** (brief Milestone 4 items 5, 7 and 9): four workers; JPEG decoded with a
  scaled IDCT (1/2, 1/4, 1/8), PNG reduced row by row (the full bitmap never exists), other formats
  decoded whole; cancellation when a card leaves the screen (queued jobs never fetch, running ones
  stop between steps, late results are never uploaded); textures in an LRU with a 24 MB budget,
  released by dropping the handle (egui frees the GPU texture at the end of that frame); a 64 MB
  disk cache of resized thumbnails. Nothing but texture creation runs on the UI thread.
- **Channels panel**: GMCP `Comm.Channel.Text` and Aardwolf's `comm.channel` into an All tab and
  one tab per channel (tells and pages first), 500 messages each, unread counts on the tabs and on
  the Channels tab title, time, speaker and the text's own colours, a reply box that sends on the
  selected channel (`gossip text`, `tell <speaker> text`) and the C# hints when it cannot. Updates
  are incremental: one layout per new message, only visible rows painted (tested).
- **Map panel**, marked INITIAL MAP: rooms and exits from GMCP `Room.Info` on a grid, placed from
  the exit that leads to a room or the direction walked, nudged when the spot is taken; the
  current floor only, stairs marked, the current room highlighted, pan by dragging, zoom with the
  wheel or a pinch, Centre; the room's name, area and count; a tooltip per room; the C# empty
  state text when there is no room data.
- **Settings**: theme, scrollback, font size, output frames a second, sent command echo,
  automatic reconnect, prompt delay, the directory address (with what is in use and why), adult
  worlds, Reset layout; applied at once and saved.
- **Status and activity**: tab titles (connecting, closed, unseen output), the Workspace panel,
  the status bar, the channel tabs.
- **Themes**: the eleven C# presets (Hull, Ember, Moonlight, Forest, Midnight, Slate, Rose, Paper,
  Parchment, Daylight, Linen) with their chrome colours, map colours and sixteen colour terminal
  palettes, applied to egui's light or dark visuals and to the terminal; Hull (the C# default) is
  light chrome around a dark transcript with light transcript text, as in C#. Coloured channel
  text uses a palette suited to the panel's lightness. Fleet and Armored skin art, world themes
  and custom themes are deferred.
- **Accessibility** (AccessKit): the transcript widget is a read-only multi-line text node labelled
  "Transcript" whose value is the rows on screen, and a polite live region "Latest output" holds
  the last five lines while the view is at the bottom. Built only when assistive technology is
  active (tested headless). Not yet tried with a screen reader.
- **Bench**: `wandur-bench directory-server` (loopback directory with generated 4000 by 3000 JPEG
  and PNG art, `--varied` for different genres, counts and ratings), `mud-server --page gmcp` (a
  small world that walks rooms and talks on channels), directory and artwork micro measurements,
  and the probe's `directory` scenario so the C# harness drives the Rust app the same way.
- **Network rule**: the directory address is `WANDUR_DIRECTORY_URL`, then `--directory-url`, then
  the setting, else `https://api.wandur.net/`. Requests carry `User-Agent:
  WandurRustPrototype/0.0.1 (macOS; aarch64)` and no install id. During this milestone the real
  directory was requested once (to check the wire format); every test and measurement used loopback
  servers.

## What remains

- Pattern-based channel capture (C# rule sets, the teaching dialog), trimming the `[channel] name:`
  prefix from the panel's text (C# does it when a rule matches).
- The full mapper (text-only room tracking, areas, search, walking, editing, saved maps).
- Login automation and credentials, scripts, macros, protocol mappings in the world form.
- The C# reorder-while-hovering rule for saved worlds; world themes and skins; custom themes.
- Transcript export UI, protocol diagnostics panel, history, everything else marked Deferred in
  `docs/parity.md`.
- Accessibility: text ranges and caret navigation inside the transcript (AccessKit `TextRun`
  nodes with character positions), announcing only new output rather than the last lines, and a
  person checking VoiceOver, NVDA and Orca.

## Tests run

| Command | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets -- -D warnings` | clean |
| `cargo test` | 155 passed: core 79, term 22, app 50, bench 4 |
| `cargo build --release` | ok |
| `cargo check --target wasm32-unknown-unknown -p wandur-core --no-default-features` | ok |
| `cargo run --release -p wandur-bench -- micro --label m3` | `.superpowers/perf/micro-m3.md` |
| C# harness: `directory` (Rust and C#), `startup`, `session-idle`, `flood-1m-nochat`, `multi-4-nochat`, `multi-8-idle`, with and without the ticker | see `docs/measurements.md` |

New tests: snapshot parsing with nulls, bad entries and other formats; live counts; row pills;
search scoring, every filter and every sort; facet options and counts; the directory service
(fetch, cache, throttle, offline with a warning, corrupt cache) with a fake fetcher; the HTTP
client and art address rules; channel decoding, tabs, unread counts, private ordering, bounds;
the Channels panel's incremental layout and bounded painting (headless); map parsing, placement
and nudging, and drawing (headless, including the empty state); layout round trip, unknown and
duplicate panels, corrupt and old files, floating panels, empty layouts; the default layout and
reopening panels; saved worlds (edit, delete, usage, recent order, Milestone 2 files) and the
world form; artwork decoding (scaled JPEG, row-by-row PNG, other formats, garbage), cancellation
(cancelled jobs neither decode nor upload), the LRU budget, remembered failures, the disk cache;
every theme preset's contrast and lightness, applying light and dark themes; AccessKit nodes only
when enabled; the loopback directory server against the real client code.

## Known limitations, and where egui was clunky or expensive

- **Rich layouts**: there is no layout engine beyond rows, columns and grids. The C# card
  measures its children (the way-in column is sized to its text, the pills wrap with a measured
  gap); here every card has a fixed height per width mode and the name is one line, because
  `ScrollArea::show_rows` needs uniform rows to virtualize. Flowing pills, measured columns and
  "one line if it fits, else two" took hand-written arithmetic. The world page is a plain
  vertical stack; the C# "chips and actions on one line when both fit" is approximated.
- **Images**: egui has no image widget that crops to cover a box with rounded corners; plates are
  painted rectangles with UV coordinates. Every texture is GPU memory counted in the footprint:
  the first texture budget (96 MB) made the directory footprint worse than C#'s until measured
  and lowered.
- **Scrolling of cards**: virtualization is cheap (five cards laid out), but immediate mode lays
  them out again on every frame anything repaints, about 1.5 MB allocated per frame on the
  directory page. It costs nothing when nothing repaints.
- **Text inputs**: `TextEdit` has no built-in clear button; the facet selects are combo boxes
  without type-to-filter, which is clumsy for long lists such as tags.
- **Theming**: egui's `Visuals` covers the chrome well (light and dark both work), but the C#
  skins (nine-slice art, bezels, ornaments) would need a custom painter for every frame and tab;
  egui_dock's tab bar style is separate from egui's and was themed by hand.
- **Fonts**: the interface font has no arrows, check marks or triangles (U+2192, U+2713, U+25BE);
  the UI uses plain text instead (`>`, "Saved", "(hide)"). A symbol font would fix it.
- **Accessibility**: as above; the painted grid is only reachable through hand-built nodes.
- **The Channels panel** shows the world's own line, so the speaker appears twice
  (`Ann: [gossip] Ann: ...`) until prefix trimming exists.
- **Not tried by a person**: docking moves, the world form, the directory with a mouse, screen
  readers. Covered by tests and screenshots only.

## Performance and comparison

From `docs/measurements.md`:

| | Rust M3 | Rust M2 | C# |
|---|---:|---:|---:|
| Directory scenario: footprint / working set (MB) | 264 / 112 | n/a | 301 / 199 (cited 297 / 212) |
| Directory scenario: heap (MB), CPU with ticker / without (%) | 5.4, 12.3 / 0.6 | n/a | 37.3 GC, 12.2 |
| Directory filter / sort, 500 worlds (ms) | 0.023 / 0.017 | n/a | 2.256 / 0.573 |
| Thumbnail 4000x3000 JPEG to 800x320 (ms, peak) | 30.7, +14 MB | n/a | 15.8, +15 MB |
| Thumbnail 4000x3000 PNG to 800x320 (ms, peak) | 78.5, +2.8 MB | n/a | 89.4, +68 MB |
| Startup to first frame (ms) | 240 to 320 | 200 to 310 | about 1,200 to 1,500 |
| Idle CPU, 1 / 8 sessions, no ticker (%) | 1.6 / 1.8 | 1.6 / 2.0 | 6.5 / 8.3 |
| CPU, flood-1m-nochat / multi-4-nochat (%) | 19.0 / 18.8 | 17.7 / 15.9 | 29.6 / 36.3 |
| Working set, 4 sessions flooding (MB) | 110 | 109 | 277 |
| Footprint, 1 session at 1 MB/s (MB) | 242 | 241 | 472 |

### Better, worse or inconclusive, for the UI-heavy parts

- **Directory memory: better** (footprint 12% lower, working set 44% lower, heap 7 times
  smaller), after lowering the texture budget; worse with the first budget. The lesson: on this
  renderer GPU textures dominate, and a byte budget is the control that matters.
- **Directory queries: better**, by two orders of magnitude.
- **Virtualization and cancellation: equal behaviour** (5 cards alive at most in both), with
  cancellation tested to the point of "never decoded, never uploaded".
- **Artwork decoding: inconclusive** (PNG much leaner, JPEG twice as slow).
- **Panel drawing cost: slightly worse than M2** under output (one to three points of CPU, more
  allocation), still well below C#.
- **Building rich UI: worse to write than Avalonia.** Cards, the world page and flowing chips
  took more hand layout than the C# views, and some C# niceties were approximated. Behaviour and
  information hierarchy match; visual polish does not yet.
- **Theming: equal for palettes, worse for skins** (no practical path to the C# skin art without
  custom painting).
- **Accessibility: inconclusive**; the transcript can be exposed, but egui's AccessKit support is
  thin for custom widgets and has not been tried with a screen reader.

## Verdict and recommendation

Rust with egui and egui_dock carries the whole workspace with less memory than C# in every
scenario measured, faster startup, much faster directory queries and lower CPU under output. The
cost is UI authoring effort: rich, responsive layouts need hand arithmetic and some C# polish is
approximated, and GPU texture memory has to be budgeted by hand. Nothing found is a blocker.

**Recommendation: continue to Milestone 4** (performance), with three items carried in: a faster
JPEG path (libjpeg-turbo binding or zune-jpeg with a pre-scale), drawing side panels only when
their data changes, and a person trying docking, the directory and a screen reader.

## Try it

```sh
cd "/Volumes/Extreme SSD/workspace/wandur/wandur-client-rust"
cargo build --release

# A local directory with 300 worlds and large artwork (the first start makes the art, a few seconds)
./target/release/wandur-bench directory-server --port 4402 --worlds 300 --varied &
# A small GMCP world: rooms for the Map, chatter for the Channels
./target/release/wandur-bench mud-server --port 4403 --rate 0 --page gmcp &

# The client on the local directory, connected to the GMCP world
WANDUR_DIRECTORY_URL=http://127.0.0.1:4402 ./target/release/wandur 127.0.0.1:4403 --data-dir /tmp/wandur-try

# Other themes: --theme Ember (or Midnight, Paper, Linen...); straight to a page: --show world:bench-world-6

# The real wandur.net directory (the default when nothing else is set); a separate data directory
./target/release/wandur --data-dir /tmp/wandur-real
```

C# harness for the directory scenario (it starts its own loopback directory with 4000 by 3000
art and points the client at it):

```sh
cd .superpowers/csharp-ref
dotnet bench/Wandur.Bench/bin/Release/net10.0/Wandur.Bench.dll app directory --runs 3 --label mine \
  --app "$PWD/../../target/release/wandur"
```

In the client: Find a MUD to search, filter and sort; Explore world for a world's page; Add to my
worlds or Connect; the saved worlds appear in the Workspace panel with their pictures; drag tab
headers to rearrange, View to reopen a closed panel or Reset layout; quit and start again to see
the layout restored. Stop the servers with `kill %1 %2`.
