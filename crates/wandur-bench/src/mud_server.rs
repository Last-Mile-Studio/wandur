//! A loopback-only stand-in for a MUD, behaving like the C# bench's `MudServer`: it accepts telnet
//! connections on 127.0.0.1, ignores what the client sends, and streams generated ANSI text at a
//! fixed byte rate in ten writes a second (or every `--write-ms`). Rate 0 means a greeting and then a prompt every two
//! seconds, which is what an idle session sees.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::generator::AnsiGenerator;

/// Milliseconds between writes at a fixed rate (`--write-ms`). The default 100 matches the C#
/// bench (ten bursts a second); a small value gives a steady stream, as a busy real MUD does.
pub static WRITE_INTERVAL_MS: AtomicU64 = AtomicU64::new(100);
/// Ticker mode (`--tick-ms N`): one short numbered line every N milliseconds, stamped with the
/// server's send time, so a client that shows output in batches visibly prints lines in clumps.
pub static TICK_MS: AtomicU64 = AtomicU64::new(0);
/// The ticker's shared start time, set when the first client connects.
static TICK_CLOCK: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Milliseconds since the ticker's shared start (starting it if this is the first client).
fn clock_ms_now() -> u64 {
    TICK_CLOCK.get_or_init(Instant::now).elapsed().as_millis() as u64
}
/// Serve the GMCP demo world (`--page gmcp`) instead of generated text.
pub static GMCP_DEMO: AtomicBool = AtomicBool::new(false);
/// Offer GMCP and put a Legends of the Jedi-like room (`Room.Info` without a description, and
/// its text) every few lines of the flood (`--page lotj`, the shell bench's LotJ scenario).
pub static LOTJ_ROOMS: AtomicBool = AtomicBool::new(false);
/// Send the page alone, with no bench greeting and nothing after it (`--page jedi`).
pub static QUIET_PAGE: AtomicBool = AtomicBool::new(false);

/// The Legends of the Jedi greeting of the C# `world-theme-session` capture: one room, then
/// nothing more.
pub fn jedi_page() -> String {
    "Welcome to Legends of the Jedi.\r\n\r\n\x1b[36mDOCKING CONTROL\x1b[0m\r\n\
     The observation deck overlooks a quiet field of stars.\r\n\
     Amber markers guide a freighter toward the lower berths.\r\n\r\n\
     Exits: \x1b[32mnorth\x1b[0m, \x1b[32meast\x1b[0m, \x1b[32mdown\x1b[0m\r\n"
        .to_string()
}

pub struct MudServer {
    pub port: u16,
    stop: Arc<AtomicBool>,
    sent: Arc<AtomicU64>,
}

impl MudServer {
    /// Listen on 127.0.0.1:`port` (0 picks a free port).
    pub fn start(port: u16, bytes_per_second: u64, chat: bool) -> std::io::Result<MudServer> {
        Self::start_with_page(port, bytes_per_second, chat, None)
    }

    /// Like [`Self::start`], sending `page` after the greeting.
    pub fn start_with_page(
        port: u16,
        bytes_per_second: u64,
        chat: bool,
        page: Option<String>,
    ) -> std::io::Result<MudServer> {
        Self::start_inner(port, bytes_per_second, chat, page, false)
    }

    /// Serve The Lantern Road (`--page lantern`) on 127.0.0.1:`port`.
    pub fn start_lantern(port: u16) -> std::io::Result<MudServer> {
        Self::start_inner(port, 0, false, None, true)
    }

    fn start_inner(
        port: u16,
        bytes_per_second: u64,
        chat: bool,
        page: Option<String>,
        lantern: bool,
    ) -> std::io::Result<MudServer> {
        let page: Arc<Option<String>> = Arc::new(page);
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let sent = Arc::new(AtomicU64::new(0));
        let (s, n) = (Arc::clone(&stop), Arc::clone(&sent));
        thread::Builder::new().name("mud-accept".into()).spawn(move || {
            let mut seed = 1;
            for stream in listener.incoming() {
                if s.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let (s, n) = (Arc::clone(&s), Arc::clone(&n));
                let page = Arc::clone(&page);
                let this_seed = seed;
                seed += 1;
                let _ = thread::Builder::new().name("mud-client".into()).spawn(move || {
                    if lantern {
                        serve_lantern(stream, &s, &n);
                    } else {
                        serve(stream, this_seed, bytes_per_second, chat, page.as_deref(), &s, &n);
                    }
                });
            }
        })?;
        Ok(MudServer { port, stop, sent })
    }

    /// Serve Starfall Reach's login screen (`--page login`, or `login-gmcp` to offer GMCP
    /// `Char.Login` first) on 127.0.0.1:`port`. The log records what each client sent.
    pub fn start_login(port: u16, gmcp_login: bool) -> std::io::Result<(MudServer, crate::login::Log)> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let sent = Arc::new(AtomicU64::new(0));
        let log = crate::login::Log::default();
        let (s, n, l) = (Arc::clone(&stop), Arc::clone(&sent), Arc::clone(&log));
        thread::Builder::new().name("mud-accept".into()).spawn(move || {
            for stream in listener.incoming() {
                if s.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let (s, n, l) = (Arc::clone(&s), Arc::clone(&n), Arc::clone(&l));
                let _ = thread::Builder::new().name("mud-client".into()).spawn(move || {
                    let _ = stream.set_nodelay(true);
                    crate::login::serve(&mut stream, gmcp_login, &l, &s, &n);
                });
            }
        })?;
        Ok((MudServer { port, stop, sent }, log))
    }

    /// Serve Dustfall Station, the MSDP world (`--page msdp`), on 127.0.0.1:`port`. The log
    /// records the MSDP commands each client sent.
    pub fn start_msdp(port: u16) -> std::io::Result<(MudServer, crate::msdp_page::Requests)> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let sent = Arc::new(AtomicU64::new(0));
        let requests = crate::msdp_page::Requests::default();
        let (s, n, r) = (Arc::clone(&stop), Arc::clone(&sent), Arc::clone(&requests));
        thread::Builder::new().name("mud-accept".into()).spawn(move || {
            for stream in listener.incoming() {
                if s.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let (s, n, r) = (Arc::clone(&s), Arc::clone(&n), Arc::clone(&r));
                let _ = thread::Builder::new().name("mud-client".into()).spawn(move || {
                    let _ = stream.set_nodelay(true);
                    crate::msdp_page::serve(&mut stream, &r, &s, &n);
                });
            }
        })?;
        Ok((MudServer { port, stop, sent }, requests))
    }

    pub fn bytes_sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }
}

