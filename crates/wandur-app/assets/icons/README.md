# App icon

`icon-1024.png` is the C# client's master icon (`wandur-client`, same owner, MIT), copied
unchanged from `src/Wandur.Desktop/Assets/icon-1024.png` at 27f66d2. `scripts/make-icons.sh`
derives the packaging icons from it (the macOS `.icns`, a 256 pixel PNG for Linux, a `.ico` for
Windows) into `target/package/icons`; none of those is committed.

`about-icon-160.png` is `icon-1024.png` scaled to 160 pixels with `sips -Z 160`, the logo in
Help > About Wandur (80 points at twice the pixels).

`app-icon-512.png` is the window and Dock icon (`src/app_icon.rs`): `icon-1024.png` cropped to a
square on a dark rounded square (824 of 1024 pixels, radius 185, Apple's grid), then 512 pixels.
Without it eframe shows its own icon in the Dock and taskbar.
