# Verdict: the Rust rebuild experiment

2026-10-08, after four milestones. For the owner. All numbers are measured on this machine
(Apple M3 Pro, macOS 26.5) against the C# client the same day, through the C# client's own
harness; details in `docs/measurements.md`.

## The short answer

The Rust engine is a better foundation than the C# client's on every measured axis but one. The
egui user interface is cheaper to run but dearer to build, and three things a real release needs
are unproven: screen readers, input methods, and Windows and Linux. The prototype does not
justify replacing the C# client now. It does justify keeping it as a reference, and several of
its lessons can go into the C# client without a rewrite.

## Engine (sessions, protocols, terminal model, directory): better

| Measured | Rust | C# |
|---|---:|---:|
| Startup to first frame | 0.25 to 0.37 s | 1.0 to 1.1 s |
| Working set, one session at 1 MB/s / four sessions | 91 / 108 MB | 225 / 318 MB |
| Heap, same | 10.5 / 31 MB | 97 / 167 MB (GC heap) |
| CPU, steady 1 MB/s / 4 x 250 KB/s | 15.7 / 19.1% | 35.3 / 70.4% |
| Idle session CPU (no forced frames) | 1.8% | 7.5% |
| ANSI parsing, 100,000 lines | 47 ms, 8 MB allocated | 55 ms, 42 MB |
| Directory filter / sort, 500 worlds | 0.02 ms | 0.6 to 2.3 ms |
| PNG plate peak memory | +2.8 MB | +68 MB |
| JPEG plate decode | 27.8 ms | 15.8 ms (worse) |

The core (telnet, GMCP, MSSP, TLS, reconnect, prompts, settings, directory, channels) is plain
Rust with no UI dependency and 80 tests; it also compiles for the browser target. The terminal
model is `alacritty_terminal`, which gave full xterm behaviour, reflow, selection and bounded
scrollback for free, at about 6 MB resident per session (C#: about 6.9 MB).

## UI build effort: worse

- Immediate mode is fast to run and simple for lists, panels and the terminal, but rich layouts
  (cards with measured columns, flowing chips, "one line if it fits") are hand arithmetic; some
  C# polish was approximated (Milestone 3).
- Missing pieces had to be written: the terminal renderer (twice: runs of galleys in M2, a glyph
  mesh in M4), layout persistence (our own description replayed into egui_dock), pin and
  auto-hide (about 400 lines with tests), artwork cropping and texture budgeting.
- Performance needs care in egui too: under a flood, laying out the visible text every frame
  cost 1.6 ms and 1.3 MB per frame until the glyph mesh; GPU textures dominated the footprint
  until budgeted (366 to 264 MB in the directory).
- Effort so far: four milestones for a client that covers sessions, the terminal, the workspace,
  saved worlds, the directory, channels and an initial map, but not the C# client's breadth.

## Gaps to parity, with rough sizes

Sizes are for one experienced developer, in focused weeks, and assume the C# behaviour as the
specification. They are estimates, not measurements.

| Gap | Size |
|---|---|
| Full mapper (text-only room tracking, areas, search, walking, editing, saved maps) | 4 to 6 weeks |
| Scripting (engine choice, sandbox, script panels, packs) | 4 to 8 weeks |
| Macros: triggers, aliases, timers, key bindings | 2 to 3 weeks |
| Pattern-based channel capture with the teaching dialog | 1 to 2 weeks |
| Credentials (OS keychain) and auto-login | 1 week |
| Session history with full-text search | 1 to 2 weeks |
| Room terrain classification (ONNX) | 1 to 2 weeks |
| Skins (nine-slice art, bezels), world themes, custom themes | 2 to 4 weeks |
| Localization (five languages) | 1 to 2 weeks |
| Update check, links in output, protocol diagnostics, transcript export UI, demo world | 1 to 2 weeks together |
| Screen reader support verified and fixed on three platforms | 2 to 4 weeks, uncertain |
| Windows and Linux builds, packaging, signing | 2 to 3 weeks |
| Agent (local LLM) | 2 to 4 weeks |

In total roughly 6 to 10 months to reach the C# client's current feature set, while the C#
client keeps moving.

## Risks

- **Accessibility**: the transcript is painted, so screen readers see only the AccessKit nodes we
  build by hand (a text node and a live region). egui's AccessKit support is thin for custom
  widgets, and nothing has been tried with VoiceOver, NVDA or Orca. Avalonia's automation is more
  mature. This is the biggest unknown.
- **IME**: egui's text fields support input methods, but CJK composition has not been tried.
- **Windows and Linux**: everything chosen is cross-platform (eframe, wgpu, rustls, fontdb), but
  nothing has been built or run there; fonts, fallback, DPI and window behaviour need a person.
- **Browser build**: the core compiles for wasm32, but `alacritty_terminal` does not (its PTY
  event loop pulls in `polling`). A browser client would need an upstream feature, a fork, or
  another grid.
- **GPU memory**: about 160 MB of the footprint appears whenever frames are drawn (wgpu and Metal
  surfaces); it is outside the app's control and equals C#'s idle footprint.
- **Text**: no complex shaping in the grid, no right-to-left, no colour emoji (egui limits).
- **Dependencies on young crates**: egui and egui_dock change APIs often (0.x versions).

## Lessons for the C# client, without a rewrite

Each with the evidence that motivates it.

1. **Budget textures by bytes.** GPU textures count in the process footprint. The Rust directory
   went from 366 to 264 MB by lowering the texture budget from 96 to 24 MB, below C#'s 301 MB.
   Cap the C# thumbnail cache by bytes of decoded pixels, not by count.
2. **Normalize searchable text once, when the catalog loads.** Filtering 500 worlds takes 0.023 ms
   in Rust against 2.256 ms in C#, because each world's text is folded once, not per query.
3. **Decode PNG row by row into the reduced size.** Peak +2.8 MB against +68 MB for the same
   plate; the full bitmap never exists. For JPEG, keep libjpeg-turbo's scaled decode (C# is faster
   there) and crop before the final resize (halved the Rust peak, 13.8 to 7.8 MB).
4. **Pace redraws by output frames, not by a timer, and wake only when a frame is due.** The M2
   change (30 output frames a second, no wake-ups between) halved Rust CPU under a steady stream
   (42 to 19%); C# drains output on a 60 ms UI timer and idles at 7.5% CPU
   with one session against 1.8%.
5. **Do not lay text out again when only its position changed.** Drawing the terminal's plain
   text from cached glyphs instead of laying out each visible run cut the frame from 1.6 ms and
   1.3 MB to 0.2 ms and 11 KB. In Avalonia, cache `GlyphRun`s or formatted lines per row content
   and reuse them as rows scroll.
6. **A steady caret.** Blinking cost about ten frames a second when idle (M2).
7. **Keep startup work off the UI thread.** Listing the thumbnail cache at start cost up to 14 ms
   on the UI thread; the C# client starts in about a second, so look there for synchronous file
   and database work at launch.
8. **Measure per panel.** A per-panel time and allocation breakdown (the Rust shell bench) showed
   the expected culprit (idle side panels) was 4% of a frame and the real one (text layout) was
   half; the same instrument in the C# perf probe would point optimizations at the right place.

## Recommendation

Keep the Rust prototype as a reference and a benchmark, and stop feature work on it unless the
owner wants a long-term replacement and accepts 6 to 10 months to parity plus the accessibility
and platform risks above. In the near term, apply lessons 1 to 5 to the C# client, where the
measured gains are largest.
