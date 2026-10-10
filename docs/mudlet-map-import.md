# Importing Mudlet maps

File > Import map... (and the full map's Import, with nothing selected) reads two kinds of
file:

- **This client's map file** (`{"Version":1|2,"Map":{...}}`, or a bare map): it replaces the
  world's map, as the C# client's import does.
- **Mudlet's JSON map export** (Mudlet's `saveJsonMap(path)`, or the export in Mudlet's map
  settings): it is merged into the world's map.

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
