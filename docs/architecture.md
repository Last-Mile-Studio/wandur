# Rust architecture

Short by design. It records what is built and where later subsystems go. Decisions marked
**(M1)**, **(M2)**, **(M3)** or **(M4)** are implemented; the rest are intentions to revisit with
measurements.

## Goals and constraints

- Core state separate from UI: everything that can be tested without a window lives in crates
  with no egui dependency.
- One canonical incremental terminal model per session, used for rendering, activity, prompts,
  line events and transcript extraction. No second transcript.
- Bounded memory everywhere: pending output, scrollback rows, line events.
- Network I/O never on the UI thread; the UI thread drains batches and paces repaints.
- Protocol pieces pure over bytes, behind a small transport trait, so another transport (a
  WebSocket in a browser) could reuse them.
- Small modules with clear boundaries; no giant state object, no giant UI file.

## Workspace layout (cargo workspace)

```
crates/
  wandur-core/     no UI, no terminal grid; serde, serde_json; rustls behind the `tls` feature
    endpoint.rs    host:port, host port, telnet://, tls:// and telnets://, [ipv6]:port
    telnet.rs      IAC state machine; ECHO, SGA, EOR, NAWS, TTYPE (MTTS cycle), GMCP (Core.Hello), MSSP
    protocol.rs    GMCP (package + JSON) and MSSP (variables, multiple values) decoding
    utf8.rs, charset.rs   incremental UTF-8 or Latin-1 decoding; command encoding with IAC doubling
    prompt.rs      PromptTracker: GA/EOR marks, quiet unterminated line, password prompts
    transport.rs   Transport trait and Link (reader, writer, closer); TCP; TLS (rustls, ring, OS verifier)
    session.rs     Session: reader and writer threads, bounded inbox, waker, NAWS updates, events
    connection.rs  Connection: lifecycle across sessions, manual and automatic reconnect with backoff
    settings.rs    Settings and saved worlds (usage, listing), data directory, atomic writes, Saver, FileSaver
    directory/     (M3) listing model, snapshot parsing, search index, query (filters, sorts), Catalog,
                   HTTP client (ureq, `http` feature), DirectoryService (worker thread, offline cache)
    channels.rs    (M3) GMCP channel messages: All and per-channel tabs, 500 each, unread, replies
    map.rs         (M3) initial room map from GMCP Room.Info
    import/csharp/ the C# client's data: read-only snapshot of its wandur.db, one merge transaction,
                   credential copy (docs/csharp-import.md)
  wandur-term/     the terminal model: alacritty_terminal (Apache-2.0) fed bytes directly
    lib.rs         Terminal: grid with bounded scrollback, LF as CR LF, local echo, reflow, selection,
                   line events (opt in, bounded), current line, transcript
    wrap.rs        (ui/reading) word wrapping at display time: display rows over grid cells
  wandur-app/      egui, eframe, egui_dock presentation (lib plus the `wandur` binary)
    app.rs         WandurApp: frame loop, settings, actions
    pacer.rs       output redraw cap
    fonts.rs       bundled JetBrains Mono Regular and Bold, lazy system fallback (fontdb, skrifa)
    terminal_view.rs  grid painter, mouse selection, scrollbar, copy, status line, input line
    grid_text.rs   (M4) the grid's plain text as one reused mesh of cached glyph quads; AtlasWatch
    autohide.rs    (M4) pinned and auto-hidden tool panels: state, where to put a panel back
    session_tab.rs per-session state over Connection: grid, input, history, prompts, protocols, activity
    sessions.rs    open sessions and the AppAction list panels push
    shell.rs       (M3) TabViewer routing and the status bar
    workspace.rs, layout.rs   (M3) tabs, the C# default layout, saved layout rebuilt through egui_dock
    workspace_panel.rs, world_form.rs   (M3) Find a MUD, open sessions, saved worlds, add/edit form
    dialog_window.rs   large dialogs (world editor, settings) in their own native window (immediate
                   viewport); a movable, resizable modal egui window when viewports are embedded
    directory_view.rs, world_page.rs    (M3) the directory list (virtualized cards) and a world's page
    artwork/       (M3) worker pool, near-size decoding, texture LRU, thumbnail disk cache
    channels_view.rs, map_view.rs       (M3) the side panels for the active session; map_view is
                   also each session's full map (ui/full-map)
    theme.rs, widgets.rs, a11y.rs       (M3) the C# presets, small drawing helpers, AccessKit nodes
    settings_panel.rs
    probe.rs, sysstat.rs, screenshot.rs   opt-in measurement and capture
    script_editor.rs  the script code editor; `Shown`: the formatted text a script is shown as
  wandur-format/   formats scripts for the editor: JavaScript through Biome's formatter (the
                   `javascript` feature), loaded on first use, cached by source for the session
  wandur-bench/    loopback MUD server (rates, steady mode, Unicode page) and micro measurements
```

Later subsystems, each a module in core (logic) plus a module in app (presentation): `history`
(background writer), pattern-based channel capture over line events, the full mapper. Heavy optional features (scripting, ONNX inference, agent) go
behind cargo features and traits so the default build never links them.

## Threading model

