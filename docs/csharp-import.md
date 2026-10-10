# Importing the C# client's data

The Rust client can bring over everything the C# Wandur client keeps that has a counterpart
here. Code: `crates/wandur-core/src/import/csharp/` (core), `crates/wandur-app/src/csharp_import.rs`
(dialog and command line). Tests: `crates/wandur-core/src/import/csharp/tests.rs`,
`crates/wandur-app/tests/csharp_import_cli.rs` and an app test in `crates/wandur-app/src/app.rs`.

## Running it

In the app: **File > Import from Wandur (C#)...** The dialog starts in the C# client's data
folder, counts what it holds, and imports on a worker thread with a progress bar. The summary
lists counts per kind and why anything was skipped. The box "Also copy saved passwords and agent
API keys" is ticked by default; macOS then asks once per Keychain entry whether this app may
read the C# client's item.

From a terminal:

```sh
wandur --import-csharp "$HOME/Library/Application Support/Wandur" [--data-dir DIR] [--dry-run] [--include-passwords]
```

- The first argument is the C# data folder or its `wandur.db` file.
- `--data-dir` is this client's data directory; without it, the platform default:
  `~/Library/Application Support/Wandur-Rust` on macOS, `%APPDATA%\Wandur-Rust` on Windows,
  `$XDG_DATA_HOME/Wandur-Rust` (or `~/.local/share/Wandur-Rust`) on Linux.
- `--dry-run` works on a copy of this client's `wandur.db` and `settings.json` in a temporary
  folder and writes nothing (it does not even create the data directory).
- `--include-passwords` also copies saved passwords and agent API keys. Without it no credential
  store is touched and they are counted as skipped. The imported worlds still keep their password
  reference (a later import with `--include-passwords` fills that same entry); the world editor
  checks the store when it opens a world and, when the entry is not there, says "No saved password
  found for this world. Enter it to save it." instead of "Saved", and Save world asks for it (or
  for Save password to be unticked). Auto-login without the entry connects and says the password
  was not found, as before.
- Usernames are imported trimmed, as the world editor saves them, so the editor's copy has the
  same vault key and a blank password keeps the copied one.
- It prints counts per kind and skip reasons only: never a world name, address, account, text or
  secret. Exit code 0 on success, 1 when the import did not run, 2 for a usage error.

Before importing:

- **Close the C# client.** The importer copies `wandur.db` and its `-wal` file; a client writing
  at that moment can leave the copy half a transaction behind (the copy is checked with SQLite's
  `quick_check` and refused if damaged, so the worst case is an error, not bad data).
- **Close the Rust client when using the command line.** The running app keeps its settings in
  memory and would write its own copy of `settings.json` over the imported worlds. The in-app
  import has no such problem: the app takes the imported settings itself.
- The C# data is never changed. The Rust data directory is changed only by a successful import.

## How it reads the C# data (read-only guarantee)

1. `wandur.db` and, if present, `wandur.db-wal` are copied by plain reads into a private folder
   under the system's temporary directory (`wandur-csharp-import-<pid>-...`). The C# folder is
   never opened for writing, not even by SQLite: opening a WAL database in place would create a
   `-shm` file next to it.
2. The copy's write-ahead log is folded into the copy (`wal_checkpoint(TRUNCATE)`, then
   `journal_mode=DELETE`), so the newest committed writes are included.
3. The copy is opened with `SQLITE_OPEN_READ_ONLY` and `file:...?immutable=1`, checked with
   `quick_check`, and its schema version (`user_version`) is read: 1 to 9 are accepted (7 is the C#
   main branch, 8 and 9 the Mudlet import branch); anything newer is refused with "The C# database
   uses schema N, newer than this importer knows (9). Update the Rust client before importing."
   A file without the C# tables is refused as not a C# Wandur database.
4. The snapshot folder is deleted when the import ends (also after an error).
5. Older C# files (`maps/*.json`, `scripts/*.scripts.json`, `scripts/*.js`) are only read.

Tests hash the fixture before and after, check that no file appears next to it, and hold a write
open in a copy's WAL to prove it is read.

