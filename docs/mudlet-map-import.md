# Importing Mudlet maps

File > Import map... (and the full map's Import, with nothing selected) reads three kinds of
file:

- **This client's map file** (`{"Version":1|2,"Map":{...}}`, or a bare map): it replaces the
  world's map, as the C# client's import does.
- **Mudlet's JSON map export** (Mudlet's `saveJsonMap(path)`, or the export in Mudlet's map
  settings): it is merged into the world's map.
- **A Mudlet Mapping Protocol XML map** (MMP, the `.xml` map a game publishes and announces with
  GMCP `Client.Map`): it is merged into the world's map too. See "XML maps (MMP)" and
  "Official maps" below.

Mudlet's binary map (`.dat`) is recognised (by its extension, or by a file that starts with a
small big-endian number instead of JSON) and refused with a note, in the person's language,
saying that Wandur reads the JSON export and how to make one in Mudlet.

The dialog asks which world the map goes into (the active session's world first; saved worlds
with no session open after it), reads the file on a worker thread with progress, and shows a
summary before anything changes: counts per kind, and what was left out or changed and why.
Import applies it as one undoable step; the undo toast offers it back.

Code: `crates/wandur-core/src/map/mudlet.rs` (reading and converting),
`RoomMapTracker::import_map` (merging as one undo step), `crates/wandur-app/src/map_import.rs`
(the dialog). Fixture: `crates/wandur-core/tests/fixtures/mudlet/lantern-road-map.json`
(hand-written, fictional, with a 32 by 32 picture drawn by a script).

## Clean room

Mudlet is GPL-3.0. None of its source was read, copied or translated, and nothing was fetched
from the internet while this was written. The reader is built from what Mudlet's public
documentation says about the JSON export and its Lua mapper API (rooms with ids, names,
coordinates, areas, environments, exits and special exits, doors, locks, weights, user data,
symbols; labels with text, colours and pictures), and is tolerant where that documentation
leaves the exact shape open. The fixture was written by hand from those descriptions.

## Assumptions about the format

Each of these is a guess at a detail the documentation does not pin down. The reader accepts
the alternatives listed, ignores fields it does not know, and treats every field as optional.

1. **Top level**: an object with `areas` (a list). `customEnvColors` (also `envColors`,
   `environmentColors`) and `userData` are read; anything else (`formatVersion`,
   `defaultAreaName`, `playerRoomId`, `mapSymbolFontDetails`...) is ignored. A file with
   `Version` or `Rooms` at the top is this client's file.
