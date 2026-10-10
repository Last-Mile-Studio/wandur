# Inventory of the existing C# client

Source: `wandur-client` at `27f66d2` (read only), its `CLAUDE_HANDOFF.md`, `README.md`,
`docs/client-architecture.md`, `docs/perf.md`, and the code under `src/` and `bench/`.
Shared models and the telnet parser come from `wandur-sdk` at `3d22f67`.

## Stack

.NET 10, Avalonia 12 (code-first views, CommunityToolkit.Mvvm), Dock for Avalonia (docking),
Iciclecreek.Avalonia.Terminal with XTerm.NET (terminal display), AvaloniaEdit (diagnostics editors),
Microsoft.Data.Sqlite (one `wandur.db`), Jint (scripts, in a worker process), ONNX Runtime and a
tokenizer (room terrain classifier), Microsoft.Extensions.DependencyInjection. About 30,000 lines of
C# in `Wandur.Core` (no GUI) and `Wandur.Desktop` (Avalonia), plus the SDK projects.

## Architecture in one paragraph

`App` is the composition root (DI). `MainWindow` owns native window, menus and the Dock layout.
`SessionWorkspace` owns one `WorkspaceController` per session tab. Each controller owns an
`IMudSession` (`TelnetSession` or the offline demo). `TelnetSession` reads the socket on a
thread-pool task, feeds bytes to the SDK `TelnetParser` (negotiation, GMCP, MSDP, MSSP, prompt
marks), decodes text (UTF-8 or Latin-1) and raises events. The controller enqueues received text
into a bounded queue (512 Ki characters or 2,048 chunks; on overflow it drops everything and shows a
notice) and a 60 ms `DispatcherTimer` flushes it on the UI thread into two models: the Core
`AnsiTerminal` (line model with styled runs, 500 lines, used for prompts, channels, completion,
history and agents) and the display's xterm buffer (2,000 rows plus viewport, used for rendering,
selection and export). Persistence is SQLite through stores injected into the controllers;
passwords live in the OS credential vault.

## Major features

| Area | What exists | Where |
|---|---|---|
| Connections | TCP and TLS, 15 s connect timeout, smart address entry (`host:port`, `host port`, `telnet://`, bracketed IPv6), UTF-8 or Latin-1, explicit connect only, reconnect by opening a new session | `Core/Sessions/TelnetSession.cs`, profile editor |
| Telnet | ECHO, SGA, TTYPE/MTTS, NAWS (real grid, debounced), EOR and GA prompt marks, GMCP, MSDP, MSSP; no MCCP, CHARSET or MXP | `wandur-sdk/Wandur.Protocol/TelnetParser.cs` |
| Terminal display | xterm emulation, 16/256/truecolor, bold, italic, underline, optional blink, cursor positioning, erase, 2,000 row scrollback, selection and copy, live tail split while scrolled up, Ctrl/Cmd+click links with a safety check | `Desktop/Terminal/*` |
| Line model | Bounded ANSI line model (styled runs, 4,096 chars per line, 200 K chars), local echo, prompt detection | `Core/Terminal/AnsiTerminal.cs` |
| Input | Command box, history with draft restore, private (masked) input from server echo and password prompts, inline completion (trie), Look and Commands buttons, F-key macros | `Core/Input`, `Desktop/Views/TerminalView` |
| Sessions | Many sessions, each with its own transcript, history, draft, scripts and privacy; rename; activity indicator; session character label | `SessionWorkspace`, `WorkspaceController` |
| Workspace | Dockable, floating, closable panels (Workspace sidebar, Map, Channels, Diagnostics); View > Restore Panels; three window skins and ten colour themes; world themes; layout not saved across restarts | `MainWindow`, `WorkspaceFactory`, `ThemeService` |
| Saved worlds | Profiles in SQLite, recency-weighted ordering, thumbnails, credentials in the OS vault, auto-login (GMCP `Char.Login` or prompt regex) | `Core/Settings`, `Core/Storage`, `Desktop/Security` |
| Directory | Find a MUD: wandur.net directory API (`/directory`), cached snapshot, filters, sort, artwork rows decoded near target size, Explore details, add to my worlds | `Core/Discovery`, `WorldBrowserViewModel` |
| Channels | Rule-based and GMCP channel recognition mirrored into a Channels panel with reply box, teaching dialog | `Core/Channels` |
| Mapping | Room tracker from GMCP/MSDP/text, map panel with floors, search, walking, room terrain inference (ONNX) | `Core/Mapping`, `Core/Classification` |
| Scripting | Jint scripts in one worker process per session, script panels, packs from the directory, macros (triggers, aliases, timers, keys) | `Core/Scripting`, `Desktop/Services` |
| Agent | Local LLM agent (LM Studio or OpenAI compatible) with goals and Preview/Step/Run | `Core/Agents` |
| History | Local session history in SQLite FTS5 with retention, read-only browser | `Core/History` |
| Diagnostics | Protocol messages with filter chips, observed fields, raw console with privacy masking, server details (MSSP) | `Core/Diagnostics`, Diagnostics views |
| Settings | Client preferences, appearance, terminal, input; five UI languages | `Core/Settings`, `Core/Localization` |
| Updates | Daily update check against `/client/latest`, anonymous install id | `Core/Updates` |
| Demo | Five-room offline demo world | `Core/Sessions/DemoSession.cs` |
| Measurement | Opt-in probe (`WANDUR_PERF_PROBE`), `bench/Wandur.Bench` harness with a loopback MUD server and fake directory | `Desktop/PerfProbe.cs`, `bench/` |

## Behaviour worth keeping in the rebuild

- Received text is batched off the network thread and applied on a UI timer, never per byte.
- Bounded everything: pending output, scrollback lines, characters per line, styles cache.
- Server echo off (WILL ECHO) means private input: masked, not echoed, not recorded.
- Prompts without a newline are shown at once (partial line), and GA/EOR mark them.
- Bold plus a basic colour (30 to 37) means the bright palette entry, resolved after the whole SGR.
- `CSI K` erases in line, `CSI 2J`/`3J` clear, other CSI sequences are consumed; OSC is consumed.
- A disconnected session keeps its transcript until closed; reconnect opens a new connection.

## Known pain points recorded in the C# docs

- Footprint of 470 to 530 MB under load is mostly native (Skia, Metal, runtime).
- Two terminal models per session (line model plus a 5.1 MB xterm buffer at 2,000 rows).
- Idle CPU of 6 to 8% with a session open, attributed to rendering (caret or animation).
- Earlier: transcript rebuilds for activity checks and per-row Channels updates (fixed).
