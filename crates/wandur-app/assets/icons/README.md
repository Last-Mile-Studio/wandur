# App icon

`icon-1024.png` is the C# client's master icon (`wandur-client`, same owner, MIT), copied
unchanged from `src/Wandur.Desktop/Assets/icon-1024.png` at 27f66d2. `scripts/make-icons.sh`
derives the packaging icons from it (the macOS `.icns`, a 256 pixel PNG for Linux, a `.ico` for
Windows) into `target/package/icons`; none of those is committed.
