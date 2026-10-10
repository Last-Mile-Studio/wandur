# Wandur (Rust prototype)

An exploratory Rust rebuild of the Wandur MUD desktop client with egui, eframe and egui_dock.
It is a parallel prototype: the C# client in `../wandur-client` remains the real client and the
reference. Requirements are in [docs/brief.md](docs/brief.md); the plan and findings are in
[docs/inventory.md](docs/inventory.md), [docs/parity.md](docs/parity.md),
[docs/architecture.md](docs/architecture.md), [docs/baseline.md](docs/baseline.md) and
[docs/measurements.md](docs/measurements.md), with the reports in
[docs/milestone-1.md](docs/milestone-1.md), [docs/milestone-2.md](docs/milestone-2.md),
[docs/milestone-3.md](docs/milestone-3.md) and [docs/milestone-4.md](docs/milestone-4.md), and the
overall assessment in [docs/verdict.md](docs/verdict.md).

Status: Milestone 4 (performance) done: the grid's text drawn as one reused mesh, pin and
auto-hide for the tool panels, measured against the C# client. Milestone 3 (workspace and UI): The C# client's workspace (Workspace panel, sessions and
Find a MUD in the middle, Map over Channels on the right, status bar) in egui_dock with a saved
layout; saved worlds; the world directory with artwork; GMCP channels; an initial map; the C#
preset themes. Underneath, Milestone 2's core: telnet and TLS sessions, an alacritty_terminal grid
per session, reconnect, prompts, settings.

## Run

The project pins its own Rust version (`rust-toolchain.toml`); rustup installs it on first use
without touching the machine default.

```sh
cargo build --release

# A local test MUD (generated text; 0 = idle prompts, or a byte rate such as 100000)
./target/release/wandur-bench mud-server --port 4400 --rate 3000
# A small GMCP world for the Channels and Map panels
./target/release/wandur-bench mud-server --port 4403 --rate 0 --page gmcp
# The Unicode test page
./target/release/wandur-bench mud-server --port 4401 --rate 0 --page unicode
# A local directory with 300 worlds and 4000x3000 artwork (--varied for different genres and counts)
./target/release/wandur-bench directory-server --port 4402 --worlds 300 --varied

# The client against the local directory, connecting at start
WANDUR_DIRECTORY_URL=http://127.0.0.1:4402 ./target/release/wandur 127.0.0.1:4403 --data-dir /tmp/wandur-try
```

Options: `wandur [HOST:PORT | tls://HOST:PORT]... [--data-dir DIR] [--scrollback ROWS]
[--font-size POINTS] [--output-fps N] [--theme NAME] [--directory-url URL]
[--show directory|settings|world:ID|unpinned:PANEL] [--window-size WxH] [--renderer wgpu|glow]
[--scene NAME|list]` (`glow` needs `--features glow`). Settings, the layout, the client database
(`wandur.db`), the directory cache and thumbnails are kept in
`--data-dir`, or in the platform data directory under `Wandur-Rust`; command line values apply to
one run only.

Scenes for screenshots: `wandur --scene list` names them; `wandur --data-dir D --scene NAME` opens
one in a window, set up in memory (nothing is saved); with `WANDUR_SCREENSHOT=shot.png` it is
rendered headless at 1300 by 820 (the C# reference size) and saved. `scripts/capture-scenes.sh
OUT_DIR [SCENE...]` captures them all against loopback bench servers it starts and stops
(`wandur-bench mud-server --page lantern`, `wandur-bench directory-server --fixture`).

The directory address is, in order: `WANDUR_DIRECTORY_URL`, `--directory-url`, the setting
(Settings, World directory), else the public `https://api.wandur.net/`. The client fetches it the
first time Find a MUD is shown and at most every five minutes after, with the User-Agent
`WandurRustPrototype/<version> (<os>; <arch>)` and no install id.

## Develop

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
cargo run --release -p wandur-bench -- micro --label mine
# Whole-app headless frames with per-panel time and allocation (sessions on a loopback server)
cargo run --release -p wandur-bench -- shell --label mine
# Memory per terminal, release at the scrollback bound
cargo run --release -p wandur-bench --example memprobe
```

Unpin a tool panel (Workspace, Map, Channels) from its tab's context menu or View, Pinned: it
moves to a strip on its edge; hover or click the strip tab to slide it out, Escape or Hide to put
it away, Pin to dock it where it was. `--show unpinned:map` slides an unpinned panel out at start.

Layout: `crates/wandur-core` (no UI: transports, telnet, protocols, prompts, sessions,
reconnect, settings, directory, channels, map), `crates/wandur-term` (the terminal grid, alacritty_terminal),
`crates/wandur-app` (egui presentation and the `wandur` binary), `crates/wandur-bench` (loopback
MUD and directory servers, the GMCP demo world, micro measurements).

## License

MIT, see [LICENSE](LICENSE). The bundled JetBrains Mono fonts are under the SIL Open Font
License 1.1 (`crates/wandur-app/assets/fonts/JetBrainsMono-OFL.txt`).
