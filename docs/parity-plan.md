# Parity plan: Rust client to the C# client's feature set

2026-10-08. The owner decided to build the Rust client to feature parity with the C# client,
verify it with screenshots against the C# client, tweak the UI afterwards, and possibly throw it
away. This plan turns the gaps in `docs/parity.md` and `docs/verdict.md` into 16 ordered tasks,
each sized for one capable agent in one to two hours.

## Parity target

- The C# client at `wandur-client` `main` (27f66d2), plus the unmerged `feature/mudlet-import`
  branch (Lua scripts and Mudlet import), because the run brief asks for them. Read that branch
  with `git -C <wandur-client> show feature/mudlet-import:<path>` or export it with
  `git archive feature/mudlet-import` into `.superpowers/`; never check it out in place.
- Not in the target: the unmerged `feature/assistant-helper` branch (help lookups), the parked
  agent redesign and the parked help search idea. They can be added later as their own tasks.
- The directory site and API stay out of scope.

## Rulings

- **Ruling: JavaScript first, Lua second.** The C# client's script language is JavaScript (Jint).
  Macros compile to it, directory script packs ship it, and every help example uses it. Lua is
  an opt-in second language on an unmerged branch. A Lua-only Rust client would not run a single
  pack or published example, so it would not be parity. JavaScript goes in through `rquickjs`
  (MIT, QuickJS) with an interrupt handler and a memory limit for the 300 ms and 64 MiB limits;
  Lua through `mlua` (MIT, vendored Lua 5.4) on the same host API. Cost if wrong: two engines to
  maintain; dropping Lua later is one feature flag.
- **Ruling: macros run natively in core, not as generated scripts.** C# compiles each macro to
  JavaScript. Rust keeps the same `MacroDefinition` data (kind, pattern, match, ignore case,
  commands, interval) and runs it in a core rule engine, so triggers, aliases, timers and keys
  work without a script engine and cost no JavaScript per line. Cost if wrong: a pattern edge case
  behaves differently from Jint's regex; the ported `MacroTests` cases guard it.
- **Ruling: one SQLite database, `wandur.db`, beside `settings.json`.** History needs FTS5; maps,
  scripts, macros, agent profiles and channel rules are relational and per world. `rusqlite` with
  the `bundled` feature (MIT, SQLite public domain, FTS5 compiled in) on one writer thread.
  `settings.json` and `layout.json` stay as they are. Cost if wrong: migrating stores later.
- **Ruling: in-process script threads, not a worker process.** C# runs one worker process per
  session because Jint could not be stopped reliably; QuickJS and Lua both have interrupt hooks
  and memory limits, so one script thread per session is enough. A panic in a script fails that
  session's scripts only. Cost if wrong: a native crash in an engine takes the app down.
- **Ruling: room classifier through `tract-onnx`** (MIT/Apache, pure Rust) behind the
  `classifier` cargo feature, loaded lazily from a local package, never downloaded by default.
  `ort` would download ONNX Runtime binaries at build time, which this run forbids. Cost if wrong:
  tract may lack an operator the encoder uses; t13 checks that first and records the result.
- **Ruling: localization infrastructure early (t02), not last.** The C# strings live in five
  `.resx` files the owner owns; they are converted once into a generated Rust table keyed by the
  C# resource keys, and every later task adds its UI text through that table. Doing it last
  would mean retrofitting every string written in t03 to t16.
- **Ruling: crate downloads.** New crates (`rusqlite`, `rquickjs`, `mlua`, `keyring`,
  `quick-xml` or `roxmltree`, `regex`, `fancy-regex`, `tract-onnx`, `pulldown-cmark`, `rfd`,
  `arboard`, `semver`, `uuid`) come from the crates.io registry through cargo, the same way the
  earlier milestones built; that is build tooling, not app traffic. The app and its tests talk
  only to loopback. t01 adds the shared ones in one go so later tasks rarely fetch.
- **Ruling: screenshots by scene, not by hand.** t01 adds `--scene <name>` so every screen in
  this plan is reproducible headless against the loopback bench, and the reference agent captures
  the same names from the C# capture tests. UI matches the C# layout, controls and wording; the
  owner tweaks looks afterwards, so pixel matching is not the goal.

## Rules every task keeps

- Work only in `wandur-client-rust`; the C# client, SDK and site are read-only. Run C# code only
  from `.superpowers/csharp-ref`.
- Loopback only at run time: `wandur-bench` MUD, directory and (new) fake model servers.
- `wandur-core` has no UI dependency. Heavy optional parts sit behind cargo features.
- Every task adds tests, keeps the app runnable, appends to `.superpowers/progress.md`, and
  passes `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` and
  `cargo build --release`. UI tasks save their screens under `.superpowers/shots/parity/<id>/`
  and compare them with `.superpowers/shots/reference/`.
- Performance: rerun the shell bench and the session scenarios after t01, t09, t12 and t15 and
  compare with `docs/measurements.md`; a regression over 20% needs a note and a fix or a ruling.

## Order and dependencies