## Merging: the policy

The import merges and never duplicates; it does not refuse a data directory that already has
data. Chosen over refusing because the owner may already have tried the Rust client, and because
the same rules make a second run harmless.

- **One transaction.** Every database change of a run is one `BEGIN IMMEDIATE` transaction on
  this client's `wandur.db`; an error anywhere rolls all of it back. `settings.json` is written
  (atomically, temporary file then rename) just before the commit; if the commit then fails, the
  previous `settings.json` is put back. In the app the settings are handed to the app instead and
  saved by it after the commit.
- **Identity.** A C# world keeps its world id (32 hex digits, the same form this client uses)
  unless this client already has one of its addresses under another id; then everything of the
  C# world joins that world. Scripts, macros and agent profiles keep their ids; history sessions
  keep their ids; maps merge by room id and exit (from, direction).
- **Something already here.** The same item is counted as "already there". A different version
  is kept as it is here and reported as skipped ("this client already has its own version, which
  was kept"). Maps are the exception: they merge by revision exactly as two saves of one map do
  (newer revision wins, a manual edit wins a tie, tombstones keep deleted rooms and exits deleted).
- **Preferences** are taken only while this client's preferences are still the defaults (for the
  fields both clients have); otherwise they are kept and reported as skipped.
- **Idempotent.** A second run changes no row and no byte of `settings.json`
  (`running_the_import_again_changes_nothing`, also after merging into existing data).

## What goes where

C# side: `src/Wandur.Core/Storage/ClientDatabase.cs` (schema), and the stores named below.
Rust side: `crates/wandur-core/src/db/mod.rs` (schema 8) and `settings.rs`.