- **UI thread**: eframe's event loop. Owns every `Terminal` and all UI state.
- **Per session, two threads** (blocking I/O) **(M1, M2)**: a reader that opens the link through the
  session's `Transport`, reads up to 16 KiB at a time, runs telnet and decoding, and appends text
  and events to the inbox; and a writer that sends commands, negotiation replies and NAWS updates
  in order. For TLS the rustls connection sits behind a mutex that is never held during a blocking
  socket read.
- **Inbox** (`Mutex` plus `Condvar`): decoded text and events, bounded at 256 KiB of text; when full
  the reader waits (TCP backpressure) instead of dropping output. The UI swaps it out in O(1).
- **Fonts worker** **(M2)**: indexes system fonts once (fontdb) and searches character maps
  (skrifa) for characters the loaded fonts cannot draw; started on the first miss.
- **Settings saver** **(M2)** and **layout saver** **(M3)**: write the newest copy after a 200 ms
  pause, atomically.
- **Directory worker** **(M3)**: reads the cached snapshot at start, fetches `GET {base}/directory`
  when asked (first time Find a MUD is shown, Refresh; throttled to five minutes), parses and
  indexes it, writes the cache, publishes an `Arc<Catalog>`. The UI reads a revision number per
  frame and clones the status only when it changed.
- **Artwork workers** **(M3)**: four threads take jobs from a stack (newest first), read the
  thumbnail disk cache or fetch, decode near the target size, write the thumbnail, and hand back
  pixels; the UI thread only turns finished pixels into textures. **(M4)** The first worker trims
  the thumbnail cache at start (10 to 14 ms of directory listing that used to be on the UI thread).

### What runs on the UI thread (audited in M4)

Per frame: draining session inboxes into the grids (the terminal model's parsing, about 1 ms
per frame at 1 MB/s with a 160-column window), building the UI, uploading finished thumbnails as
textures, and comparing the layout description with the saved one every two seconds. At start:
reading `settings.json` and `layout.json` (a few KB each). Everything else is on threads:
settings and layout writes (`Saver`, `FileSaver`, 200 ms coalescing), the directory fetch, parse,
index and cache write, artwork fetching, decoding and the thumbnail cache, system font indexing
and character search. The probe records a histogram of UI-thread time per frame (`frameHist`,
`frameMsMax`, `framesOver16ms`, `framesOver50ms`), and `no_long_ui_thread_stalls_under_a_flood`
(wandur-bench) drives the whole app headless under a flood and fails on a long frame.

## Artwork pipeline (M3)

1. A card or thumbnail asks `ArtLoader::request` each frame it is drawn; the key is the picture
   (what it was made from) plus the pixel size.
2. At the end of the frame every pending job not asked for again is cancelled: a queued job is
   removed before it fetches; a running one checks its flag after fetching and after decoding,
   and a result that arrives cancelled is dropped, never uploaded.
3. Decoding: JPEG with `jpeg-decoder`'s scaled IDCT (1/2, 1/4, 1/8) and, for plates, the crop before the final resize (M4), PNG row by row into a box
   average (the full bitmap never exists), other formats whole; then a triangle resize and a
   centre crop for plates.
4. Textures sit in an LRU bounded at 24 MB (96 MB at first; lowered after measuring the footprint); eviction drops the `TextureHandle`, and egui frees the
   GPU texture at the end of that frame. Textures used in the current frame are never evicted.
5. Resized thumbnails are kept on disk (JPEG, 64 MB, oldest removed first).

## Workspace and layout (M3, M4)

- Tabs: Workspace, Find a MUD, Settings, Map, Channels and one per session. Sessions and Find a
  MUD share the document leaf. **(ui/reading)** View > Layout offers four presets
  (`workspace::Preset`): Panels on the left (one column, Map over Saved worlds over Channels, the
  Workspace closed), Panels on the right (the same column at the right), Both sides (the C#
  layout) and Focus (every tool panel unpinned to the edges). A preset replaces the dock and the
  unpinned panels, keeps the open sessions, and is saved at once as `layout.json`. A new install
  (no `layout.json`) starts with Panels on the left; an existing file keeps the person's layout;
  scenes start with Both sides, as their C# reference captures have. View > Restore Panels gives
  the new-install layout. Nothing needs the Workspace panel open: Find a MUD is on the toolbar,
  and the session tabs (below) show, switch, close, rename and reorder the sessions.
- The layout is saved as our own description (splits with fractions, leaves with panel names),
  not egui_dock's internal tree, and restored by replaying splits. Any file that parses gives a
  valid dock; unknown names are dropped; a bad file is moved to `layout.json.bad`. Sessions are
  not saved. Saved every two seconds when changed, and on exit.
- Map and Channels follow the active session: the session tab focused last.
- **(ui/full-map)** Each session's document switches between Play, Map and Diagnostics in its
  footer's view tabs, with a side-by-side toggle beside them. Map is the session's full map
  (`MapViewState::full`, kept per session in `full_maps`); the docked Map panel is a mini map
  (`MapViewState::mini`). Both draw the session's one `RoomMapTracker`, so a move, an edit or an
  undo shows in both in the next frame; each keeps its own view (centre, zoom, selection). The
  full map's Edit toggle turns on the map editor (below); there is no separate map editor
  document. `terminal_view::show` lays the page out and hands the shell the
  map's rectangle (`ViewActions::map_rect`); a hidden full map is not drawn at all. Under the full
  map, the output strip paints the newest rows of the session's grid (`Terminal::tail_row`, the
  same rows as the live view) into its own glyph mesh: no copy of the transcript. Side by side,
  the share is per session for the run, and the last share settled on is saved in `layout.json`
  (`split_share`) for new sessions.