| Id | Title | Depends on |
|---|---|---|
| t01 | Foundations: client database, world identity, scene capture | none |
| t02 | Localization, menu bar, Help, About and the offline demo world | t01 |
| t03 | Macros: triggers, aliases, timers and key shortcuts | t01, t02 |
| t04 | Login: password vault, auto-login, private input, session character | t01, t02 |
| t05 | Composer and transcript parity | t02 |
| t06 | Protocols: MSDP, state cache, diagnostics, server details, mapped vitals | t01, t02 |
| t07 | Session history with full-text search | t01, t04 |
| t08 | Channels parity: rule sets, teaching dialog, world rules | t01, t02 |
| t09 | JavaScript scripting: engine, `mud` API, library, editor | t03, t06 |
| t10 | Script panels, the session rail, bars and script packs | t09 |
| t11 | Lua, the Mudlet layer and Mudlet profile import | t03, t09 |
| t12 | Mapper: tracking, saved maps, search, routes and walking | t01, t06 |
| t13 | Map editor, map import and export, room terrain inference | t12 |
| t14 | Local model agents | t04, t08 |
| t15 | Skins with title bars, world themes, custom themes | t02 |
| t16 | Settings sweep, update check, install id, accessibility, packaging | all |

## Tasks

### t01 Foundations: client database, world identity, scene capture

Goal: the shared pieces later tasks build on. A `wandur.db` (rusqlite, bundled, WAL, foreign
keys, versioned migrations in one transaction, one writer thread, short read connections), a
stable world id that survives endpoint edits (as `ClientDatabase` and profile resolution do), the
shared new dependencies, and a scene runner: `wandur --data-dir D --scene NAME` sets up a named
screen (open panels, dialogs, a loopback session from `wandur-bench`) and with
`WANDUR_SCREENSHOT` saves it as PNG, so every later screen is one command.

Acceptance:
- `wandur.db` is created in the data dir; schema version recorded; a migration from an empty and
  from a version 1 file is tested; a corrupt file is moved aside and the app still starts.
- Saved worlds get a stable world id; editing host or port keeps the id; tests cover it.
- The writer thread batches writes and never runs on the UI thread (test with a slow fake).
- `--scene` supports at least `main-hull-dark`, `main-hull-light`, `directory`, `world-details`,
  `settings-general`, `session-play`, each producing a PNG headless, listed by `--scene list`.
- `scripts/capture-scenes.sh` captures every scene into a given folder against a bench server it
  starts and stops.
- Shell bench and session numbers within 10% of `docs/measurements.md`.

C# refs: `src/Wandur.Core/Storage/ClientDatabase.cs`, `src/Wandur.Core/Settings/ClientSettings.cs`,
`src/Wandur.Core/Settings/WorldAddress.cs`, `src/Wandur.Core/Storage/WorldUsageStore.cs`,
`docs/client-architecture.md` (Local persistence), `tests/Wandur.Desktop.Tests/HelpCapture.cs`,
`tests/Wandur.Desktop.Tests/LanternRoadSession.cs`.

Screens: `main-hull-dark`, `main-hull-light`, `session-play`.

### t02 Localization, menu bar, Help, About and the offline demo world

Goal: every UI string through a generated table from the C# `.resx` files (English, Spanish,
French, German, Brazilian Portuguese), a language choice in Settings > General with live switch
and system fallback, the C# menu structure (File, Edit, View, Session, Scripts, Window, Help;
macOS app menu), About, Help links behind the "Open this link?" confirmation, Save transcript and
Clear transcript, and the offline five-room demo world (File > Open offline demo).

Acceptance:
- A generator (`scripts/generate-localization.py` or a build step) turns the five `.resx` files
  into Rust; a check mode fails on drift; tests assert key and format-argument parity across
  languages.
- Switching language in Settings changes visible labels without reconnecting; Cancel restores.
- Menus list the C# items in C# order; items whose feature lands later are present and disabled.
- Save transcript writes the grid's transcript to a file chosen in a native dialog; Clear empties
  the grid; both tested in core or term.
- The demo world plays offline with the C# room text and commands; test drives it to every room.
- Screens captured in English and German and compared with the reference.

C# refs: `src/Wandur.Core/Localization/Strings*.resx`, `src/Wandur.Core/Localization/UiLanguage.cs`,
`scripts/generate-localization.py`, `src/Wandur.Desktop/DesktopMenus.cs`, `src/Wandur.Desktop/App.axaml`,
`src/Wandur.Core/Sessions/DemoSession.cs`, `src/Wandur.Core/Terminal/WebLinks.cs`,
`tests/Wandur.Core.Tests/LocalizationTests.cs`, `tests/Wandur.Desktop.Tests/HelpMenuTests.cs`.

Screens: `menu-file`, `menu-view`, `menu-help`, `about`, `settings-general`, `settings-general-de`,
`demo-session`.

### t03 Macros: triggers, aliases, timers and key shortcuts

