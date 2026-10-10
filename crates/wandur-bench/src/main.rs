//! `wandur-bench`: loopback servers and in-process measurements for the Rust prototype.
//!
//! - `wandur-bench mud-server [--port 4400] [--rate BYTES_PER_SECOND] [--nochat] [--page unicode] [--write-ms 100] [--tick-ms N]`
//! - `wandur-bench mud-server --page gmcp` (a small GMCP world: rooms for the Map, channel talk)
//! - `wandur-bench mud-server --page jedi` (one Legends of the Jedi room and nothing more: the C#
//!   `world-theme-session` capture)
//! - `wandur-bench mud-server --page lotj --rate 1000000` (the flood with a Legends of the Jedi-like
//!   room every 20 lines: GMCP `Room.Info` without a description, before or after its text)
//! - `wandur-bench mud-server --page lantern` (The Lantern Road of the C# reference captures: a
//!   21-room map, guild and other channels, vitals, MSSP and the showcase transcript)
//! - `wandur-bench mud-server --page login` (Starfall Reach's name and password screen; the
//!   fixture password is `starfall-test-only`), or `--page login-gmcp` (GMCP `Char.Login` first)
//! - `wandur-bench mud-server --page msdp` (Dustfall Station: MSDP vitals and fights, one
//!   variable per subnegotiation; commands kill, ambush, hit, won, flee, rest)
//! - `wandur-bench directory-server [--port 4402] [--worlds 300] [--varied] [--art-size 4000x3000]`
//! - `wandur-bench directory-server --fixture` (the four fictional worlds of the C# captures)
//! - either directory server takes `--latest VERSION` (what `/client/latest` answers) and
//!   `--log FILE` (every request head appended)
//! - `wandur-bench model-server [--port 4410] [--delay-ms N] [--action ID]` (a stand-in LM Studio:
//!   both the OpenAI-compatible and the native API, answering every decision with `ID`)
//! - `wandur-bench micro [--label NAME]` (run with `--release`)
//! - `wandur-bench shell [--label NAME] [--frames N] [--only TEXT]` (whole-app headless frames, `--release`)
//!
//! App-level scenarios are driven by the C# harness (see docs/measurements.md).

#[cfg(test)]
mod a11y_tests;
mod directory_server;
mod generator;
mod gmcp_demo;
mod lantern;
mod login;
mod lotj_page;
mod micro;
mod micro_agent;
mod micro_dir;
mod micro_history;
mod micro_map;
mod micro_panels;
mod micro_shell;
mod model_server;
mod msdp_page;
mod mud_server;
mod unicode_page;
#[cfg(test)]
mod update_tests;

