# Milestone 2 report

2026-10-08. Core client, with a terminal and text spike first as a decision gate.

## Decision gate (Part A): passed

The owner chose `alacritty_terminal` as the grid, one canonical model, bundled fonts with lazy
system fallback, and a redraw cap. Each was tried and measured before the rest was built.

- **egui_term**: MIT, but version 0.1.0 targets egui 0.31 and alacritty_terminal 0.25, runs a PTY
  backend and draws one text shape per cell. It cannot be used with egui 0.36, so the renderer is
  our own (`terminal_view.rs`); egui_term was read as a reference only.
- **Selection: workable.** It is alacritty's selection over the whole grid. Tests cover selection
  across the scrollback, soft-wrapped rows copied without extra newlines, word and line modes, a
  selection surviving 20 new lines of output, and one cleared when its rows leave the scrollback.
  A headless egui test drives the real widget with pointer events: press on a row, hold the pointer
  above the terminal for ten frames (the view scrolls into history), release, and the copied text
  is the consecutive lines from history to the start point. Not yet tried by a person with a mouse.
- **Unicode rendering: workable, with limits.** Screenshots of the Unicode test page:
  `.superpowers/shots/m2-unicode-full.png` (whole page) and `m2-unicode-1.png` (lower half):
  CJK takes two cells and the `|` columns line up, precomposed and combining accents look the
  same, box drawing joins, block elements fill their cells (drawn as rectangles; the font's own
  are shorter than a row), symbols come from the system fallback, colours, bold face, dim, italic,
  underline, strike and inverse render. Emoji are monochrome (egui's Noto Emoji) or boxes; Arabic
  is unshaped; Hebrew and Arabic are not reordered.
- **Fonts**: JetBrains Mono Regular and Bold (OFL, licence bundled), same advance, so bold stays
  on the grid. Missing characters are looked up on a worker: system font index 221 to 356 ms
  once, search 63 to 108 ms, faces memory mapped (Arial Unicode MS, 22 MB file, and Apple
  Symbols); the only UI pause is one 11 to 21 ms frame when the atlas is rebuilt.
- **Redraw cap: kept.** Under a steady 1 MB/s stream, 30 output frames a second gives 18 to 20%
  CPU and about 60 MB/s allocated, against 41 to 44% and 206 to 252 MB/s uncapped (120 frames a
  second on this display). Making it work needed two egui findings: a zero-delay repaint request
  costs two frames, and a delayed request from another thread paints at once (see
  `docs/architecture.md`, Repaint policy).
- **Performance against M1**: CPU and allocation better under every flood, idle better (after
  turning off the caret blink, which cost about ten frames a second), throughput equal, memory
  per session worse (about 6 MB of grid at 120 columns and 2,000 rows against 328 KB) but still
  below the C# client's display buffer. No regression that needed stopping.

## Implemented

- `wandur-term` (new crate): the session's `alacritty_terminal` grid fed bytes directly; LF as
  CR LF; bounded scrollback in rows (default 2,000, hard maximum 50,000, settable at run time);
  reflow on resize; wide and combining characters; local echo written straight to the grid; opt-in
  line events read back from the grid (bounded); current line for prompts; transcript text;
  selection; spare row release.
- Renderer: runs per style, characters outside the bundled font placed on their own cells, block
  elements as rectangles, selection highlight, scrollbar, wheel, PageUp/PageDown, Latest output,
  mouse selection (drag, double and triple click, Shift+click, drag past the edge scrolls), copy
  with Cmd/Ctrl+C when the input line has no selection of its own.
- Fonts: bundled JetBrains Mono Regular and Bold, lazy system fallback (fontdb, skrifa, memmap2).
- Output pacing: `--output-fps` (default 30) and a Settings slider; steady caret.
- Transport trait (`Link` of reader, writer and closer) with TCP and TLS (rustls 0.23, ring
  provider, OS certificate verifier, `tls://host:port`), behind the `tls` feature.
- Telnet: NAWS (on request and whenever the grid size changes), TTYPE with the MTTS cycle
  (`Wandur-Rust`, `XTERM-256COLOR`, `MTTS 267` plus UTF-8 and TLS bits), GMCP with `Core.Hello`
  and `Core.Supports.Set`, MSSP; GMCP and MSSP decoded in `protocol.rs` into session events.
- Latin-1 as well as UTF-8, per saved world.
- Prompts: GA/EOR marks, or an unterminated line quiet for 250 ms (configurable); password
  prompts mask the next command.
- `Connection`: manual reconnect in the same tab and transcript; optional automatic reconnect with
  backoff (2 s doubling to 60 s, give up after 10, reset after 30 s connected), notices in the
  transcript, Stop while waiting.
- Settings (`settings.json`, versioned, clamped, atomic writes, background saver, unreadable file
  moved aside) in `--data-dir` or the platform data directory under `Wandur-Rust`; Settings tab;
  saved worlds (name, address, TLS, charset, reconnect) in the Sessions panel; recent addresses.
- Activity indicators from the grid's line sequence numbers; GMCP count and MSSP name in the
  status line.
- Bench: Unicode test page (`--page unicode`), steady stream mode (`--write-ms`), grid micro
  measurements, `memprobe` example.
- Removed: the M1 ANSI parser, line buffer and style module (the grid replaces them).

## Remaining (Milestone 3 onward)

Transcript export and channels UI (line events are ready), GMCP-driven panels (vitals, room,
channels), MSDP, saved world editing beyond name/charset/reconnect, login automation, layout
persistence, world directory, artwork, themes, history persistence (deliberately not done:
commands can hold passwords), live tail split, terminal cursor display, everything listed as
Deferred in `docs/parity.md`.

## Tests run

| Command | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets -- -D warnings` | clean |
| `cargo test` | 94 passed: core 51, term 22, app 18, bench 3 |
| `cargo build --release` | ok |
| `cargo check --target wasm32-unknown-unknown -p wandur-core --no-default-features` | ok |
| `cargo run --release -p wandur-bench -- micro --label m2` | `.superpowers/perf/micro-m2.md` |
| C# harness (`rust-m2`, `-final`, `-startup`, `-notick`, `-noblink`) and steady stream runs | see `docs/measurements.md` |

New tests cover: telnet NAWS, TTYPE/MTTS cycle and restart, GMCP hello, MSSP, config refusals;
GMCP and MSSP parsing; Latin-1; prompt tracker; NAWS over a socket; GMCP/MSSP session events;
TLS loopback with a generated certificate and an untrusted one refused; reconnect with backoff,
give up, user disconnect cancelling, manual reconnect; settings round trip, clamping, corrupt
file, saver coalescing, recent list; grid bounds, reflow, wide and combining characters, colours,
local echo, prompts, selection across scrollback, soft-wrap copy, selection surviving output,
line events and their bound; renderer runs and block elements; a real mouse drag selecting into
history; session tabs (prompt marks, quiet password prompt masking, manual reconnect); several
sessions keeping output and activity apart; the pacer; bundled font coverage.

## Known limitations

- **Text**: no shaping (Arabic, Indic), no right-to-left reordering, no colour emoji, ZWJ
  sequences shown as parts. Characters from fallback fonts are centred in their cells and can
  look narrow or overhang. Italic is egui's slant, not a real italic face.
- **Memory per session**: about 3 KB per 120-column row (24-byte cells at full width), so 50,000
  rows at 120 columns would be about 140 MB; the default is 2,000 rows. The grid counts rows, not
  lines, so a narrower window keeps fewer lines.
- **Selection** is cleared when the column count changes (alacritty), and copy uses Cmd/Ctrl+C
  only when the input line has no selected text. No right-click menu yet.
- **Terminal queries** (cursor position reports, device attributes) are answered by alacritty into
  an event that is dropped, so the server gets no reply. Window titles (OSC) are ignored.
- **Graphics memory**: about 160 MB appears whenever frames are drawn, as in M1.
- **IME, screen readers and Linux/Windows**: untested (as in M1). The grid is painted, so a
  screen reader sees nothing of the transcript; that needs AccessKit work.
- **Browser build**: `wandur-core` compiles for wasm32 without TLS; `wandur-term` does not,
  because `alacritty_terminal` always builds its PTY event loop (`polling`).
- **Not tried by a person**: drag selection, docking moves, settings edits; they are covered by
  headless tests and screenshots only.

## Performance and comparison

From `docs/measurements.md` (medians):

| | M2 | M1 | C# |
|---|---|---|---|
| CPU, steady 1 MB/s, 1 session | 18.8% | 42.4% | 37.6% |
| CPU, steady 4 x 250 KB/s | 19.7% | 40.6% | 76.0% |
| CPU, harness flood-1m-nochat (bursty, ticker) | 17.7% | 21.1% | 29.6% |
| Alloc, steady 1 MB/s | 60 MB/s | 252 MB/s | 84 MB/s |
| Idle CPU, 1 session / 8 sessions (no ticker) | 1.6% / 2.0% | 2.1% / 2.4% | 6.5% / 8.3% |
| Startup to first frame | 202 to 308 ms | 185 to 333 ms | about 1,100 to 1,200 ms |
| Working set, 4 sessions flooding | 107 to 112 MB | 90 MB | 262 to 313 MB |
| Heap, 4 sessions | 31.7 MB | 4.9 MB | 114 to 212 MB (GC) |
| Terminal model, 2,000 rows, 120 columns | 8.0 MB counted (6 MB touched) | 328 KB | about 6.9 MB |
| Footprint idle / one session idle / 1 MB/s | 70 / 226 / 240 MB | 66 / 230 / 231 MB | 224 / 363 / 514 MB |
| Keeps up at 1 MB/s and 4 sessions | yes | yes | yes |

## Verdict

**Better** on CPU under output (about half of M1 and of C# under a steady stream), allocation rate
and idle CPU; **equal** on throughput and startup; **worse than M1 but better than C#** on memory
per session, the price of a real terminal grid; **better than M1** on text: real bold, CJK and
symbols, selection across the scrollback. egui remains suitable; the open text gaps (shaping,
right-to-left, colour emoji, accessibility of a painted grid) are egui-wide limits worth weighing
before committing to it for good.

**Recommendation: continue to Milestone 3** (workspace, saved worlds UI, directory, panels), with
a person trying selection, docking and settings early, and with the accessibility question for the
painted transcript kept on the list.

## Try it

```sh
cd "/Volumes/Extreme SSD/workspace/wandur/wandur-client-rust"
cargo build --release

# The Unicode test page
./target/release/wandur-bench mud-server --port 4401 --rate 0 --page unicode &
./target/release/wandur 127.0.0.1:4401 --data-dir /tmp/wandur-try

# A steady flood (1 MB/s in 500 writes a second) and a bursty one
./target/release/wandur-bench mud-server --port 4402 --rate 1000000 --write-ms 2 &
./target/release/wandur-bench mud-server --port 4403 --rate 3000 &
./target/release/wandur 127.0.0.1:4402 127.0.0.1:4403 --data-dir /tmp/wandur-try
```

In the client: drag to select (past the top edge to scroll back), double click a word, triple
click a line, Cmd+C to copy; PageUp/PageDown; Save world in a session's status line, then use it
from the Sessions panel; Settings (top right) for scrollback, font size, output frames a second
and automatic reconnect. Stop a server (`kill %1`) to see the reconnect notices. Leave out
`--data-dir` to keep settings in `~/Library/Application Support/Wandur-Rust`.