Goal: the C# no-code macros in a core rule engine fed by the terminal's line events and the send
path: triggers (contains, starts with, exact, ignore case), aliases (whole command), timers
(1 to 86,400 s) and F1 to F12 shortcuts, each sending up to 20 command lines. Stored per world in
`wandur.db`. The world editor gets its section list (Connection, Login, Scripts, Macros, Agent,
Channels, with later sections disabled), the Macros section with the C# list and form, and the
session footer gets the Macros toggle for this session only.

Acceptance:
- Port the cases from `MacroTests` and `MacroStorageTests` as Rust tests (validation limits,
  escaping, matching, timers, shortcuts).
- Triggers see public lines only; nothing fires on private or login lines (test).
- An alias replaces the command; history and local echo follow the C# behaviour (test).
- Timers stop on disconnect and on the session toggle; a loopback test sends from a timer.
- Footer Macros toggle and world editor Macros section match the reference screens.
- Rule matching for 1,000 rules over a 1 MB/s flood measured; frame time within 10%.

C# refs: `src/Wandur.Core/Scripting/MacroDefinition.cs`, `src/Wandur.Desktop/Views/MacroLibraryView.cs`,
`src/Wandur.Desktop/ViewModels/MacroLibraryViewModel.cs`, `src/Wandur.Desktop/Views/SessionAutomationToolbar.cs`,
`src/Wandur.Desktop/ViewModels/SessionAutomationViewModel.cs`, `src/Wandur.Desktop/ViewModels/ProfileEditorViewModel.cs`,
`src/Wandur.Desktop/Views/ProfileDialog.axaml`, `docs/scripting.md` (Macros),
`tests/Wandur.Core.Tests/MacroTests.cs`, `tests/Wandur.Desktop.Tests/MacroLibraryTests.cs`.

Screens: `world-editor-connection`, `world-editor-macros`, `session-footer-macros`.

### t04 Login: password vault, auto-login, private input, session character

Goal: the world editor Login section (username, save password, auto-login, username and password
prompt patterns), an OS vault through `keyring` (macOS Keychain, Windows Credential Manager,
Linux Secret Service) keyed by world id, password reference, host, port, TLS and username, GMCP
`Char.Login` and the two-minute text prompt handshake, the padlock Private toggle in the session
bar, and the session character name in the tab, Workspace row, status bar and window title.

Acceptance:
- Port `AutoLoginTests` and `GmcpLoginTests` cases against an in-memory vault.
- A loopback bench login page: auto-login sends name and password once, never echoes or records
  them; a manual command cancels it; a rejected GMCP login stops without text retry (tests).
- Changing host or username drops the stored reference; changing port or TLS moves the saved password to the new key; renaming does not matter (tests).
- Vault failures show a readable error; no plaintext fallback anywhere.
- The macOS vault is exercised by one opt-in test with a temporary item, removed afterwards.
- Window title, tab and Workspace row show "World · Character" as in C#; Login section and the
  private toggle match the reference.

C# refs: `src/Wandur.Core/Sessions/AutoLoginSequence.cs`, `src/Wandur.Desktop/WorkspaceController.Login.cs`,
`src/Wandur.Desktop/Security/*.cs`, `src/Wandur.Desktop/Views/ProfileDialog.axaml`,
`docs/client-architecture.md` (Credentials and login), `tests/Wandur.Core.Tests/AutoLoginTests.cs`,
`tests/Wandur.Core.Tests/GmcpLoginTests.cs`, `tests/Wandur.Desktop.Tests/LoginTests.cs`,
`tests/Wandur.Desktop.Tests/SessionCharacterTests.cs`.

Screens: `world-editor-login`, `session-private`, `session-character-title`.

### t05 Composer and transcript parity

Goal: the remaining session view behaviour: inline completion (bounded trie of public lines and
sent commands, muted ghost, Tab or Right accepts, Escape hides, setting to turn it off), the live
tail split while scrolled up with a draggable divider and saved share, links (Ctrl+click, Cmd+click
on macOS, hover hint, "Open this link?", the `WebLinks` safety rules), Look and Commands buttons
beside the command box, the command hint, and Settings > MUD colors and Terminal (custom ANSI
palette, text and background override, text size with preview, blinking text option, local echo).

Acceptance:
- Port `CompletionTests` and `ComposerCompletionTests` cases; the trie stays bounded (test).
- Port the `WebLinks` safety cases (private hosts, user info, look-alike characters refused).
- The tail split appears only while scrolled up, keeps following the tail, and its share is saved
  (test on the view state).
- Custom ANSI palette and overrides apply at paint time without rebuilding the grid.
- Settings > MUD colors and Terminal and the composer with a ghost match the reference.
- Flood frame time within 10% with completion learning on.

C# refs: `src/Wandur.Core/Input/*.cs`, `src/Wandur.Desktop/Terminal/TranscriptTailPane.cs`,
`src/Wandur.Desktop/Terminal/MudTerminalSurface.cs`, `src/Wandur.Core/Terminal/WebLinks.cs`,
`src/Wandur.Core/Terminal/AnsiPalette.cs`, `src/Wandur.Desktop/Views/TerminalView.cs`,
`src/Wandur.Desktop/Views/OptionsDialog.axaml`, `tests/Wandur.Core.Tests/CompletionTests.cs`,
`tests/Wandur.Desktop.Tests/TranscriptTailTests.cs`, `tests/Wandur.Desktop.Tests/LinkClickTests.cs`.