| C# table or file | Holds | Rust destination | Imported, transformed or skipped |
|---|---|---|---|
| `worlds` | world ids | `worlds` | Imported. Ids kept, or mapped to the id this client has for one of the world's addresses. Worlds without a saved profile (ad hoc connections) are imported too, since maps, usage and history refer to them. |
| `endpoints` | canonical `host:port` keys per world | `endpoints` | Imported after re-canonicalizing with this client's `canonical_endpoint_key` (IDNA, IPv6 brackets, no trailing dot). An address that belongs to another world here is skipped ("address taken"). |
| `legacy_endpoints` | older spellings (`source_key`) of each key | `endpoints` | Folded in: each spelling is canonicalized; one that leads to a key not listed yet becomes an address. `canonical_endpoint_key` now drops the C# `:True`/`:False` TLS suffix, as C# does, so the old `host:port:tls` keys find the world. The table itself has no counterpart. |
| `profiles` (`payload` = `ConnectionProfile` JSON, `position`) | saved worlds | `settings.json` `worlds` | Imported in C# order. Name, host, port, TLS, encoding (`utf-8`, `latin1`), username, password reference, auto-login, both prompts, codebase, channel rules, world theme (directory wire format), protocol mapping. Usage fields filled from `world_usage`. `auto_reconnect` and `listing_id` (Rust only) take their defaults. The C# profile `Id` (a GUID) is not kept (this client keys worlds by world id) but is used to find the password. |
| `profiles` channel rules | taught channel rules | `SavedWorld::channel_rules` | Imported. A rule whose .NET pattern uses look-around or backreferences (which the `regex` crate cannot run) is skipped with that reason; other invalid rules as invalid. |
| `client_settings` (`payload` = `ClientSettings` JSON) | preferences | `settings.json` | Theme, skin, language, text size, foreground and background, local echo, blinking text, live view share, world themes, local room classification and its threshold, map auto-centre, Channels panel, composer suggestions, history on and retention, hide the history notice, update check on, send install id, and on the Mudlet branch the Lua preference. Taken only while this client's preferences are the defaults. |
| `client_settings` `CustomThemes` | colour schemes | `settings.json` `custom_themes` | Imported with the preferences (id, name, light, all chrome colours, terminal colours). The base preset, which C# does not record, is Hull for a light scheme and Ember for a dark one; it only matters for colours a scheme does not name, and C# schemes name all. |
| `client_settings` `InstallId` | install id | none | Skipped: each client keeps its own install id. |
| `client_settings` `LastUpdateCheck`, `SkippedUpdateVersion` | update check | none | Skipped: they describe C# releases (this client's update check is separate, t16 ruling). |
| `world_connections` (ISO 8601 text times) | connection log | `world_connections` (seconds) | Imported, times converted; a time already logged for the world is not added again. |
| `world_usage` (`connections`, `last_connected_at` text, `last_character`) | usage summary | `world_usage` and `SavedWorld::connections`, `last_connected` | Recomputed: connections grow by the log rows added, the last connection is the later of the two, the last character is kept if this client has one. |
| `scripts` (`macro_json`, `pack_json`, and on schema 8 and 9 `import_json`, `language`, `compatibility`) | script libraries and macros | `scripts` | Imported row by row with every column (this client's table is a superset). Each entry passes this client's checks first (a macro's source must be its compiled JavaScript, which this client produces identically). A full library (64 entries or 4 MiB) skips the rest with that reason. |
| `imports` `script-library:<world>` | library set up (starter given) | `imports` | Imported for the mapped world, so no second starter script appears. Other C# markers (`settings-json`, `map-file:...`, `script-source:...`) are the C# client's own migration steps and are only read, to tell which older files are still pending. |
| `script_deletions` | tombstones of deleted scripts | none | Skipped ("no counterpart"): this client has no older script files that could bring a deleted script back. They are honoured when older C# script files are read. |
| `world_agent_profiles` (`AgentProfile` JSON) | agent settings | `world_agent_profiles` | Imported after this client's validation, re-encoded by this client (same field names); the profile id is kept, so an API key stays bound to it. |
| `map_snapshots`, `map_rooms`, `map_links`, `map_areas`, `map_room_aliases`, `map_room_deletions`, `map_link_deletions` | saved maps | the same tables | Every table imported: coordinates, areas, floors, server ids, edits, locks, notes, weights, colours, symbols, inferred terrain, door states, exit commands and drawn lines, area grid mode, aliases, tombstones with revisions. Read and written with the map store's own code and validated (limits, references). Merged by revision with a map already here. The tracker loads them unchanged (tested). |
| `history_sessions`, `history_entries` | session history | `history_sessions`, `history_entries` | Imported with session ids; times converted from .NET ticks to microseconds since 1970; entries keep sequence, kind (`received`, `sent`, `script`, `private`) and text, so private markers stay markers. Sessions get this client's integer key. A session id already here is not imported again; one deleted here stays deleted. |
| `history_entries_fts` and its shadow tables | search index | `history_entries_fts` | Not copied: rebuilt (`'rebuild'`) after the import, so search covers imported text. |
| `history_deletions` | deleted sessions | `history_deletions` | Imported, so a deleted C# session can never be imported later. |
| `world_capabilities` | protocol knowledge per address (GMCP, MSDP, room fields seen) | none | Skipped ("no counterpart"): this client has no protocol knowledge store (a Partial row of the Map panel). |
| `cache_entries` | the C# directory cache | none | Skipped ("no counterpart"): this client fetches and caches the directory itself (`directory.json`). |
| `wandur.db-wal` | newest writes | (folded into the snapshot) | Read through the snapshot. |
| `wandur.db-shm` | WAL index | none | Never read or copied; SQLite rebuilds it in the snapshot. |
| `settings.json` | settings before the C# database existed | none | Not read. The C# client moves it into `wandur.db` the first time it starts with a database; if a C# install never did, start the C# client once first. |
| `maps/<SHA-256 of host:port>.json` | per-address maps before the database | map tables | Read when the C# database has no `map-file:` marker for the file (the C# client moves such a file the next time it opens that world's map), and merged under the database's map (the database is newer). |
| `scripts/<SHA-256>.scripts.json`, `scripts/<SHA-256>.js` (and `.lock`) | per-address script libraries before the database | `scripts` | Read when no `script-source:` marker names the file. Scripts keep their ids (a lone `.js` gets a stable id derived from its file name, so a second run finds it); scripts deleted in the C# client are left out. `.lock` files are ignored. |
| `directory.json` | the C# directory cache before the database | none | Skipped: refetched by this client. |
| `models/room-classifier/` | the installed terrain classifier package | none | Not imported: install the package again in Map tools > Room terrain inference (this client's classifier is an opt-in build feature). |
| Keychain, service `net.wandur.world-login`, account = C# `PasswordVault.Key` | saved world passwords | Keychain, service `net.wandur.rust.world-login`, account = this client's `vault::key` | Copied only when asked (in-app box, or `--include-passwords`), after the database commit. The C# key is SHA-256 of the C# identity JSON as `System.Text.Json` writes it (escaping included; tested against keys printed by the C# code). Only for a world whose password reference here is the C# one. The C# entry is never changed. Windows: target `Wandur/world-login/<key>`; Linux: `secret-tool` attributes `application net.wandur.client credential <key>`. |
| Keychain, `agent-<SHA-256>` accounts in the same service | agent API keys | the same account in this client's service | Copied only when asked, for an imported (or identical) agent profile; the key derivation is the same in both clients. |

Not stored by the C# client, so nothing to import: the window layout (C#: not saved), input
history, thumbnails.

## The report

Kinds: saved worlds, world addresses, connections logged, preferences, colour schemes, other
settings, scripts, macros, channel rules, agent settings, maps, map rooms, map exits, history
sessions, history lines, saved passwords, agent API keys, other records. For each: found,
imported, already there, and skipped by reason. Reasons: kept this client's version, fails this
client's checks, pattern syntax, address taken, deleted here, library full, not in the C#
credential store, credential store failed, passwords not asked for, own install id, C# releases,
no counterpart. The report never holds content, and a test checks the command line's output for
every name, address, account and text of the fixture.

## Fixtures

`crates/wandur-core/tests/fixtures/csharp-data/` holds two synthetic C# data directories made by
the C# client's own stores (so the schema and every JSON payload are exactly what the C# client
writes), with the generator beside them:

- `main/wandur.db` (schema 7, C# main at 04a77a9): four worlds (an edited address, TLS with
  Latin-1 and a world theme, an IPv4 and an IPv6 address), usage, a custom colour scheme and other
  preferences, a library with the starter, a script and four macros, a deleted script, a pack
  script opened by its `:True` key, two agent profiles (one with an API key reference, no key),
  two maps (areas, an alias, tombstones, a locked and edited room, a far-parked room, a locked
  door, a drawn exit), four history sessions (one deleted, private markers).
- `schema9/wandur.db` (the Mudlet branch, 621672e): a Lua script with the Mudlet layer and a
  script imported from Mudlet.

To regenerate: copy `generator/main` into an exported copy of the C# repository as
`tools/FixtureGen` and run `dotnet run --project tools/FixtureGen -- <out dir>` (and the same with
`generator/schema9` in an export of `feature/mudlet-import`). Never point it at real data.

## For the person running the real import

1. Quit the C# client and the Rust client.
2. Try it without writing anything:
   `wandur --import-csharp "$HOME/Library/Application Support/Wandur" --dry-run`
3. Import for real, either in the app (File > Import from Wandur (C#)..., passwords included by
   default, macOS asks per entry) or with the same command without `--dry-run` (add
   `--include-passwords` to copy passwords; macOS asks per entry).
4. Running it again is safe. To start over, quit the Rust client and remove its data directory
   (`~/Library/Application Support/Wandur-Rust`); the C# data is untouched either way.
5. History older than this client's retention (the imported setting, 30 days by default in C#)
   is pruned when the Rust app starts, as it would be in the C# client.
6. A large history makes the import's single transaction long; recording in open sessions of
   the Rust app may pause with a notice meanwhile. Importing with no sessions open avoids it.
7. If the process is killed mid-import, a copy of the C# database may remain in the temporary
   folder (`wandur-csharp-import-*` under `$TMPDIR`); it is safe to delete.
