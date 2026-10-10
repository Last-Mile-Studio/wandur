# Packaging

Unsigned downloads of the Rust client, made by scripts in `scripts/`. Nothing is uploaded,
signed or notarized; the owner runs releases.

| Platform | Script | Output | Status |
|---|---|---|---|
| macOS | `scripts/package-macos.sh [--version X] [--check-updates-as X]` | `target/package/macos/Wandur Rust.app` and `Wandur-Rust-X-macos-<arch>.dmg` | Built and started on macOS (arm64): the bundle's executable, the executable inside the mounted image, and the bundle through LaunchServices (`open`) |
| Linux | `scripts/package-linux.sh [--version X] [--target T]` | `target/package/linux/Wandur-Rust-X-linux-<arch>.tar.gz` with `wandur`, `install-desktop-entry.sh`, the `.desktop` template, the icon and the licence | Untested: no Linux target is installed for the pinned toolchain here. `--check` runs `cargo check --target` once one is. The menu entry installer was dry-run on macOS (quoting of an odd path) |
| Windows | `scripts/package-windows.sh [--version X] [--target T]` | `target/package/windows/Wandur-Rust-X-windows-<arch>.zip` with `wandur.exe` and the licence | Untested: no Windows target is installed here. `wandur.exe` has no icon resource yet |

`scripts/make-icons.sh` derives the icons from `crates/wandur-app/assets/icons/icon-1024.png` (the
C# client's master icon) into `target/package/icons`, as the C# script does (Pillow when it is
there, `sips` and `iconutil` for the `.icns`).

## Names

The Rust client installs beside the C# client: the bundle is `Wandur Rust.app` with the
identifier `net.wandur.client.rust`, the Linux entry `net.wandur.client.rust`, and the data
folder `Wandur-Rust` (see `settings::default_data_dir`).

## Versions and update checks

`--version` only labels the package. The executable is a build from source (`0.0.0-dev`) and
never asks for updates, unless `--check-updates-as X` builds it as release X
(`WANDUR_RELEASE_VERSION`), after which it asks the configured directory's `/client/latest` at
most once a day. Do not use it until wandur.net lists releases of this client: today
`/client/latest` describes the C# client.

## Differences from the C# packaging

- The disk image is made with `hdiutil` alone: the C# client's `dmgbuild` layout (window
  background, icon positions) needs a Python package download.
- No ad-hoc `codesign` of the bundle; the arm64 executable keeps the ad-hoc signature the
  linker gives it, which is what Apple Silicon needs to start it.
- One executable, no runtime folder: the Linux and Windows downloads hold a single binary.