Screens: `session-completion`, `session-tail-split`, `link-confirm`, `settings-mud-colors`,
`settings-terminal`, `settings-input`.

### t06 Protocols: MSDP, state cache, diagnostics, server details, mapped vitals

Goal: MSDP negotiation and decoding in core (REPORT, SEND, tables and arrays), a protocol state
cache of the latest GMCP and MSDP values gated by privacy, the protocol binding engine that maps
a directory listing's `mapping` to resource bars and the opponent card (the vitals strip under
the transcript), and the Diagnostics document (Messages with kind chips and filter, Observed
fields, Console with visible control characters, private markers and redaction) plus the Server
details tab from MSSP.

Acceptance:
- MSDP decoding and encoding tested against byte fixtures, including nested tables.
- Port `ProtocolBindingEngine` cases, including the two-fight and lingering-login fixes
  (`MappedVitalsLiveTests` variants as core tests).
- Console ring bounded at 2,000 chunks or 512 KiB; private intervals collapse to one marker;
  remembered secrets are masked (tests).
- Diagnostics filter chips and counts behave as `ProtocolDiagnosticsFilterTests`.
- `wandur-bench` gains an MSDP page; the vitals strip, Diagnostics tabs and Server details match
  the reference.

C# refs: `external/wandur-sdk/Wandur.Protocol` (MSDP, decoders), `src/Wandur.Core/Protocol/*.cs`,
`src/Wandur.Core/Scripting/ProtocolStateCache.cs`, `src/Wandur.Core/Diagnostics/ConsoleLog.cs`,
`src/Wandur.Desktop/WorkspaceController.Protocol.cs`, `src/Wandur.Desktop/WorkspaceController.Diagnostics.cs`,
`src/Wandur.Desktop/WorkspaceController.Console.cs`, `src/Wandur.Desktop/Views/ResourceBarsView.cs`,
`src/Wandur.Desktop/Views/ProtocolDiagnosticsView.cs`, `src/Wandur.Desktop/Views/ConsoleView.cs`,
`docs/protocol-diagnostics.md`, `docs/proposals/protocol-mappings.md`,
`tests/Wandur.Desktop.Tests/ResourceBarsTests.cs`, `tests/Wandur.Desktop.Tests/ServerDetailsTests.cs`.

Screens: `session-vitals`, `diagnostics-messages`, `diagnostics-observed`, `diagnostics-console`,
`server-details`.

### t07 Session history with full-text search

Goal: a bounded background recorder of completed public lines and sent commands into `wandur.db`
(sessions, entries, external-content FTS5), with the C# privacy rules, retention (30, 90, 365
days, forever), the recording notice with Don't show again, and View > Session history: a
non-modal window with Search and Sessions tabs, word and phrase search, world, character and date
filters, paging (50 results, 100 transcript lines) and Delete session with confirmation.

Acceptance:
- Port `HistoryRecorderTests` and `SessionHistoryStoreTests` cases: private and login intervals
  become markers, credential echoes are redacted even split across reads, lines at the length cap
  are dropped.
- Search semantics match C# (AND words, quoted phrase, operators literal, newest first).
- Retention removes expired entries at start, after a change and periodically (test with a clock).
- A full queue or a storage error pauses recording with a notice; the session stays connected.
- Recording at 1 MB/s adds under 5% CPU; nothing on the UI thread (measured).
- History window, settings rows and the notice match the reference.

C# refs: `src/Wandur.Core/History/*.cs`, `src/Wandur.Desktop/WorkspaceController.History.cs`,
`src/Wandur.Desktop/MainWindow.History.cs`, `src/Wandur.Desktop/Views/HistoryWindow.cs`,
`src/Wandur.Desktop/ViewModels/HistoryViewModel.cs`, `docs/session-history.md`,
`tests/Wandur.Core.Tests/HistoryRecorderTests.cs`, `tests/Wandur.Desktop.Tests/HistoryViewTests.cs`,
`tests/Wandur.Desktop.Tests/HistoryNoticeTests.cs`.

Screens: `history-search`, `history-sessions`, `history-notice`, `settings-general-history`.

### t08 Channels parity: rule sets, teaching dialog, world rules

Goal: pattern channel capture over line events with the C# family rule sets (`families.json`,
picked by the profile's codebase or MSSP CODEBASE), world rules and exclusions that run first,
Mark as channel... on a transcript line's context menu with the proposer and a preview over the
last 200 lines, the world editor Channels section (list, toggle, delete), and the Show channels
panel setting.