- **(M4)** Pin and auto-hide for tool panels (Workspace, Map, Channels), which egui_dock does not
  have: unpinning removes the panel from the dock and remembers where it was (the tabs sharing its
  leaf, or the other side of its split with the side and fraction); a strip on the nearest edge
  holds its tab; hovering or clicking the tab slides the panel out in a foreground area over the
  dock; Escape, Hide, a click elsewhere, or moving away for 400 ms (when opened by hovering) hides
  it; Pin splits it back beside the smallest subtree that holds what was on the other side. The
  unpinned panels are saved in `layout.json` (`auto_hide`). Documents cannot be unpinned.

## Panel headers (ui-chrome, October 2026)

The C# client's panel headers (grip at the far left, title, the panel's actions, and the dock's
options, pin and close buttons, shown only on hover or keyboard focus in System and Fleet) are
drawn by the app, not by egui_dock, which has no place for widgets in its tab bar. No fork and no
`[patch]`: for a tool panel docked alone in its leaf of the main dock, the tab viewer's
`tab_style_override` stretches the dock's own tab across the whole header and makes it
invisible, and `is_closeable` turns off the dock's close cross for it. egui_dock still lays out,
drags and drops that tab exactly as before (the whole header drags). After `DockArea::show`, the
app paints the header over it (`panel_header::header`) and adds its buttons there; added later,
they take the clicks, while a drag that starts on the header still goes to the tab underneath.
Header buttons are always interacted (focusable and named); hidden, they have no size, so they
never take a click meant for the title, and the focused one shows the strip. The plan
(`WandurApp::plan_headers`) is made before the dock from last frame's leaf widths, so a panel
knows whether its actions are in the header or in a row of their own
(`panel_header::actions_fit`, the C# eight-character rule). Panels sharing a leaf, floating
windows and slid-out panels keep egui_dock's tabs or a plain action bar. The centre document
leaves hide egui_dock's tab bar (`hidable_tab_bars`, a zero drag height); the app draws its own
session tabs there instead (below). What would break this:
egui_dock changing the tab's id scheme (used for accessibility names) or the meaning of
`TabStyle::minimum_width`; both are covered by tests.

## Session tabs (ui/session-tabs, October 2026)

`session_tabs.rs` draws a tab per open session across the top of the document area, plus Find a
MUD while it is open: it is a document of its own in the same dock leaf, so it gets a tab (with a
magnifier where sessions have their dot) and can be closed like one. The Map and Diagnostics
pages are views of a session (its footer switches them), not documents, so they get no tab.

- Where: the shell draws the strip inside the shown document's body (`Viewer::ui` for a tab of a
  leaf holding only documents, `workspace::document_leaves`), one panel header high, and the
  document under it. A hidden document draws nothing, so the strip costs a few rects and galleys
  per frame (measured below). The order is the dock leaf's tab order; `Sessions` follows it, so
  the Workspace list and the status bar agree.
- A tab: the title (`SessionTab::title`, "World · Character", numbered when sessions share it),
  the connection dot (filled green connected, an amber ring connecting, an amber ring with a
  centre reconnecting, a red ring disconnected), and an accent dot where the close button sits
  when the session has output not seen (the terminal's line and revision counters, as before).
  The close button shows on the shown tab and under the pointer; a middle click closes; a double
  click or the menu's Rename renames in place. The right-click menu: Reconnect (disconnected or
  reconnecting), Disconnect (connected, connecting or reconnecting), Duplicate session (another
  session to the same world, or another demo), Rename, Close, Close other tabs.
- Closing: a shown tab hands over to its right-hand neighbour (the left one when it was last),
  as browsers do (`workspace::remove_document`); the last session gives way to Find a MUD, so
  the document area never disappears; Find a MUD alone stays. Closing never asks: there was no
  close or disconnect confirmation before, and none is added.
- Width (`session_tabs::fit`): what each title needs, at most 220 points; when they do not fit,
  the widest shrink evenly (titles ellipsized) down to 120; past that the strip scrolls (wheel,
  arrows) and a chevron lists every tab. A tab newly shown scrolls into view.
- Drag to reorder (`drop_index`): the other tabs close up round a gap at the place nearest the
  dragged tab; on release the leaf's tabs and the sessions are reordered. A drag never moves a
  session out of the document area. View > Layout presets keep the order.
