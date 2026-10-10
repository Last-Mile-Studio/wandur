# UI string tables

- `Strings*.resx` are copies of the C# client's tables (`src/Wandur.Core/Localization` in
  wandur-client, same owner, MIT), taken at 731b699 (earlier at 27f66d2). Both clients share their keys, wording and
  translations. Refresh them with
  `python3 scripts/generate-localization.py --csharp <wandur-client>/src/Wandur.Core/Localization`.
- `Rust*.resx` hold the strings only this client has, in the same five languages, and (at
  their end, from `ScriptImportInvalid` to `MudletRunAsLuaHint`) the 93 strings the C# client's
  unmerged `feature/mudlet-import` branch adds at 621672e, with its translations, for Lua and
  the Mudlet import. They stay here, not in `Strings*.resx`, so those keep matching C# main;
  move them over if the branch is merged.

Languages: English (base file), Spanish (`.es`), French (`.fr`), German (`.de`) and Brazilian
Portuguese (`.pt-BR`). Every file of a table must have exactly the base file's keys.

After editing a table, run `python3 scripts/generate-localization.py`; it writes
`crates/wandur-core/src/l10n/generated.rs`. `--check` fails when that file is stale (a core test
runs it).