Acceptance:
- `families.json` reused (owner's asset, licence note kept); port `ChannelClassifierTests` and
  `ChannelRuleProposerTests`.
- World rules beat family rules; "Not a channel" beats both (test).
- GMCP channels keep working and are not duplicated by a pattern match (test).
- The teaching dialog previews matches before saving; saved rules apply at once (test).
- Classifying at 1 MB/s costs under 0.3 ms a frame (measured).
- Mark as channel dialog, Channels section and the panel with pattern channels match the reference.

C# refs: `src/Wandur.Core/Channels/*`, `src/Wandur.Desktop/WorkspaceController.Channels.cs`,
`src/Wandur.Desktop/Views/MarkChannelDialog.cs`, `src/Wandur.Desktop/ViewModels/MarkChannelViewModel.cs`,
`src/Wandur.Desktop/Views/ChannelRulesView.cs`, `src/Wandur.Desktop/Views/ChannelsView.cs`,
`docs/client-architecture.md` (Channels), `tests/Wandur.Desktop.Tests/TeachChannelTests.cs`,
`tests/Wandur.Desktop.Tests/ChannelPanelTests.cs`.

Screens: `channels-panel`, `mark-channel-dialog`, `world-editor-channels`.

### t09 JavaScript scripting: engine, `mud` API, library, editor

Goal: per-world scripts in JavaScript through `rquickjs` behind the `scripting` feature (on by
default), one script thread per session, one context per script, the C# host API from
`docs/scripting-reference.json` (send, echo, trigger, alias, every, after, on with `Events.*`
including Prompt, Gmcp, Msdp and Key, remove, `mud.state.get`, `snapshot`, `format`), the C#
limits (300 ms per callback including regexes, 100,000 statements equivalent, 64 MiB), the
restart policy, the script library per world in `wandur.db`, the Scripts footer menu with a
checkbox per script, and the script editor (code view with JavaScript colouring, completion from
the reference, Save, Run).

Acceptance:
- The reference file drives a test that every global, event and limit in it exists in the engine.
- Port `ScriptRuntimeTests` and the script session cases: a runaway loop fails only its script;
  a script sees no private input; the state seed arrives before load.
- The three help examples in `HelpScreenshotTests` (Lantern watch minus panels, Guild greeter,
  Marsh warning) run against the bench and do what they say (loopback test).
- Script start cost and a trigger over a 1 MB/s flood measured; frame time within 15%.
- Scripts menu, script library and editor with completion match the reference.

C# refs: `src/Wandur.Core/Scripting/JavaScriptEngine.cs`, `src/Wandur.Core/Scripting/ScriptContracts.cs`,
`src/Wandur.Core/Scripting/ScriptWorker.cs`, `src/Wandur.Core/Scripting/ScriptLineBuffer.cs`,
`src/Wandur.Core/Scripting/ScriptExamples.cs`, `src/Wandur.Core/Scripting/WorldScriptStore.cs`,
`src/Wandur.Desktop/Services/SessionScripts.cs`, `src/Wandur.Desktop/Services/SessionScriptWorker.cs`,
`src/Wandur.Desktop/Views/ScriptLibraryView.cs`, `src/Wandur.Desktop/Views/ScriptCodeEditor.cs`,
`src/Wandur.Desktop/Views/ScriptCompletionCatalog.cs`, `src/Wandur.Desktop/Views/SessionScriptsButton.cs`,
`docs/scripting.md`, `docs/scripting-reference.json`, `tests/Wandur.Desktop.Tests/ScriptSessionTests.cs`.

Screens: `scripts-menu`, `script-library`, `script-editor`, `script-editor-completion`.

### t10 Script panels, the session rail, bars and script packs

Goal: `mud.panel(id, {title, dock})` with gauge, label, text, list, table, button, toggle, input,
separator and group widgets; the session rail beside the transcript for non-bars panels;
`dock: "bars"` gauges in the vitals strip; SMAUG and ANSI colour codes in widget text; focus at
most once a second per panel; and directory script packs (install and upgrade by version, marked
as a pack, the send policy toggle refusing `mud.send` outside aliases and buttons until allowed).

Acceptance:
- Port `MudColorCodes` cases and the panel contract checks (only gauge and label on bars).
- The full Lantern watch example renders its panel and strip against the bench (loopback test).
- Pack install, upgrade and the send policy tested with the directory bench serving `scripts`.
- Panel updates cost nothing when values do not change (frame measurement with ten panels).
- Rail, panel widgets and bars match the reference `help-scripting-panel` and `help-resource-bars`.

C# refs: `src/Wandur.Core/Scripting/ScriptPanelContracts.cs`, `src/Wandur.Core/Terminal/MudColorCodes.cs`,
`src/Wandur.Desktop/Services/ScriptPanels.cs`, `src/Wandur.Desktop/Views/ScriptPanelView.cs`,
`src/Wandur.Desktop/Views/ScriptPanelRailView.cs`, `src/Wandur.Desktop/Services/WorldScriptLibrary.cs`,
`docs/proposals/script-panel-rail.md`, `tests/Wandur.Desktop.Tests/ScriptPanelViewTests.cs`,
`tests/Wandur.Desktop.Tests/ScriptPackTests.cs`, `tests/Wandur.Desktop.Tests/ScriptPanelColorFocusBarsTests.cs`.

Screens: `script-panel-rail`, `script-bars`, `world-editor-scripts`.

### t11 Lua, the Mudlet layer and Mudlet profile import

Goal: Lua scripts through `mlua` (vendored Lua 5.4) behind the `lua` feature, on the same host
API, gated by a preference, with each script's language stored; the Mudlet compatibility layer
ported from the owner's clean-room `mudlet.lua` on the C# branch (never from Mudlet's GPL
source); and File > Import Mudlet profile or package: parse Mudlet XML into macros and Lua
scripts, report what was imported and what was skipped.