2. **Areas**: `id`, `name`, `rooms`, `labels`, `gridMode`, `userData`. The area with id `-1`
   (Mudlet's default area) becomes rooms with no area; an empty name too.
3. **Rooms**: `id` (a number or a string), `name`, `coordinates` as `[x, y, z]` (also `coords`,
   `position`, an `{x, y, z}` object, or `x`, `y`, `z` fields), `environment` (also `env`),
   `exits`, `specialExits`, `stubExits` (also `stubs`), `doors`, `exitWeights`, `exitLocks`,
   `userData`, `hash` (also `roomHash`), `symbol` (also `char`; a string or an object with
   `text`), `weight`, `locked` (also `isLocked`), `customLines`. `null` lists are empty lists.
4. **Exits**: a list of objects `{name, exitId, door, weight, locked, customLine}` (the name
   also as `direction`, `dir`, `command`, `cmd`; the destination also as `id`, `to`, `target`,
   `roomId`, `destination`), or an object of name to destination id. A name that is a
   compass direction, `up`, `down`, `in` or `out` (or their short forms) is a normal exit; any
   other name is a special exit whose name is its command. `specialExits` takes the same
   shapes and is always special. An exit to `-1`, `0` or nowhere is Mudlet's mark of an
   unexplored exit (a stub).
5. **Doors**: `0` none, `1` open, `2` closed, `3` locked (the same numbers as this client's),
   or the words `open`, `closed`, `locked`. On the exit, or in the room's `doors` table by exit
   name.
6. **Weights**: an exit's weight `0` means the destination room's weight, as in this client;
   a room's weight defaults to 1. Exit weights also from the room's `exitWeights` table; exit
   locks from `exitLocks` (a table or a list of names).
7. **Locks**: Mudlet's room lock keeps a room out of routes; it becomes this client's room lock
   (which also keeps it out of routes and in place in the editor).
8. **Environments**: Mudlet's default environment colours 1 to 16 are taken as the ANSI colours
   (1 red, 2 green, 3 yellow, 4 blue, 5 magenta, 6 cyan, 7 light grey, 8 dark grey, then the
   bright ones; 15 white, 16 grey). `customEnvColors` overrides them: a list of `{id, color}`
   (the colour as `color32RGBA`, `color24RGB`, `[r, g, b(, a)]`, `{r, g, b(, a)}`, `#RRGGBB` or
   Qt's `#AARRGGBB`) or an object of id to colour. The environment becomes the room's colour;
   the terrain is left empty (Mudlet's environments are numbers without meaning), so the
   room's colour wins over any terrain guess.
9. **The way up**: Mudlet's mapper API puts north at +Y, as this map does, so coordinates are
   used as they are. Because a file could still be the other way up, the exits are checked:
   when more north (north-east, north-west) and south exits between rooms of one floor point
   the wrong way than the right way, Y is flipped for every room. The summary says so.
10. **Labels**: each area's `labels`, objects with `id`, `coordinates` (as rooms; also
    `position`, `pos`, or `x`, `y`, `z`), `size` as `[width, height]` (also an object or
    `width`, `height` fields) in room cells, `text`, colours (`fgColor`, `foregroundColor`,
    `foreground`, `textColor`, `color`, or the first of `colors`; `bgColor`,
    `backgroundColor`, `background`, or the second of `colors`), `image` (base64 PNG or JPEG,
    one string or a list of strings joined; also `pixmap`), `showOnTop` (also `onTop`),
    `noScaling` (also `fixedSize`), `fontSize` or `font.size`.
    - The position is the label's top-left corner. Labels may be stored with Y the other way
      from the rooms; for each area the reader takes the way that puts more labels over the
      area's rooms (a tie goes to the rooms' way).
    - A label with text is a text label (its picture, if any, is taken to be Mudlet's own
      drawing of the text and is not used); without text and with a readable picture it is a
      picture label (scaled down to 2 MiB and 2048 pixels a side when larger). A background
      with alpha 0 is no background. The text size, when not given, is 60% of the label's
      height (96 points a cell) divided by its lines.
11. **User data**: a room's `userData` entries become its notes, one `key: value` line each,
    except a `description` entry, which becomes the room's description (Mudlet keeps no
    descriptions of its own). Area and map user data are not kept.
12. **Ids**: a room with a `hash` (Mudlet's room hash, the server's room id a mapper script
    recorded) becomes `s:<hash>` with that server id, so it lines up with GMCP tracking of the
    same world; other rooms become `mudlet:<id>`. Labels become `mudlet:label:<area>:<id>`.
    A hash shared by two rooms, or longer than 120 characters, is not used. The same file
    always gives the same ids, so importing it again changes nothing (items already exactly as
    imported are skipped, and no undo step is made).

## What is kept, left out or changed

Mapped: areas, rooms, coordinates, area grid mode, environment colours, exits (compass, up,
down, in, out), special exits (an exit named by its command, lower case, the command kept as
written), doors, exit locks and weights, room locks and weights, symbols, stubs (as known
exits), user data (notes and description), labels with text or a picture.

Left out, each counted in the summary with its reason:

- special exits whose command starts with `script:` (they run Lua in Mudlet);
- exits to rooms that are not in the file;
- a second exit of a room by the same direction or command;
- rooms without a usable id or position;
- custom exit lines (the exits are drawn straight);
- symbol colours (symbols take the map's colour);
- labels that keep their size when zooming (they scale with the map);
- labels with neither text nor a readable picture, or without a position or size;
- pictures that cannot be read, or past the map's 20 MiB of pictures, or labels past 5,000;
- area and map user data.

## Limits

The map's own: 10,000 rooms and 60,000 exits. A file past them, or one that would take the
world's map past them, is refused with the numbers and nothing is imported (no partial maps).
Files are read up to 64 MiB.

## Merging into a world's map

An imported room replaces the map's room with the same id but keeps what the tracker observed
of it (the server's name, description and area for recognition, a terrain guess, the known
exits), and keeps its description when the import has none. Rooms, exits and labels of the map
that the file does not have stay. The player's position is kept. All of it is one undo step.

## XML maps (MMP)

Code: `crates/wandur-core/src/map/mudlet/xml.rs`. Fixture:
`crates/wandur-core/tests/fixtures/mudlet/ember-vale-map.xml` (hand-written, fictional).

The XML is read into the same model as Mudlet's JSON export and converted by the same code, so
everything above (ids, environment colours, special exits, doors, the way up, the summary, the
limits, merging) applies. Sources: the public Mudlet wiki pages "Standards:MMP" (the layout),
"Manual:GMCP Extensions" (`Client.Map`) and "Manual:Mapper Functions" (`loadMap` reads `.xml`
maps). No Mudlet source was read.

The layout the wiki documents: `<map>` holds `<areas>` (`<area id name>`), `<rooms>` (`<room id
area title environment>` with a `<coord x y z>` and `<exit direction target>` children) and
`<environments>` (`<environment id name color>`). Assumptions where the wiki is silent:

1. **Ids**: a room's id is the game's own room number, the one GMCP `Room.Info` gives as `num`
   (true of the games that publish these maps). It becomes `s:<id>`, so imported rooms line up
   with tracking. Without that, two maps would never meet.
2. **Environment colours**: `color` is an ANSI colour number (0 black, 1 to 15 the normal and
   bright colours as Mudlet's defaults above), or a 256 colour palette number. An
   `htmlcolor="#RRGGBB"` attribute, when present, wins.
3. **Exits**: `direction` takes full or short compass names, `up`, `down`, `in`, `out`; any other
   direction is a special exit whose name is its command. Optional `door` (a number as above or
   a word), `weight` and `locked` attributes are read when present. Exits to rooms the file does
   not have are left out and counted.
4. **Areas**: rooms whose `area` the file does not name get an area of their own with no name.
   A second area with the same id keeps the first name.
5. **Labels**: the documented format has none, so an XML map brings no labels.
6. **Element names** compare without case and namespace prefixes; elements and attributes the
   format does not name are ignored.

Safety: a file that declares a DTD is refused; an entity other than the five built-in ones and
character references is refused; nesting deeper than 16 elements is refused. Limits, checked
while reading: 64 MiB, 10,000 rooms and 60,000 exits (the map's own, refused with the counts),
10,000 areas, 10,000 environments and 256 exits per room.

## Official maps

A game can name its official map with GMCP `Client.Map {"url": "https://..."}` (the public
Mudlet wiki, "Manual:GMCP Extensions", Automatic map download). The session then shows a strip:
"This game offers an official map." with Download, Not now and Never.

- **Never** is saved for the world; the strip does not come back for it.
- **Not now** hides it for the rest of the session.
- When a file was imported before, the client asks the server first (with the file's ETag as
  `If-None-Match`, or, when the server gave no ETag, its `Last-Modified` as
  `If-Modified-Since`); the strip shows only when the file changed. The server is asked at most
  once every 24 hours per world; within that time the last check's answer stands (a change it
  found is still offered, without asking again). A new address is checked at once, without
  validators; the same file moved there is not news, and the record follows it.
- **Download** fetches the file on a worker thread: https only (an http address is refused), at
  most 3 redirects and each to https (another host is fine), 64 MiB at most counted while it
  streams, 15 seconds to connect and 5 minutes in all. The file is read as an MMP XML map or
  Mudlet's JSON export (as File > Import map reads them) and merged into the session's map as
  one undo step, once the session's saved map has loaded. The toast says how many rooms, exits
  and labels were added, updated, kept with your edits and removed, and offers Undo.

The file is kept at `<data dir>/official-maps/<world>/map.xml` (`map.json` for a JSON map),
with `meta.json` beside it: the address, the ETag and Last-Modified, the SHA-256, when it was
imported, the last merge's counts, Never, and the last check (`last_checked_at`, its address,
and whether it found a change). `<world>` is the saved world's id, or `endpoint-<host>-<port>` for a
session opened by address (only letters, digits, `-` and `_`).

Merging a new version is three-way: the base is the file kept last time, theirs the new file,
ours the world's map now (the base is the last imported file even when the address changed).
Item by item (rooms by id, exits by room and direction, labels by id):
untouched here since the base takes the new version (or is removed when the new file dropped
it); changed or deleted here keeps your version; new in the file is added. What the tracker
records on its own (the server's words for a room, known exits, a terrain guess, an exit seen
from both ends) is not an edit. A room the map learned by walking under an id the file now has
takes the file's version. The first download, with no base, is a plain merge, as above.

Code: `crates/wandur-core/src/map/official.rs` and its folder (address, download, store, offer,
merge), `crates/wandur-app/src/official_map.rs` (the strip and the workers). To try it by hand:
`wandur-bench mud-server --port 4403 --rate 0 --page gmcp --client-map https://.../map.xml` names
the address; `crates/wandur-core/tests/fixtures/mudlet/lantern-town-map.xml` is a map of that
world's rooms to put on an https address.

Not done: an Undo of the merge does not forget the kept file, so the next new version is merged
as if the person had deleted what the undo took away (it stays away).