impl Drop for MudServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake the accept loop.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn serve_lantern(mut stream: TcpStream, stop: &AtomicBool, sent: &AtomicU64) {
    let _ = stream.set_nodelay(true);
    // The world reads what the client sends itself (it answers a few commands).
    crate::lantern::serve(&mut stream, stop, sent);
}

fn serve(
    mut stream: TcpStream,
    seed: u64,
    rate: u64,
    chat: bool,
    page: Option<&str>,
    stop: &AtomicBool,
    sent: &AtomicU64,
) {
    let _ = stream.set_nodelay(true);
    // Drain whatever the client sends so its writes never block.
    if let Ok(mut reader) = stream.try_clone() {
        let _ = thread::Builder::new().name("mud-drain".into()).spawn(move || {
            let mut sink = [0u8; 4096];
            while reader.read(&mut sink).map(|n| n > 0).unwrap_or(false) {}
        });
    }
    let mut write = |bytes: &[u8]| -> bool {
        if stream.write_all(bytes).is_err() {
            return false;
        }
        sent.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        true
    };
    if QUIET_PAGE.load(Ordering::Relaxed) {
        if page.is_some_and(|page| write(page.as_bytes())) {
            while !stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(100));
            }
        }
        return;
    }
    if !write(b"\x1b[1;33mWelcome to the bench world.\x1b[0m\r\nA loopback stand-in; nothing here is real.\r\n\r\n") {
        return;
    }
    if GMCP_DEMO.load(Ordering::Relaxed) {
        crate::gmcp_demo::serve(&mut stream, stop, sent);
        return;
    }
    if let Some(page) = page
        && !write(page.as_bytes())
    {
        return;
    }
    let tick = TICK_MS.load(Ordering::Relaxed);
    if let Some(mut n) = (clock_ms_now()).checked_div(tick) {
        // One clock for the whole server, so every connected client gets the same tick number at the same
        // moment and two clients can be compared line for line.
        let clock = *TICK_CLOCK.get_or_init(Instant::now);
        while !stop.load(Ordering::Relaxed) {
            n += 1;
            // Sleep to the shared due time rather than a fixed gap, so the rate does not drift.
            if let Some(wait) = Duration::from_millis(tick * n).checked_sub(clock.elapsed()) {
                thread::sleep(wait);
            }
            // The scheduled time, not the measured one, so every client receives exactly the same line.
            let ms = tick * n;
            let bar = "#".repeat((n % 40) as usize);
            let line = format!("\x1b[36mtick {n:>6}\x1b[0m  due at {ms:>8} ms  \x1b[33m{bar}\x1b[0m\r\n");
            if !write(line.as_bytes()) {
                return;
            }
        }
        return;
    }
    if rate == 0 {
        while !stop.load(Ordering::Relaxed) {
            if !write(b"\x1b[32m<500hp 200m 300mv>\x1b[0m ") {
                return;
            }
            thread::sleep(Duration::from_secs(2));
        }
        return;
    }
    let lotj = LOTJ_ROOMS.load(Ordering::Relaxed);
    if lotj {
        if !write(crate::lotj_page::offer()) {
            return;
        }
        // Give the client time to answer DO GMCP.
        thread::sleep(Duration::from_millis(300));
    }
    let mut generator = AnsiGenerator::new(seed, chat);
    let mut lines = 0usize;
    let mut room = 0u32;
    let clock = Instant::now();
    let mut sent_here = 0u64;
    let mut pending = String::new();
    while !stop.load(Ordering::Relaxed) {
        let due = (clock.elapsed().as_secs_f64() * rate as f64) as i64 - sent_here as i64;
        if due <= 0 {
            thread::sleep(Duration::from_millis(10));
            continue;
        }
        pending.clear();
        if lotj {
            let mut out = Vec::new();
            while ((out.len() + pending.len()) as i64) < due {
                generator.next_line(&mut pending);
                lines += 1;
                if lines.is_multiple_of(crate::lotj_page::LINES_PER_ROOM) {
                    out.extend_from_slice(pending.as_bytes());
                    pending.clear();
                    out.extend(crate::lotj_page::room(room));
                    room += 1;
                }
            }
            out.extend_from_slice(pending.as_bytes());
            if !write(&out) {
                return;
            }
            sent_here += out.len() as u64;
            thread::sleep(Duration::from_millis(WRITE_INTERVAL_MS.load(Ordering::Relaxed)));
            continue;
        }
        while (pending.len() as i64) < due {
            generator.next_line(&mut pending);
        }
        if !write(pending.as_bytes()) {
            return;
        }
        sent_here += pending.len() as u64;
        thread::sleep(Duration::from_millis(WRITE_INTERVAL_MS.load(Ordering::Relaxed)));
    }
}