Acceptance:
- Port `LuaScriptEngineTests`, `MudletLuaWrapperTests`, `ScriptHookRemovalTests` cases.
- Port `MudletImportTests` with the two C# fixtures (`lantern-road-profile.xml`,
  `gate-helper-package.xml`); plain-send triggers become macros, code becomes Lua scripts.
- Lua scripts obey the same limits as JavaScript (runaway loop test).
- A note in `docs/` records that the layer is clean-room and where it came from.
- Import dialog summary and the script library with a Lua script match the reference.

C# refs (branch `feature/mudlet-import`): `src/Wandur.Core/Import/Mudlet*.cs`,
`src/Wandur.Core/Scripting/LuaScriptEngine.cs`, `src/Wandur.Core/Scripting/LuaBridge.cs`,
`src/Wandur.Core/Scripting/LuaMudletLayer.cs`, `src/Wandur.Core/Scripting/Lua/mudlet.lua`,
`src/Wandur.Core/Scripting/IScriptEngine.cs`, `src/Wandur.Desktop/MainWindow.MudletImport.cs`,
`tests/Wandur.Core.Tests/MudletImportTests.cs`, `tests/Wandur.Core.Tests/LuaScriptEngineTests.cs`,
`tests/Fixtures/Mudlet/*.xml`.

Screens: `mudlet-import`, `script-library-lua`, `settings-scripting-lua`.

### t12 Mapper: tracking, saved maps, search, routes and walking

Goal: replace the initial map with the C# tracker: GMCP and MSDP room ids (`num`, `id`, `vnum`;
area, zone, planet), text-only tracking (`TextRoomObserver`), learned directed edges, provisional
rooms and identity upgrade, areas and floors, saved maps per world in `wandur.db` with merge and
tombstones, the map toolbar (floors, auto-center, grid mode, search box with match stepping and
other-floor list, zoom slider, Stop), exit lights and door marks, route planning (weights,
exclusions) and verified walking on double-click, and the GMCP and MSDP negotiation status line.

Acceptance:
- Port `RoomMapTracker` cases from the core tests (text tracking, upgrade, teleport, pending
  direction rules) and `RoomSearch` cases.
- Route planner: weights, closed and locked doors, excluded rooms, inferred edges opt-in (tests).
- Walking sends one step and waits for the expected room; wrong room, private input, a manual
  command or disconnect stops it (loopback test with a bench map world).
- Maps persist and reload; two sessions of one world merge (test).
- Rendering 2,000 rooms stays under 2 ms a frame (measured).
- Map panel, search, grid mode and route match the reference.

C# refs: `src/Wandur.Core/Mapping/*.cs`, `src/Wandur.Desktop/WorkspaceController.Mapping.cs`,
`src/Wandur.Desktop/WorkspaceController.Navigation.cs`, `src/Wandur.Desktop/ViewModels/MapViewModel*.cs`,
`src/Wandur.Desktop/ViewModels/MapViewport.cs`, `src/Wandur.Desktop/Views/MapView*.cs`,
`src/Wandur.Desktop/Views/RoomMapControl.cs`, `docs/mapper.md`, `tests/Wandur.Core.Tests/ExpandedMapTests.cs`,
`tests/Wandur.Desktop.Tests/MapNavigationTests.cs`, `tests/Wandur.Desktop.Tests/MapRoomSearchTests.cs`.

Screens: `mapper-full`, `mapper-search`, `mapper-grid`, `mapper-route`, `mapper-tools`.

### t13 Map editor, map import and export, room terrain inference

Goal: the map editor document (its own canvas and inspector, Edit map drag, room editor: add,
rename, delete, merge, area, coordinates, terrain, colour, symbol, notes, weight, exclusion; exit
editor: destination, direction, command, cost, door, exclusion, optional return exit; undo and
redo of 50 edits), Import map and Export map in the versioned Wandur JSON (16 MiB, 10,000 rooms,
60,000 exits), and room terrain inference behind the `classifier` feature: lazy load of the
local 0.1.1 classifier package from a path, inference on a worker, threshold setting, "inferred"
with confidence in tooltip and editor, server terrain and edits always winning.

Acceptance:
- Port `RoomMapTracker.Editing` and `MapFileFormat` cases; imports validated before replacing.
- Undo and redo across 50 edits including an import (test).
- First check whether `tract-onnx` runs the package's encoder; record the result in the ledger.
  If it does, a parity test against `tests/Fixtures/room-classifier-parity.json` within the C#
  tolerance; if not, a ruling and the inference interface with a fixture classifier.
