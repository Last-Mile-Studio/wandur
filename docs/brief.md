# Owner's brief (verbatim, 2026-10-08)

I want to rebuild the Wandur desktop client as an exploratory Rust application.

This is a parallel prototype, not an immediate replacement for the existing C# applications. The experiment may ultimately be abandoned. Preserve the existing implementations as reference projects and do not destabilize them.

Existing desktop client: /Volumes/Extreme SSD/workspace/wandur/wandur-client
Rust rebuild: /Volumes/Extreme SSD/workspace/wandur/wandur-client-rust
The web application (wandur-site) stays C# and is out of scope.

Boundaries:
- Do not rewrite, delete, or destabilize `wandur-client`. Do not modify the web application. Do not move shared projects.
- Do not mix Rust source into the existing C# desktop project. Treat the C# projects as read-only references.
- All Rust implementation under /Volumes/Extreme SSD/workspace/wandur/wandur-client-rust.
- You may inspect other projects for domain models, APIs, behavior and shared concepts. Do not modify unrelated siblings.
- Preserve any uncommitted changes in other repositories.

Target technology: Rust, egui, eframe, egui_dock; native desktop; no webview/Electron; cross-platform where practical.

Primary goal: determine whether a Rust implementation can provide a cleaner architecture, lower memory usage, better sustained terminal performance and a more maintainable foundation than the C# Avalonia implementation. Not a line-by-line translation: an idiomatic Rust application based on the existing client's behavior.

Before substantial code:
1. Inspect the existing repository and documentation.
2. Identify the major features and current architecture.
3. Feature-parity table: Feature | Existing C# implementation | Planned Rust implementation | Complexity | Current status.
4. Baseline of the existing desktop app where practical: startup time; idle memory; memory with one connected session; sustained high-volume output; multiple sessions; world directory with artwork.
5. A short Rust architecture proposal before the major subsystems.
Do not spend long designing without a runnable vertical slice.

Initial architecture: core state separated from UI; telnet/session layer; ANSI parser and terminal buffer; output/event pipeline; history and persistence; world directory/discovery; settings; workspace and docking state; egui presentation layer; optional features behind clear interfaces or feature flags. Clear module boundaries; no giant state object or giant UI file.

Milestone 1 (proof of concept): native window; main dark workspace shell; egui_dock basic dockable layout; terminal panel; connect to a configured or test MUD endpoint; receive and render text; keyboard input; scrolling; bounded scrollback; connection status. Validates egui/eframe suitability, egui_dock behavior, terminal rendering approach, keyboard and text input, responsiveness, cross-platform feasibility. No full parity before this works.

Milestone 2 (core client): TCP lifecycle; telnet negotiation; ANSI parsing; ANSI colors and attributes; prompt handling; input history; reconnect; bounded scrollback; session state; multiple sessions; basic settings persistence; output batching; session activity indicators. Terminal buffer incremental and bounded; never rebuild the full transcript per frame or output event.

Milestone 3 (workspace and UI): workspace shell; dockable panels; saved worlds; world directory/browser; terminal transcript; channels/communication panels; map panel or clearly marked initial map; settings; connection/session status; activity indicators; theme support; workspace layout persistence (egui_dock). Match behavior and information hierarchy first, visual polish later.

Milestone 4 (performance):
1. No full transcript reconstruction for activity detection (sequence numbers, dirty flags, incremental state).
2. Bounded scrollback, configurable, guarded against unbounded growth; old lines and formatting released.
3. One canonical incremental terminal model for parsing, activity, transcript extraction and rendering unless profiling shows a second is needed.
4. No needless per-character allocations; profile before pooling/unsafe/complex abstractions; simple reusable buffers and capacity hints where they demonstrably help.
5. Decode thumbnails near target size; avoid loading large sources when only small cards are visible.
6. Lazy-load optional heavy features (scripting, ML, tokenizers, advanced processing).
7. Bounded, cancellable image loading; only visible/relevant entries; deterministic disposal.
8. Batch output updates; coalesce while keeping input/output responsive.
9. Background work off the UI thread (history writes, artwork, indexing, expensive parsing).
10. Measure before and after each significant optimization; drop optimizations that neither improve a benchmark nor simplify the architecture.

Prioritize first: connection and session management; terminal I/O; ANSI parsing and colors; bounded scrollback; multiple sessions; dockable workspace; saved worlds; basic settings; world directory; basic persistence.
Defer or isolate: ML classification; advanced scripting; complex map; advanced text editing; nonessential integrations; cosmetic parity; rare automation. Deferred features get clear interfaces or feature flags and are documented as missing, never silently removed.

Tests: ANSI parsing; telnet negotiation; scrollback limits; prompt handling; session lifecycle; reconnect; persistence; workspace layout persistence; image loading and cancellation; directory filtering and sorting; multiple-session output; activity indicators.
Run and report: cargo fmt --check; cargo clippy; cargo test; release builds; release benchmarks where available.
Performance tests: ANSI parsing throughput; appending 100,000 lines; rendering 2,000 lines of scrollback; multiple concurrent sessions; directory filtering and sorting; loading many large artwork files; startup time; idle memory; sustained output. Compare with the C# client where practical.

End of each milestone: what was implemented; what remains; tests run; known limitations; performance measurements; comparison with C#; whether Rust looks better, worse or inconclusive; whether to continue.

Decision rule: do not assume Rust is faster or leaner. If egui, egui_dock, terminal rendering or cross-platform behavior proves unsuitable, document it clearly. If Rust is not materially better, say so honestly. No months chasing parity; always keep a runnable app and the ability to stop cleanly. Document assumptions in the Rust project. Do not modify the C# client or web app to accommodate the prototype.