use wandur_app::sysstat::CountingAllocator;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn option(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// `--latest VERSION` (what `/client/latest` answers) and `--log FILE` (every request head).
fn configure_directory(server: &directory_server::DirectoryServer, args: &[String]) {
    if let Some(version) = option(args, "--latest") {
        server.set_latest("200 OK", directory_server::latest_json(&version));
    }
    if let Some(file) = option(args, "--log") {
        server.log_to(std::path::PathBuf::from(file));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("mud-server") => {
            let port = option(&args, "--port").and_then(|p| p.parse().ok()).unwrap_or(4400);
            let rate = option(&args, "--rate").and_then(|p| p.parse().ok()).unwrap_or(0);
            let chat = !args.iter().any(|a| a == "--nochat");
            if let Some(ms) = option(&args, "--tick-ms").and_then(|v| v.parse().ok()) {
                mud_server::TICK_MS.store(ms, std::sync::atomic::Ordering::Relaxed);
            }
            if let Some(ms) = option(&args, "--write-ms").and_then(|v| v.parse().ok()) {
                mud_server::WRITE_INTERVAL_MS.store(ms, std::sync::atomic::Ordering::Relaxed);
            }
            let page = match option(&args, "--page").as_deref() {
                None => None,
                Some("unicode") => Some(unicode_page::unicode_page()),
                Some("jedi") => {
                    mud_server::QUIET_PAGE.store(true, std::sync::atomic::Ordering::Relaxed);
                    Some(mud_server::jedi_page())
                }
                Some("lotj") => {
                    mud_server::LOTJ_ROOMS.store(true, std::sync::atomic::Ordering::Relaxed);
                    None
                }
                Some("gmcp") => {
                    mud_server::GMCP_DEMO.store(true, std::sync::atomic::Ordering::Relaxed);
                    None
                }
                Some("lantern") | Some("login") | Some("login-gmcp") | Some("msdp") => Some(String::new()),
                Some(other) => {
                    eprintln!(
                        "unknown page {other} (try: unicode, gmcp, lantern, login, login-gmcp, msdp, jedi, lotj)"
                    );
                    std::process::exit(2);
                }
            };
            let lantern = option(&args, "--page").as_deref() == Some("lantern");
            let login = match option(&args, "--page").as_deref() {
                Some("login") => Some(false),
                Some("login-gmcp") => Some(true),
                _ => None,
            };
            let msdp = option(&args, "--page").as_deref() == Some("msdp");
            let started = if msdp {
                mud_server::MudServer::start_msdp(port).map(|(server, _)| server)
            } else if let Some(gmcp) = login {
                mud_server::MudServer::start_login(port, gmcp).map(|(server, _)| server)
            } else if lantern {
                mud_server::MudServer::start_lantern(port)
            } else {
                mud_server::MudServer::start_with_page(port, rate, chat, page)
            };
            match started {
                Ok(server) => {
                    println!(
                        "Loopback MUD on 127.0.0.1:{} at {rate} bytes/s; Ctrl+C stops it.",
                        server.port
                    );
                    loop {
                        std::thread::park();
                    }
                }
                Err(e) => {
                    eprintln!("could not listen on 127.0.0.1:{port}: {e}");
                    std::process::exit(1);
                }
            }
        }
        Some("model-server") => {
            let port = option(&args, "--port").and_then(|p| p.parse().ok()).unwrap_or(4410);
            let delay = option(&args, "--delay-ms").and_then(|p| p.parse().ok()).unwrap_or(0);
            let action = option(&args, "--action").unwrap_or_else(|| "look".into());
            model_server::run(port, std::time::Duration::from_millis(delay), &action);
        }
        Some("directory-server") if args.iter().any(|a| a == "--fixture") => {
            let port = option(&args, "--port").and_then(|p| p.parse().ok()).unwrap_or(4402);
            match directory_server::DirectoryServer::start_fixture(port) {
                Ok(server) => {
                    configure_directory(&server, &args);
                    println!(
                        "Loopback directory at {} with the four fixture worlds; run the client with \
                         WANDUR_DIRECTORY_URL={} (or --directory-url). Ctrl+C stops it.",
                        server.address(),
                        server.address()
                    );
                    loop {
                        std::thread::park();
                    }
                }
                Err(e) => {
                    eprintln!("could not listen on 127.0.0.1:{port}: {e}");
                    std::process::exit(1);
                }
            }
        }
        Some("directory-server") => {
            let port = option(&args, "--port").and_then(|p| p.parse().ok()).unwrap_or(4402);
            let worlds = option(&args, "--worlds").and_then(|p| p.parse().ok()).unwrap_or(300);
            let varied = args.iter().any(|a| a == "--varied");
            let (w, h) = option(&args, "--art-size")
                .and_then(|v| {
                    v.split_once('x')
                        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                })
                .unwrap_or((4000, 3000));
            println!("Preparing {w}x{h} artwork (cached under .superpowers/perf/art)...");
            let (jpeg, png) = directory_server::artwork(std::path::Path::new(".superpowers/perf/art"), w, h);
            let json = directory_server::listing_json(worlds, varied, &directory_server::now_rfc3339());
            match directory_server::DirectoryServer::start(port, json, jpeg, png) {
                Ok(server) => {
                    configure_directory(&server, &args);
                    println!(
                        "Loopback directory at {} with {worlds} worlds; run the client with \
                         WANDUR_DIRECTORY_URL={} (or --directory-url). Ctrl+C stops it.",
                        server.address(),
                        server.address()
                    );
                    loop {
                        std::thread::park();
                    }
                }
                Err(e) => {
                    eprintln!("could not listen on 127.0.0.1:{port}: {e}");
                    std::process::exit(1);
                }
            }
        }
        Some("shell") => {
            let label = option(&args, "--label").unwrap_or_else(|| "run".into());
            let frames = option(&args, "--frames").and_then(|v| v.parse().ok()).unwrap_or(240);
            micro_shell::run(&label, frames, option(&args, "--only").as_deref());
        }
        Some("history") => {
            let mb = option(&args, "--mb").and_then(|v| v.parse().ok()).unwrap_or(10);
            let rate = option(&args, "--rate")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1_000_000);
            micro_history::run(mb, rate);
        }
        Some("micro") => {
            let label = option(&args, "--label").unwrap_or_else(|| "run".into());
            micro::run(&label);
        }
        _ => {
            eprintln!(
                "commands: mud-server [--port N] [--rate BYTES] [--nochat] [--page unicode|gmcp|lantern|login|login-gmcp|msdp|jedi|lotj] [--write-ms N] [--tick-ms N] | \
                 directory-server [--port N] [--worlds N] [--varied] [--art-size WxH] [--fixture] | micro [--label NAME] | history [--mb N] [--rate BYTES] | shell [--label NAME] [--frames N] [--only TEXT]"
            );
            std::process::exit(2);
        }
    }
}