- The default build does not link the classifier; with the feature, nothing loads until enabled.
- No hashing or inference on the UI thread (the C# follow-up), measured.
- Map editor, room editor and inferred terrain match the reference.

C# refs: `src/Wandur.Core/Mapping/RoomMapTracker.Editing.cs`, `src/Wandur.Core/Mapping/MapFileFormat.cs`,
`src/Wandur.Core/Mapping/RoomMapTracker.Inference.cs`, `src/Wandur.Core/Classification/*.cs`,
`src/Wandur.Desktop/WorkspaceController.Inference.cs`, `src/Wandur.Desktop/Views/MapEditorView.cs`,
`src/Wandur.Desktop/ViewModels/MapEditors.cs`, `src/Wandur.Desktop/Views/MapEnvironmentPalette.cs`,
`docs/superpowers/specs/2026-09-18-room-terrain-inference-design.md`,
`tests/Wandur.Desktop.Tests/MapEditorTests.cs`, `tests/Wandur.Desktop.Tests/RoomInferenceRenderingTests.cs`.
Model: the 0.1.1 package under `room-classifier/export/release/` (read-only, copy into
`.superpowers/`).

Screens: `map-editor`, `map-room-editor`, `map-inferred-terrain`.

### t14 Local model agents

Goal: per-world agent settings (server address, provider LM Studio native or OpenAI-compatible,
model discovery, instructions, goals with Markdown description and rules, templates, allowed
commands as `id | command | meaning`, limits, timeouts, JSON mode), the API key in the vault, and
the session footer Agent menu (goal radio, Play/Stop, Preview, Step, Reset memory, status,
Recent decisions) with the C# runner rules, behind the `agent` feature.

Acceptance:
- Port `AgentRunnerTests`, `AgentProviderTests`, `LmStudioNativeProviderTests` and
  `AgentProfileStoreTests` cases against a fake model server in `wandur-bench`.
- Only allowed commands are ever sent; private and login text never reaches the model (tests).
- Stop, a manual command, walking, private input, disconnect or a goal change cancels a run.
- Model calls never block the UI thread; a 120 s timeout is honoured (test with a slow fake).
- Agent settings, goals editor and the footer menu match the reference.

C# refs: `src/Wandur.Core/Agents/*.cs`, `src/Wandur.Desktop/WorkspaceController.Agent.cs`,
`src/Wandur.Desktop/Views/AgentSettingsView.cs`, `src/Wandur.Desktop/Views/AgentGoalsEditor.cs`,
`src/Wandur.Desktop/Views/AgentSessionView.cs`, `src/Wandur.Desktop/ViewModels/Agent*.cs`,
`docs/agent.md`, `tests/Wandur.Desktop.Tests/AgentSessionTests.cs`, `tests/Wandur.Desktop.Tests/AgentSettingsTests.cs`.

Screens: `agent-settings`, `agent-goals`, `agent-menu`.

### t15 Skins with title bars, world themes, custom themes

Goal: the three C# skins, chosen independently of the colour theme: Fleet (default), Armored
(plates, rails, foot, title band, the owner's wear texture) and System (plain toolbar in the
native caption area), each with its title bar metrics, Skin and palette buttons, fullscreen
button, dock header heights 38/30/30, and native caption buttons on macOS with custom ones on
Windows and Linux; plus world themes (applied while a world's session is focused, reverted on
close, images and bezels from the directory theme) and custom themes (Settings > Appearance
editor, saved in settings).

Acceptance:
- Port `TitleBarMetricsTests` and `TitleBarLayout` cases as pure layout tests (plate title rules:
  full name when it fits, "Wandur" when not).
- Skin switch keeps sessions and dock content; unknown ids fall back to Fleet (tests).
- World theme follows the focused session and reverts on close (test with the theme fixtures).
- Custom theme editor validates hex values and contrast as `ThemeContrastTests`.
- Theme switch under 10 ms, shell frame within 15% in each skin (measured).
- Main shell in all three skins, light and dark, and the Appearance page match the reference.

C# refs: `src/Wandur.Desktop/FleetSkin.cs`, `src/Wandur.Desktop/FleetToolbarSurface.cs`,
`src/Wandur.Desktop/ArmoredSkinRenderer.cs`, `src/Wandur.Desktop/ArmoredWear.cs`,
`src/Wandur.Desktop/DefaultSkin.cs`, `src/Wandur.Desktop/TitleBarLayout.cs`,
`src/Wandur.Desktop/TitleBarMetrics.cs`, `src/Wandur.Desktop/MainWindow.Skin.cs`,
`src/Wandur.Desktop/MainWindow.TitleActions.cs`, `src/Wandur.Desktop/Theme*.cs`,
`src/Wandur.Desktop/Assets/Skins/`, `src/Wandur.Core/Discovery/WorldTheme*.cs`,
`src/Wandur.Core/Settings/UserTheme.cs`, `docs/fleet-skin-implementation.md`,
`docs/client-architecture.md` (Window skins and color themes), `tests/Fixtures/world-theme*.json`.

Screens: `main-fleet-dark`, `main-fleet-light`, `main-armored-dark`, `main-armored-light`,
`main-system-dark`, `main-system-light`, `skin-menu`, `settings-appearance`, `world-theme-session`.

### t16 Settings sweep, update check, install id, accessibility, packaging

Goal: close the remaining gaps and prove parity: every C# setting present on the five Settings
pages with C# defaults; the update check (`/client/latest` from the configured directory base,
20 s after start then daily, remembered result, skip version, the notice strip, Help > Check for
Updates, off for 0.0.0-dev); the anonymous install id header only to the configured origin and
only on `/directory` and `/client/latest`; an accessibility pass (AccessKit names, roles and
keyboard paths for every control added in t02 to t15, a VoiceOver smoke run on macOS recorded in
the ledger); packaging without signing (`scripts/package-macos.sh` for `.app` and `.dmg`, a
Linux tarball with the `.desktop` file and icons, a Windows zip script); and `docs/parity.md`
rewritten with every row's final status.

