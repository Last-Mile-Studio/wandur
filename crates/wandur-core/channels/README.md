# Channel families

`families.json` is a copy of the C# client's shipped channel rule sets
(`src/Wandur.Core/Channels/families.json` in wandur-client, same owner, MIT licence, see the
repository `LICENSE`), taken at 27f66d2 without changes. Both clients read the same file.

It holds one ordered rule list per codebase family (`smaug`, `swr`, `diku`, `rom`, `circle`,
`lp`, `generic`). Each entry is either `{"include": "<family>"}` (the rules of another family,
inlined in place) or a rule in the shape a world profile and a channel pack carry:
`{"channel", "pattern", "reply_command", "private", "exclude", "disabled"}`. Patterns are
regular expressions over the plain (colour stripped) line, anchored at its start, matched
without regard to case; the optional named groups `speaker` and `text` say who spoke and what
was said.

`wandur-core` embeds the file at build time (`src/channels/families.rs`). Refresh it by copying
the C# file again and running `cargo test -p wandur-core channels`.