- Keys: Ctrl+Tab and Ctrl+Shift+Tab go round the tabs in the order shown (Find a MUD among them
  only while it is open; before, they went through Find a MUD, opening it, then the sessions in
  the order opened). Cmd+1 to Cmd+9 (Ctrl elsewhere) show tab 1 to 9 (Find a MUD counts when
  open); a number past the last tab does nothing. Cmd+W closes the shown tab (File > Close).
  Ctrl+Shift+Tab used to be taken as Ctrl+Tab (egui's match ignores an extra Shift); it is
  checked first now.
- The Workspace panel stays (View > Workspace) for people with many sessions; nothing needs it.

## The undo toast (ui/session-tabs)

`toast.rs`: after a delete, a note at the bottom of the window, "Deleted 3 rooms · Undo", for
seven seconds; the clock stands still under the pointer. One at a time: a new toast replaces the
old, whose delete is then no longer undoable from a toast (the map's own Undo still works).

- Map rooms and exits (the full map's editor): the toast holds the tracker's undo step number
  (`RoomMapTracker::last_edit`); Undo runs the tracker's undo only while that step is the next
  one, and the toast goes as soon as another edit (or Edit > Undo) moves it on. More than five
  rooms still ask first; up to five rooms and an exit never asked and do not now.
- Saved worlds: the toast keeps the world exactly as it was (its password reference included)
  and the sessions opened from it; Undo puts it back at its place and points those sessions at
  it again. The saved password is forgotten only when the toast goes without Undo (time up,
  replaced, or the app closing), and not if a saved world still refers to it; nothing secret is
  read or kept. A world with an open session still asks first; others no longer ask.
- Scripts and macros (the world editor's draft library): the toast shows in the world editor's
  window and Undo puts the entry back exactly, at its place, selected. Their delete questions
  ("Delete this script/macro from the world's saved library?") are gone: a delete is one entry
  of an unsaved draft, and the toast covers it.
- Not covered (they keep their questions): deleting a history session (View > Session history).

Dragging a panel (`dock_drop.rs`) works as in C# (Visual Studio): egui_dock decides drops on a
panel and draws its guide glyphs, styled to the theme (accent glyphs, 34 point tiles, no
whole-panel highlight); the app works out the same guide geometry from the pointer, paints the
tiles' faces and a translucent accent rectangle of exactly where the panel will land (30% fill,
50% edge) under them, and adds four window-edge guides that egui_dock does not have (a panel
released there floats in egui_dock and the app docks it along that edge at a quarter of the
dock). After a split drop the app sets the new split's fraction once it has been laid out, so the
panel has the extent the preview showed even when taking it out of its old place widened the
target first.

## Room descriptions from text (map/text-descriptions, October 2026)

Legends of the Jedi and many other worlds send GMCP `Room.Info` (or MSDP rooms) without a
description, so search, the terrain classifier and the inspector had nothing to read. The text
room observer (`map::text`) reads every line anyway (it also notices failed moves); once the
world sends structured rooms, or has GMCP or MSDP on, it hands each finished room block (lines up
to the exits, and the exits) to the session (`MapSession`):

- A protocol room without a description takes the lines under the block's title that matches its
  name (case and spacing ignored; trailing bracketed flags such as `[Bacta]` left out of both),
  and only when the block's exits share one with the protocol's. Occupants and objects after the
  exits are never part of a block; occupant lines and rules inside it are left out, whitespace is
  collapsed, the tracker's 16,000 limit holds.
- Either order: a block finished up to one second before the room (and since the last command),
  or the first block finished up to one second after it. A room change and a GA/EOR prompt mark
  end the block being read. Brief mode (a title and exits, no description) uses the block up and
  leaves the description empty.
- The protocol stays authoritative for identity, exits and position: the text fills only
  `description`, after the tracker placed the room (`RoomMapTracker::set_text_description`, an
  observation, not undoable). The first description is kept; text whose sentences mostly match it
  (a weather or time-of-day sentence) changes nothing; a different text replaces it after two
  visits in a row show it. Rooms edited by hand are never touched. A world that sends
  descriptions is unaffected.
- Filling a description bumps the room's revision, so the classifier worker queues the room again
  (it skips rooms checked at their revision) and a merge keeps the newer text. Saved maps fill in
  as the person walks; there is no migration. During a walk the descriptions read are held and
  applied when it ends, since a room's new revision would stop the walk as a changed map (the
  classifier's results wait the same way).
- Cost (on the UI thread, where the observer already ran): one block copy and a title search per
  finished room block. `wandur-bench mud-server --page lotj` and the shell scenario "flood 1 MB/s,
  LotJ rooms" serve a LotJ-like flood; the micro measurement "Map session, 1 MB of LotJ-like
  flood" went from 12.5 to 14.2 ms per MB (0.2% of a core more at 1 MB/s), the shell scenario's
  logic time stayed within noise (2.3 to 2.5 ms per frame before and after).

## Map editor (ui/map-editor, October 2026)

The full map's Edit mode is a small drawing program over the session's live map
(`map_view/editor.rs` and its `toolbar`, `canvas` and `inspector` modules):

- A toolbar over the canvas: the tools Select (V), Add room (R) and Connect (E), Delete, Undo,
  Redo, grid snap, the area and floor (the shared select) and Fit. Escape steps back: closes a
  menu or a question, cancels a gesture, returns to Select, clears the selection; only then does
  the Map page's own Escape (back to Play) run (the editor says each frame whether it keeps
  Escape, in an egui temporary value the session view reads).
- Canvas tools: click, Shift or Cmd click, a marquee from empty space; dragging the selection
  moves it (snapped to whole cells when snap is on; locked rooms stay); arrows nudge; Add room
  places a room on the clicked cell (in the shown area and floor), selects it and focuses its
  name; Connect drags from room to room with a rubber band and the inferred direction (eight
  compass points, up and down between floors), Alt or Option for one-way; a right click opens a
  context menu (rename, connect from here, set the position here, lock, merge into, delete;
  one-way or two-way and delete on an exit; add a room on empty space). Space and drag, or the
  middle button, pans. Exits are hit along their drawn line (each half of a two-way line is
  its own exit).
- A property inspector docked on the right (its width, closed sections and snap are saved in
  `settings.json`, `map_editor`): Room, Position, Exits (a compact table), Notes and
  description for one room; the shared fields of several rooms with "Mixed" where they differ
  and Move by instead of Position; an exit's own fields; the map overview and shortcuts when
  nothing is selected. Choices, checks and steppers apply at once; text fields keep a draft and
  commit on Enter or when they lose focus, so typing is one undo step; a bad value is flagged
  under its field and not applied.

Every change is an `EditAction` applied after drawing through `wandur_core::map::editing`
(connect, place and move rooms, edit rooms and exits, one-way, delete), which uses only the
tracker's own editing calls inside `RoomMapTracker::edit_group`: the checks, revisions,
tombstones, manual-edit marks (which keep hand-placed rooms out of automatic docking), saving
and merging stay the tracker's, and each action is one undo step. The Edit menu's Undo and Redo
reach the map when no text field has the keyboard. Outside Edit mode the map draws as before:
the editor's highlights, exit hit lines and overlays are computed only while editing.

## Map labels and map import (map/mudlet-json-import, October 2026)

- **Labels** (`map::model::MapLabel`): an id, area, floor, top-left corner and size in map
  cells (north is +Y), text with its size (points at zoom 1), colour and background, or a
  picture (`MapImage`: PNG or JPEG bytes in an `Arc`, keyed by SHA-256, so undo steps and saves
  copy nothing), opacity, above or under the rooms. The tracker keeps them like rooms: manual
  edits with revisions, tombstones, undo (an undo step carries the pictures it needs), the
  revision merge (`merge::combine`, pictures kept when a merged label shows them) and the map
  file (`format`: version 2 only when the map has labels, so a map without them stays readable
  by the C# client; base64 pictures, 64 MiB a file, 16 MiB of map besides pictures).
  `wandur.db` migration 9 adds `map_labels`, `map_label_deletions` and `map_images` (per world,
  keyed by hash, written once and deleted when no label shows them). Pictures are 2 MiB each at
  most after `images::prepare` scales them down (2048 a side), 20 MiB a map, 5,000 labels.
- **Drawing** (`map_view/labels.rs`): the floor's labels before the exits (under) or after the
  rooms (above), on the full map and the mini map alike. Pictures are decoded once on a thread
  of their own into textures (`LabelImages`, one per window, 64 MiB of RGBA, least recently
  drawn first out); nothing is decoded while drawing, a picture not ready yet is a frame.
- **Editing**: the editor's Add label tool (L), Target::Label (rooms and exits are hit first),
  drag to move, the corner grip to resize, arrows nudge, Delete with the undo toast, a context
  menu and an inspector section; each change one undo step (`editing::edit_label`,
  `place_picture_label`).
- **Import** (`map::mudlet`, `map_import.rs`): File > Import map... and the full map's Import
  open a dialog that picks the world (the active session's first; saved worlds with no
  session), reads the file on a worker thread with progress, and shows the summary. This
  client's file replaces the map (`replace_map`); Mudlet's JSON export is merged
  (`RoomMapTracker::import_map`: stable ids, unchanged items skipped, one undo step). Into a
  session's map the toast's Undo is the map's undo; into a world with no session the import
  loads, merges and saves on a worker thread and the app keeps that world's tracker while the
  toast shows. Details and every assumption about Mudlet's format: `docs/mudlet-map-import.md`.

## Menus (ui-chrome, October 2026)

One menu model (`menus::menus`, the C# `DesktopMenus` order, labels, shortcuts, checks and
enabled rules) feeds everything: on macOS the system menu bar (`native_menu.rs`: `spec` turns the
model into a plain description, tested everywhere; `NativeMenuBar` builds it with `muda`, sets
changed checks and enabled states in place each frame and rebuilds when a label changes), and on
every platform the title bar's menu button (`menus::MenuButton`, each menu a submenu, keyboard
navigation in `MenuButton::key`). There is no menu bar in the window. On Windows and Linux the
button sits at the top left and Alt alone, Alt+F or F10 open it with the first item lit; on macOS
it is the last title control. Window shortcuts (`menus::shortcuts`) work with no menu showing;
on macOS a native menu's key equivalent reaches the app as a menu event instead of a key press,
so nothing runs twice. Session > Show Map (Cmd+Shift+M on macOS, Ctrl+Shift+M elsewhere; Cmd+M
stays Minimize) switches the active session between Play and Map, and Session > Play and Map Side
by Side toggles both at once (ui/full-map).

## Script formatting (October 2026)

The world editor shows every JavaScript script formatted for reading, pack scripts included
(`script_editor::Shown`). Showing is not editing: the draft keeps the stored source while the
editor's text is the untouched formatted view, so viewing never marks the world changed. The first
edit makes the draft hold the editor's text (formatted, with the edit), which Save world writes;
undoing back to the formatted view is the stored source again. A copy (Duplicate, or Make copy
from a pack script) starts from the text shown. Lua, a source that does not parse, one over 64 KB
and a format slower than 50 ms are shown as stored, with no message.

`wandur-format` wraps `biome_js_formatter` 0.5.7 (MIT OR Apache-2.0) with Prettier's defaults:
two spaces, semicolons, double quotes unless that needs more escapes, trailing commas, parentheses
around arrow parameters, 100 columns; a source without a final newline gets none. Biome stopped
publishing to crates.io at 0.5.7, so the crate names Biome's own crates at exactly 0.5.7 (later
point releases of some do not build with it). The options are built on the first script shown
(`is_loaded`), results are cached by source for the session (`shown_javascript`, emptied at 4 MB),
and the app's `format` feature (on by default) turns it on.

Rulings, with the alternatives measured (`.superpowers/formatter/`):

- Biome over `dprint-plugin-typescript`. Why: dprint's parser (swc) needs `smartstring`, which is
  MPL-2.0, and it adds more (4.2 MB against 3.7 MB stripped, 60 MB against 12 MB peak memory on a
  130 KB script); the quality was the same once dprint keeps braces. Cost: Biome's crates.io
  release is from 2024 and frozen; fixes would need a move to dprint or a vendored Biome.
- Biome over `prettify-js` (BSD-2-Clause, 0.1 MB). Why: a token-based reprinter writes `e=>{`,
  `() =>mud.send`, `name=== "x"`, drops blank lines and explodes every object, and cannot tell
  that a script does not parse; since an edit saves the shown text, it would make tidy scripts
  worse. Cost: 3.6 MB more in the binary.
- No Lua formatting. Why: StyLua as a library adds 2.9 MB and about 80 crates, among them its
  command line's (clap 3, atty), and is MPL-2.0; Lua scripts are an opt-in prototype. Cost: Lua
  is shown as stored.

## Directory (M3)

- The snapshot's searchable text is normalized once per world when the catalog is built, so a
  query compares prepared strings; results are recomputed only when the query, the catalog or the
  saved worlds change.
- The list scrolls with the advanced filters above it (`ScrollArea::show_viewport`); every card
  has one height per width mode (plate on top below 560 points, beside the text with the way in
  under it below 760, beside it with a ruled column above), so the cards in view are found by
  arithmetic and only they are laid out and request artwork. Text wraps inside the card: the
  name takes up to two rows, the chips up to two, the blurb the rows left.
- The controls above the list are laid out to the panel's width (`control_slots`): one row when
  they fit, else the search box alone and the rest flowing under it. The advanced filters are a
  grid of captioned cells (`filter_columns`). Every select is `select::Select`: one field
  surface, opened by a click anywhere or the keyboard, named for screen readers.

## Channels (M3)

- `ChannelLog` (core) keeps `Arc<ChannelMessage>` in All and per-channel tabs (500 each). The panel
  caches one galley per message keyed by its sequence number and wrap width: a new message costs
  one layout, and only rows in view are painted.

## Output pipeline (M2)

1. Reader: bytes -> `TelnetParser` (data, replies, events) -> `TextDecoder` -> inbox. GMCP and MSSP
   payloads become `SessionEvent::Gmcp` and `SessionEvent::Mssp`.
2. UI frame start (`App::logic`): each `SessionTab` drains its `Connection`, feeds text to its
   `Terminal` (alacritty's vte parser drives the grid), applies events at their offsets, and runs
   the reconnect and prompt timers. One pass, no per-byte UI work.
3. Render: only the visible rows are visited; each row becomes a few text runs (see the view).
4. Activity: `Terminal::lines_total` (line feeds, never decreases) and `revision` (every feed);
   a tab compares them with what it last showed.

## Terminal model (M2)

The `alacritty_terminal` grid is the transcript. It is fed server bytes directly (no PTY):

- Scrollback is counted in rows (default 2,000, as the C# xterm display; hard maximum 50,000).
  Rows are 24 bytes per cell, allocated at full width: about 5.9 MB at 120 columns and 2,000 rows.
  alacritty grows storage 1,000 rows at a time; spare rows are released once the history is full.
- Reflow on resize, wide characters (two cells), combining marks (zero-width extras on a cell),
  SGR colours and attributes, cursor addressing, erase, scroll regions and the alternate screen
  come from alacritty. LF is treated as CR LF because MUDs often send a bare LF.
- Local echo is written to the grid through alacritty's `Handler` methods, not the parser, so a
  server escape sequence split across reads is never disturbed.
- Line oriented consumers do not keep a second transcript. Prompt detection reads the current line
  from the grid when it fires; line events (for future triggers and channels) are opt in, read
  back from the grid as each line completes and queued (at most 4,096 between takes); transcript
  export and copy read the grid on request. The only second representations are these transient,
  bounded line events and the small per-frame run buffer of the renderer.
- Colours stay symbolic (named, indexed or RGB) in the cells; the theme resolves them at paint
  time, with bold turning the eight basic colours bright.

## Rendering (M2, M4)

- Each visible row is split into runs. Plain characters (ASCII, Latin, box drawing) of one style
  go into one mesh for the whole grid **(M4)**: each character's glyph quad is cut once from a
  one-character galley (cached per character and style) and copied to its cell, on whole physical
  pixels, into a mesh whose buffers are reused from frame to frame. Under a flood the rows on
  screen are new every frame, and laying them out with egui (shaping, glyph lists, a mesh per
  galley) cost 1.6 ms and 1.3 MB per frame; the mesh costs 0.2 ms and 11 KB. No ligatures (as
  most terminals). The mesh travels as a galley so egui normalizes its texture coordinates at
  tessellation time (the font atlas may grow during a frame).
- Any other character (CJK, symbols, combining sequences) is placed alone at its cell position as
  a galley, centred in its one or two cells, so fallback glyphs with other advances cannot push the
  row off the grid. Block elements (U+2580 to U+259F) are painted as rectangles; underline and
  strike are lines.
- Galleys kept across frames (glyph quads, Channels rows) are dropped when egui rebuilds its font
  atlas (`AtlasWatch`: a one-character galley asked for every frame changes identity then).
- Selection, scrolling and copy work on the grid: drag (past the edge scrolls), double click word,
  triple click line, Shift+click extends; copy joins soft wraps. The view has its own scrollbar.

## Word wrapping (ui/reading, October 2026)

alacritty wraps a long line at exactly the last column, as a terminal does, which splits words
("restocked wit / h hunted goods"). The transcript is drawn word wrapped instead (Settings >
Terminal > Wrap at word boundaries, on by default; Indent wrapped lines adds a two column
hanging indent, off by default). It is a display layout over the grid, not a second model
(`wandur-term`, `wrap.rs`):

- A `DisplayRow` names the cells of one logical line it shows (the grid line the line starts
  on, a start and end offset, an indent). The painter reads those cells straight from the grid,
  in one or two row slices, so colours, wide characters and combining marks come from the cells
  as before. Breaks go after spaces, after a hyphen inside a word and around wide characters;
  spaces at a break stay at the end of the row; only a word wider than the row is cut. A wide
  character is never split from its spacer.
- Selection, copy, search, links, line events and the transcript stay on the grid: a point on
  screen maps to a grid cell through its display row (`Terminal::display_point`), the selection
  is drawn per display row (`row_selection`), and copying reads the grid, so it gives each line
  as the MUD sent it, with no inserted line breaks.
- Only normal flowing text is rewrapped. The alternate screen keeps the grid's exact rows and
  columns (full-screen programs and cursor addressing), as do lines that fit in one row and
  logical lines longer than 64 grid rows (`MAX_WRAP_ROWS`, which also bounds the work per line).
- Placement: the view's bottom grid row stays at the bottom (live, the cursor's row or a lower
  one holding text), and the extra rows rewrapping adds take room at the top; scrolled all the
  way up, the oldest row stays at the top instead so every row can be reached. The live view and
  the Map page's output strip are wrapped the same way, newest at the bottom. Scrolling still
  moves by grid rows.
- NAWS: the server is told the real width (the grid's columns), not a wider one. A server that
  formats to the width (tables, centred titles, its own wrapping, full-screen pages) gets the
  truth and its output fits; a MUD that sends long unwrapped lines is word wrapped here. Telling
  a larger width would make such servers produce lines wider than the window, which would then be
  rewrapped anyway and would break their tables.
- Cost: the layout of the rows on screen is kept until the grid's revision, the scroll offset,
  the width, the height or the options change (`TerminalViewState::display`), so an idle or
  pointer-only frame lays nothing out. Under a flood every frame has new rows, and laying out
  the rows in view is a walk over their cells, the same order of work as building their text
  runs. A per-line cache would need a stable identity for grid lines, which alacritty does not
  give (rows rotate once the history is full); measured, it is not needed (`docs/measurements.md`,
  Word wrapping).
- Limit text width (Settings > Terminal, off by default, 100 columns, 60 to 240): in a pane wider
  than the limit, the grid is that many columns and the transcript is centred in the pane; the
  server is told that width (NAWS), as it is the width the text is laid out at. A narrower pane
  is filled as before.

## Repaint policy (M2)

- egui answers every zero-delay repaint request with two frames, and shortens every delay by its
  predicted frame time; a delayed request from another thread paints at once. So the network
  waker asks for a 1 ms repaint only when an output frame is due, and the UI thread, after it
  applied output, schedules the next output frame itself (interval plus predicted frame time).
  Default cap: 30 output frames a second (`--output-fps`, Settings). Input repaints at once.
- The caret does not blink: the blink cost about ten frames a second while idle.
- Reconnect and prompt timers request a repaint when they are due.

## Persistence (M2)

`settings.json` (versioned, unknown fields ignored, values clamped; M3 added theme, directory
address, adult worlds, and per world the listing id and usage), `layout.json` (M3),
`directory.json` (the snapshot, M3) and `thumbnails/` (M3) in the data directory:
`--data-dir`, else the platform data directory plus `Wandur-Rust` (never the C# client's
`Wandur`). Written to a temporary file and renamed. Unreadable files are moved to
`settings.json.bad`. Input history is not persisted (it can hold passwords typed without echo off).

Parity t01 added `wandur.db` (`wandur-core::db`), the C# `ClientDatabase` in Rust: rusqlite with
SQLite bundled (FTS5 compiled in), WAL, foreign keys, numbered migrations applied in one
transaction and recorded in `user_version`. Reads open a short connection; writes go to one writer
thread that takes everything queued as one transaction (a savepoint per job), so the UI thread
never waits for the disk. A file SQLite cannot read is renamed `wandur.db.corrupt-<seconds>` and a
new one is made; a file from a newer schema is left alone and not used that run. Saved worlds keep
a stable `world_id` in `settings.json`; the database maps every endpoint key (canonical host, IDNA
ASCII, no trailing dot, plus port) to it, so an edited address keeps the world and the old address
stays an alias. Later stores (macros, scripts, maps, history) key by this id.

## Importing the C# client's data

`wandur-core::import::csharp` never opens the C# data folder for writing: `wandur.db` and its
`-wal` are copied into a private temporary folder, the log is folded into the copy, and the copy
is opened `SQLITE_OPEN_READ_ONLY` with `immutable=1`. The merge into this client's `wandur.db` is
one immediate transaction (rolled back on any error or for a dry run); `settings.json` is written
just before the commit and restored if the commit fails. Ids are kept, so a second run finds
everything already there. The app runs it on a worker thread and takes the returned settings on
the UI thread; secrets are copied after the commit, only when asked. Details and the table map:
`docs/csharp-import.md`.

## Scenes (parity t01)

`wandur --scene NAME` builds a named screen in memory (`Options::ephemeral`: no settings, layout,
directory cache, thumbnails or database writes). With `WANDUR_SCREENSHOT` it runs headless: the
app's frames go through `egui::Context::run_ui` with real time between them, textures go to
egui_kittest's wgpu renderer (no window, no surface), and once the scene reports ready for twelve
frames in a row the last frame is rendered to PNG. The bench serves the C# reference fixtures:
The Lantern Road (`mud-server --page lantern`) and the four fictional directory worlds
(`directory-server --fixture`).

## Browser feasibility (checked in M2, not built)

- `cargo check --target wasm32-unknown-unknown -p wandur-core --no-default-features` succeeds:
  the protocol, prompt, reconnect and settings code compiles for the browser. `std::net` and
  `std::thread` compile there but fail at run time, so a browser build needs a WebSocket
  `Transport` and a single-threaded driver of the same pure pieces.
- With the `tls` feature, `ring` needs `getrandom`'s `js` feature on wasm32 (and a browser would
  use WSS instead anyway).
- `wandur-term` does not compile for wasm32: `alacritty_terminal` always builds its PTY event
  loop, whose `polling` dependency has no wasm32 support. A browser build would need an upstream
  feature to leave out the PTY parts, a fork, or another grid. egui and eframe themselves support
  the web; fontdb and memory mapping would be replaced by bundled fonts.

## Testing

- Core: unit tests per module and loopback sessions (lifecycle, backpressure, NAWS, GMCP and MSSP
  events, Latin-1, TLS with a generated certificate, untrusted certificate), reconnect with backoff
  and give-up, settings round trip, clamping, corrupt files and the saver.
- Term: grid tests (bounds, reflow, wide and combining characters, colours, local echo, prompts,
  selection across scrollback, soft-wrap copy, selection surviving output, line events).
- M3: directory parsing, search, filters and sorts; the directory service with a fake fetcher
  (cache, offline, corrupt cache); channels (decoding, tabs, bounds, private ordering) and the
  panel's incremental layout; map placement and drawing; layout round trip, corrupt, old and
  unknown entries; saved worlds; artwork decoding, cancellation, LRU budget, disk cache; themes
  (every preset's contrast, applying light and dark); AccessKit nodes.
- App: headless egui tests (runs, block elements, a real mouse drag past the top edge selecting
  into history), session tabs over loopback (prompts, password masking, reconnect), several
  sessions with separate output and activity, the pacer, fonts.
- Bench: `wandur-bench micro`, and the C# harness driving the Rust binary for app-level scenarios.

## Optional features: what starts, and where the deferred ones plug in (M4)

Nothing optional starts at launch. Fonts beyond the bundled pair are indexed only when a
character is missing; the directory is fetched only when Find a MUD is first shown (its cached
snapshot is parsed on the directory thread); artwork workers sleep until a card asks; line events
(each completed line read back from the grid) are off unless something consumes them; the script
formatter is first used when the world editor shows a script (a test checks it in a fresh process);
the probe,
screenshots and panel statistics exist only when their environment variable is set.

The deferred heavy features have a place to plug in without touching the default build:

- **Scripting** (C#: Jint in a worker process): a `scripting` cargo feature in a new
  `wandur-script` crate, behind a trait in core such as
  `trait ScriptHost: Send { fn on_line(&mut self, line: &CompletedLine) -> Vec<ScriptAction>; fn
  on_gmcp(&mut self, m: &GmcpMessage) -> Vec<ScriptAction>; fn on_command(&mut self, input: &str)
  -> Option<String>; }`, running on its own thread with a bounded channel. A session turns on
  `Terminal::set_line_events` only while a host is attached, so the cost without scripts stays
  zero. Actions (send, echo, gag, set a variable) come back to the session tab like `AppAction`.
- **Room terrain classification** (C#: ONNX Runtime): an `ml` cargo feature with
  `trait RoomClassifier: Send { fn classify(&mut self, room: &RoomText) -> Option<Terrain>; }`,
  the model loaded lazily the first time the map asks, inference on a worker thread, results
  cached by room id in `RoomMap`. Candidates: `ort` (ONNX Runtime binding, a native library per
  platform) or `tract` (pure Rust). The default build links neither.
- **Tokenizers, agent, history (FTS5)**: same pattern: a feature, a trait in core, a worker
  thread, nothing at start.

## Assumptions

- MUD output is mostly line oriented; full screen programs work through alacritty but are rare.
- Complex shaping, right-to-left reordering and colour emoji are out of reach of egui's text
  engine; they are documented limitations.
- The default renderer is eframe's wgpu backend (Metal on macOS); glow is a cargo feature.