Acceptance:
- A test compares the Rust settings with the C# `ClientSettings` field list and defaults.
- Port `UpdateServiceTests` and `InstallIdentityTests` cases against the directory bench; no
  request leaves loopback (bench logs checked).
- Every interactive widget has an accessible name; a test walks the AccessKit tree of each scene.
- `scripts/package-macos.sh` builds an unsigned `.app` and `.dmg` that start; Linux and Windows
  scripts are checked with `cargo check --target` only where that target is already installed,
  and marked untested otherwise.
- `docs/parity.md` updated; every row Done or a named gap with a reason.
- Final measurement run of all scenarios against `docs/measurements.md`.
- All Settings pages and the update notice match the reference.

C# refs: `src/Wandur.Core/Settings/ClientSettings.cs`, `src/Wandur.Desktop/ViewModels/PreferencesViewModel.cs`,
`src/Wandur.Desktop/Views/OptionsDialog.axaml`, `src/Wandur.Core/Updates/*.cs`,
`src/Wandur.Desktop/MainWindow.Updates.cs`, `src/Wandur.Core/Discovery/InstallIdentity.cs`,
`src/Wandur.Core/Discovery/ClientUserAgent.cs`, `scripts/package-macos.sh`, `scripts/package-portable.sh`,
`scripts/linux/*`, `scripts/macos/make-dmg.sh`, `docs/releasing.md`,
`tests/Wandur.Desktop.Tests/UpdateNoticeTests.cs`, `tests/Wandur.Desktop.Tests/PreferencesTests.cs`.

Screens: `settings-general`, `settings-appearance`, `settings-mud-colors`, `settings-terminal`,
`settings-input`, `update-notice`, `about`.

## Reference screens

The reference agent captures these from `.superpowers/csharp-ref` (headless Avalonia capture
tests with `WANDUR_CAPTURE_DIR`, extended in the exported copy only, against loopback bench
servers and fixture catalogs, Lantern Road world, 1600 by 1000) into
`.superpowers/shots/reference/<name>.png`, using the same names the tasks use:

Main shell: `main-hull-dark`, `main-hull-light`, `main-fleet-dark`, `main-fleet-light`,
`main-armored-dark`, `main-armored-light`, `main-system-dark`, `main-system-light`,
`session-play`, `skin-menu`, `world-theme-session`.
Directory: `directory`, `world-details`.
Menus and dialogs: `menu-file`, `menu-view`, `menu-help`, `about`, `link-confirm`,
`update-notice`, `mudlet-import`, `demo-session`.
Settings: `settings-general`, `settings-general-de`, `settings-general-history`,
`settings-appearance`, `settings-mud-colors`, `settings-terminal`, `settings-input`,
`settings-scripting-lua`.
World editor: `world-editor-connection`, `world-editor-login`, `world-editor-scripts`,
`world-editor-macros`, `world-editor-channels`, `agent-settings`, `agent-goals`.
Session: `session-footer-macros`, `session-private`, `session-character-title`,
`session-completion`, `session-tail-split`, `session-vitals`, `server-details`.
Scripts: `scripts-menu`, `script-library`, `script-library-lua`, `script-editor`,
`script-editor-completion`, `script-panel-rail`, `script-bars`.
Mapper: `mapper-full`, `mapper-search`, `mapper-grid`, `mapper-route`, `mapper-tools`,
`map-editor`, `map-room-editor`, `map-inferred-terrain`.
Channels: `channels-panel`, `mark-channel-dialog`.
History: `history-search`, `history-sessions`, `history-notice`.
Diagnostics: `diagnostics-messages`, `diagnostics-observed`, `diagnostics-console`.
Agents: `agent-menu`.

Screens whose C# capture needs the `feature/mudlet-import` branch (`mudlet-import`,
`script-library-lua`, `settings-scripting-lua`) are captured from an export of that branch.
