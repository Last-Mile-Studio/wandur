# The Mudlet compatibility layer

`crates/wandur-core/src/scripting/lua/mudlet.lua` gives a Lua script marked "Mudlet names" the
functions Mudlet scripts call (`send`, `tempTrigger`, `gmcp`, `registerAnonymousEventHandler` and
the rest), mapped onto Wandur's own host API, the `mud` table every script has.

## Where it came from

- It is a port of `src/Wandur.Core/Scripting/Lua/mudlet.lua` on the C# client's unmerged
  `feature/mudlet-import` branch (commit 621672e). That file belongs to the same owner and is
  MIT licensed, like this repository.
- The C# file was written for the client from Mudlet's public documentation of what each
  function does (the Mudlet manual and its function reference). It contains no code from Mudlet.
  Mudlet is GPL-3.0; none of its source was read or copied for the layer, here or in C#.
- The port keeps the C# behaviour line for line. Three things changed: the messages say
  "not supported in Wandur yet" where C# says "WMC", the host helper table is `__wandur` (C#
  `__wmc`), and the header names this document. The host helpers it calls (`literal`, `perl`,
  `compile`) are Rust ports of the C# `MudletRegex` and `LuaMudletLayer` (owner's code).
- The importer (`crates/wandur-core/src/mudlet`) is a port of the same branch's `Import` folder
  (`MudletSourceReader`, `MudletPackageParser`, `MudletPlainSend`, `MudletRegex`,
  `MudletConverter`, `MudletImporter`, `MudletImportSummary`, `MudletLuaWrapper`). It reads
  Mudlet's XML format, a file format, from its documented structure; it runs none of Mudlet's
  code. The two test fixtures (`crates/wandur-core/tests/fixtures/mudlet`) are the C# branch's
  hand-written fictional files (The Lantern Road; Wren, Odo and Bastian).

## Rules for changing it

- Add a Mudlet function only from Mudlet's public documentation of its behaviour, never from
  Mudlet's source or from a GPL-licensed port of it. Note the documentation page in the commit.
- A name Mudlet has and Wandur does not stays in the unsupported list, so a script says what it
  is missing instead of failing silently.
- Everything the layer does goes through `mud`, so it keeps the host API's limits: the layer
  adds no capability a native Lua script lacks, except compiling code given as text (Lua source,
  never bytecode) into the script's own sandbox, under the script's own limits.

## What it covers

`send`, `sendAll`, `echo`, `cecho`, `decho`, `hecho` (colour markup removed), `display`,
`matches`, `multimatches`, `line`, `tempTrigger`, `tempRegexTrigger`, `tempBeginOfLineTrigger`,
`tempExactMatchTrigger`, `tempPromptTrigger`, `tempAlias`, `tempTimer`, `killTrigger`,
`killAlias`, `killTimer`, `enableTrigger`, `disableTrigger`, `enableAlias`, `disableAlias`,
`enableTimer`, `disableTimer`, `expandAlias` (this script's aliases, else sent as typed), the
`gmcp` and `msdp` tables with their `gmcp.*` and `msdp.*` events, `registerAnonymousEventHandler`,
`killAnonymousEventHandler`, `raiseEvent` (within the script), `loadstring` and `load` for text,
the Lua 5.1 names `unpack`, `table.getn`, `math.mod`, `string.gfind`, and the helpers
`string.split`, `string.trim`, `string.starts`, `string.ends`, `table.contains`, `table.size`,
`table.is_empty`, `table.keys`, `table.index_of`, `getEpoch`. Items an import kept for conversion
register through `__mudlet.trigger`, `alias`, `timer` and `script` when the person chooses
Run as Lua.
