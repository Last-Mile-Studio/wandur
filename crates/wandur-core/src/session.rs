//! A MUD connection: network I/O on its own threads, results handed to the UI in batches.
//!
//! Each session has a reader thread (open the link, read, telnet, decode) and a writer thread
//! (commands and negotiation replies, in order). Decoded text and events go into a bounded inbox.
//! The UI takes the whole inbox at once with [`Session::drain`]. The waker is called only when
//! the inbox goes from empty to non-empty, so a flood costs one wake per UI frame, not one per read.
//! When the inbox is full the reader waits, which pushes back on the server through TCP instead of
//! dropping output.
//!
//! The link comes from a [`Transport`] (TCP or TLS); the protocol work is the pure
//! [`TelnetParser`] and [`TextDecoder`], so another transport (a WebSocket in a browser) could
//! drive the same pieces.

use crate::l10n::{S, t, tf};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use crate::charset::{Charset, TextDecoder, encode_line};
use crate::endpoint::Endpoint;
use crate::protocol::{GmcpMessage, MsspTable, parse_gmcp, parse_mssp};
use crate::telnet::{self, OPT_GMCP, OPT_MSDP, OPT_MSSP, TelnetConfig, TelnetEvent, TelnetOutput, TelnetParser};
use crate::transport::{self, Link, Transport};

/// Called from network threads when there is something to drain. Must be cheap and thread safe
/// (for the app it asks egui for a repaint).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Pending text kept before the reader waits for the UI (the C# client keeps 512 Ki characters).
pub const DEFAULT_INBOX_LIMIT: usize = 256 * 1024;
/// Bytes read from the link at a time.
const READ_CHUNK: usize = 16 * 1024;

#[derive(Clone)]
pub struct SessionConfig {
    pub endpoint: Endpoint,
    pub connect_timeout: Duration,
    pub inbox_limit: usize,
    pub charset: Charset,
    pub telnet: TelnetConfig,
    /// Window size reported with NAWS until the UI says otherwise (columns, rows).
    pub window: (u16, u16),
    /// How to reach the server; by default TCP, or TLS for a `tls://` endpoint.
    pub transport: Arc<dyn Transport>,
}

impl std::fmt::Debug for SessionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionConfig")
            .field("endpoint", &self.endpoint)
            .field("charset", &self.charset)
            .field("window", &self.window)
            .finish_non_exhaustive()
    }
}

impl SessionConfig {
    pub fn new(endpoint: Endpoint) -> Self {
        let mut telnet = TelnetConfig::default();
        if endpoint.tls {
            telnet.mtts |= telnet::mtts::SSL;
        }
        telnet.mtts |= telnet::mtts::UTF8;
        Self {
            transport: transport::for_endpoint(&endpoint),
            endpoint,
            connect_timeout: Duration::from_secs(15),
            inbox_limit: DEFAULT_INBOX_LIMIT,
            charset: Charset::Utf8,
            telnet,
            window: (100, 40),
        }
    }

    /// Decode (and encode) with `charset`; MTTS claims UTF-8 only for UTF-8.
    pub fn with_charset(mut self, charset: Charset) -> Self {
        self.charset = charset;
        if charset == Charset::Utf8 {
            self.telnet.mtts |= telnet::mtts::UTF8;
        } else {
            self.telnet.mtts &= !telnet::mtts::UTF8;
        }
        self
    }
}

/// Something that happened on the connection, in order with the text around it.
#[derive(Clone, Debug, PartialEq)]
pub enum SessionEvent {
    Connected {
        peer: String,
        secure: bool,
    },
    /// The connection ended or never started; the text is for people.
    Closed {
        reason: String,
    },
    /// The server echoes (true): input should be private.
    ServerEcho(bool),
    /// GA or EOR after the text drained before it.
    PromptMark,
    /// The server enabled GMCP.
    GmcpEnabled,
    /// The server enabled MSDP.
    MsdpEnabled,
    /// The server declined or turned off GMCP.
    GmcpDisabled,
    /// The server declined or turned off MSDP.
    MsdpDisabled,
    /// A GMCP message.
    Gmcp(GmcpMessage),
    /// An MSDP payload (without the option byte), not decoded yet.
    Msdp(Vec<u8>),
    /// The server's MSSP table (never empty).
    Mssp(MsspTable),
    /// Another telnet subnegotiation, not decoded.
    Subnegotiation {
        option: u8,
        len: usize,
    },
}

/// What one [`Session::drain`] returned. Reuse it between calls; `text` keeps its capacity.
#[derive(Debug, Default)]
pub struct Drained {
    /// Text in arrival order. Events refer to byte offsets into it.
    pub text: String,
    /// Events with the length of `text` at the moment they happened.
    pub events: Vec<(usize, SessionEvent)>,
}

impl Drained {
    pub fn clear(&mut self) {
        self.text.clear();
        self.events.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.events.is_empty()
    }
}

#[derive(Default)]
struct Inbox {
    text: String,
    events: Vec<(usize, SessionEvent)>,
}

const NO_SIZE: u32 = u32::MAX;

fn pack(columns: u16, rows: u16) -> u32 {
    (u32::from(columns) << 16) | u32::from(rows)
}

struct Shared {
    inbox: Mutex<Inbox>,
    space: Condvar,
    limit: usize,
    closing: AtomicBool,
    finished: AtomicBool,
    closer: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    bytes_received: AtomicU64,
    waker: Waker,
    /// Window size the UI last reported, packed.
    window: AtomicU32,
    /// The server asked for NAWS.
    naws: AtomicBool,
    /// Window size last sent with NAWS, packed, or [`NO_SIZE`].
    naws_sent: AtomicU32,
}

impl Shared {
    fn lock_inbox(&self) -> MutexGuard<'_, Inbox> {
        self.inbox.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Append to the inbox, waiting while it is full. Returns false when the session is closing.
    fn push(&self, text: &str, events: &mut Vec<(usize, SessionEvent)>) -> bool {
        let mut inbox = self.lock_inbox();
        while inbox.text.len() >= self.limit && !self.closing.load(Ordering::Acquire) {
            let (guard, _) = self
                .space
                .wait_timeout(inbox, Duration::from_millis(100))
                .unwrap_or_else(PoisonError::into_inner);
            inbox = guard;
        }
        if self.closing.load(Ordering::Acquire) && !events.iter().any(|(_, e)| matches!(e, SessionEvent::Closed { .. }))
        {
            return false;
        }
        let was_empty = inbox.text.is_empty() && inbox.events.is_empty();
        let base = inbox.text.len();
        inbox.text.push_str(text);
        inbox.events.extend(events.drain(..).map(|(at, e)| (base + at, e)));
        drop(inbox);
        if was_empty {
            (self.waker)();
        }
        true
    }

    fn event(&self, event: SessionEvent) {
        self.push("", &mut vec![(0, event)]);
    }

    /// Send the window size with NAWS if the server wants it and it changed (or `force`).
    fn send_naws(&self, writer: &Sender<WriterMessage>, force: bool) {
        if !self.naws.load(Ordering::Acquire) {
            return;
        }
        let size = self.window.load(Ordering::Acquire);
        let previous = self.naws_sent.swap(size, Ordering::AcqRel);
        if force || previous != size {
            let _ = writer.send(WriterMessage::Bytes(telnet::naws((size >> 16) as u16, size as u16)));
        }
    }
}

enum WriterMessage {
    Bytes(Vec<u8>),
}

/// A connection to one MUD. Dropping it disconnects.
pub struct Session {
    shared: Arc<Shared>,
    writer: Sender<WriterMessage>,
    endpoint: Endpoint,
    charset: Charset,
}

impl Session {
    /// Start connecting in the background; returns at once. Progress arrives as events.
    pub fn connect(config: SessionConfig, waker: Waker) -> Session {
        let shared = Arc::new(Shared {
            inbox: Mutex::new(Inbox::default()),
            space: Condvar::new(),
            limit: config.inbox_limit.max(1024),
            closing: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            closer: Mutex::new(None),
            bytes_received: AtomicU64::new(0),
            waker,
            window: AtomicU32::new(pack(config.window.0, config.window.1)),
            naws: AtomicBool::new(false),
            naws_sent: AtomicU32::new(NO_SIZE),
        });
        let (tx, rx) = channel();
        let endpoint = config.endpoint.clone();
        let charset = config.charset;
        let reader_shared = Arc::clone(&shared);
        let reply = tx.clone();
        let spawned = thread::Builder::new()
            .name(format!("wandur-read {endpoint}"))
            .spawn(move || reader_main(config, reader_shared, rx, reply));
        if let Err(e) = spawned {
            shared.finished.store(true, Ordering::Release);
            shared.event(SessionEvent::Closed {
                reason: tf(S::CouldNotStartTheConnectionThread, &[&e]),
            });
        }
        Session {
            shared,
            writer: tx,
            endpoint,
            charset,
        }
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Send one command line (CR LF is added). Queued if the connection is still opening.
    pub fn send_line(&self, line: &str) -> bool {
        if self.is_finished() {
            return false;
        }
        let mut bytes = Vec::with_capacity(line.len() + 2);
        encode_line(line, self.charset, &mut bytes);
        self.writer.send(WriterMessage::Bytes(bytes)).is_ok()
    }

    /// Send a GMCP message (`Package.Name json`) if the server enabled GMCP.
    pub fn send_gmcp(&self, message: &str) -> bool {
        !self.is_finished() && self.writer.send(WriterMessage::Bytes(telnet::gmcp(message))).is_ok()
    }

    /// Send a subnegotiation (`IAC SB option payload IAC SE`, IAC doubled), such as an MSDP
    /// REPORT. It goes out in order with commands and negotiation replies.
    pub fn send_subnegotiation(&self, option: u8, payload: &[u8]) -> bool {
        let mut bytes = Vec::with_capacity(payload.len() + 5);
        telnet::subnegotiation(option, payload, &mut bytes);
        !self.is_finished() && self.writer.send(WriterMessage::Bytes(bytes)).is_ok()
    }

    /// The terminal size changed: remember it and tell the server if it asked (NAWS).
    pub fn set_window_size(&self, columns: u16, rows: u16) {
        self.shared.window.store(pack(columns, rows), Ordering::Release);
        self.shared.send_naws(&self.writer, false);
    }

    /// Take everything received since the last call. `into` is cleared first and swapped with
    /// the inbox, so both strings keep their capacity and nothing is copied.
    pub fn drain(&self, into: &mut Drained) {
        into.clear();
        let mut inbox = self.shared.lock_inbox();
        std::mem::swap(&mut inbox.text, &mut into.text);
        std::mem::swap(&mut inbox.events, &mut into.events);
        drop(inbox);
        self.shared.space.notify_all();
    }

    /// Close the connection. Pending text stays drainable; a `Closed` event follows.
    pub fn disconnect(&self) {
        self.shared.closing.store(true, Ordering::Release);
        if let Some(close) = self
            .shared
            .closer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            close();
        }
        self.shared.space.notify_all();
    }

    /// Whether the reader thread has ended (connection closed or failed).
    pub fn is_finished(&self) -> bool {
        self.shared.finished.load(Ordering::Acquire)
    }

    /// Bytes read from the link so far (before telnet processing).
    pub fn bytes_received(&self) -> u64 {
        self.shared.bytes_received.load(Ordering::Relaxed)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // The threads end on their own once the link is closed; they are not joined so a
        // connect still waiting for its timeout cannot block the UI.
        self.disconnect();
    }
}

fn reader_main(
    config: SessionConfig,
    shared: Arc<Shared>,
    writes: Receiver<WriterMessage>,
    reply: Sender<WriterMessage>,
) {
    let reason = match config.transport.open(&config.endpoint, config.connect_timeout) {
        Ok(link) => {
            let Link {
                reader,
                writer,
                closer,
                peer,
                secure,
            } = link;
            *shared.closer.lock().unwrap_or_else(PoisonError::into_inner) = Some(closer);
            if shared.closing.load(Ordering::Acquire) {
                if let Some(close) = shared.closer.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                    close();
                }
                t(S::Disconnected).to_string()
            } else {
                let _ = thread::Builder::new()
                    .name(format!("wandur-write {}", config.endpoint))
                    .spawn(move || writer_main(writer, writes));
                shared.event(SessionEvent::Connected { peer, secure });
                read_loop(reader, &config, &shared, &reply)
            }
        }
        Err(e) => e,
    };
    let reason = if shared.closing.load(Ordering::Acquire) {
        t(S::Disconnected).to_string()
    } else {
        reason
    };
    shared.finished.store(true, Ordering::Release);
    // The close event must always arrive, even while the UI is not draining.
    let mut events = vec![(0, SessionEvent::Closed { reason })];
    shared.push("", &mut events);
}

fn read_loop(
    mut reader: Box<dyn std::io::Read + Send>,
    config: &SessionConfig,
    shared: &Shared,
    reply: &Sender<WriterMessage>,
) -> String {
    let mut buf = vec![0u8; READ_CHUNK];
    let mut telnet = TelnetParser::with_config(config.telnet.clone());
    let mut out = TelnetOutput::default();
    let mut decoder = TextDecoder::new(config.charset);
    let mut text = String::with_capacity(READ_CHUNK);
    let mut events = Vec::new();
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => return t(S::ServerClosedTheConnection).to_string(),
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return tf(S::ConnectionLost, &[&e]),
        };
        shared.bytes_received.fetch_add(n as u64, Ordering::Relaxed);
        out.clear();
        telnet.feed(&buf[..n], &mut out);
        if !out.replies.is_empty() {
            let _ = reply.send(WriterMessage::Bytes(out.replies.clone()));
        }
        // Decode text between events so each event lands at the right text offset.
        text.clear();
        let mut consumed = 0;
        for (at, event) in out.events.drain(..) {
            decoder.decode(&out.data[consumed..at], &mut text);
            consumed = at;
            let event = match event {
                TelnetEvent::ServerEcho(on) => SessionEvent::ServerEcho(on),
                TelnetEvent::PromptMark => SessionEvent::PromptMark,
                TelnetEvent::GmcpEnabled => SessionEvent::GmcpEnabled,
                TelnetEvent::MsdpEnabled => SessionEvent::MsdpEnabled,
                TelnetEvent::GmcpDisabled => SessionEvent::GmcpDisabled,
                TelnetEvent::MsdpDisabled => SessionEvent::MsdpDisabled,
                TelnetEvent::WindowSizeWanted(on) => {
                    shared.naws.store(on, Ordering::Release);
                    if on {
                        shared.send_naws(reply, true);
                    } else {
                        shared.naws_sent.store(NO_SIZE, Ordering::Release);
                    }
                    continue;
                }
                TelnetEvent::Subnegotiation { option, payload } => match option {
                    OPT_GMCP => match parse_gmcp(&payload) {
                        Some(message) => SessionEvent::Gmcp(message),
                        None => continue,
                    },
                    OPT_MSDP => SessionEvent::Msdp(payload),
                    OPT_MSSP => {
                        let table = parse_mssp(&payload);
                        if table.is_empty() {
                            continue;
                        }
                        SessionEvent::Mssp(table)
                    }
                    _ => SessionEvent::Subnegotiation {
                        option,
                        len: payload.len(),
                    },
                },
            };
            events.push((text.len(), event));
        }
        decoder.decode(&out.data[consumed..], &mut text);
        if (!text.is_empty() || !events.is_empty()) && !shared.push(&text, &mut events) {
            return t(S::Disconnected).to_string();
        }
    }
}

fn writer_main(mut writer: Box<dyn Write + Send>, messages: Receiver<WriterMessage>) {
    while let Ok(WriterMessage::Bytes(bytes)) = messages.recv() {
        if writer.write_all(&bytes).and_then(|()| writer.flush()).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telnet::{DO, IAC, OPT_ECHO, OPT_NAWS, SB, SE, WILL};
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    fn waker() -> (Waker, Arc<AtomicUsize>) {
        let count = Arc::new(AtomicUsize::new(0));
        let c = Arc::clone(&count);
        (
            Arc::new(move || {
                c.fetch_add(1, Ordering::Relaxed);
            }),
            count,
        )
    }

    fn endpoint(listener: &TcpListener) -> Endpoint {
        Endpoint::new("127.0.0.1", listener.local_addr().unwrap().port())
    }

    /// Drain until `done` says so or the deadline passes.
    fn drain_until(session: &Session, all: &mut Drained, done: impl Fn(&Drained) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut batch = Drained::default();
        while !done(all) {
            assert!(Instant::now() < deadline, "timed out; got {:?}", all.events);
            session.drain(&mut batch);
            let base = all.text.len();
            all.text.push_str(&batch.text);
            all.events.extend(batch.events.drain(..).map(|(at, e)| (base + at, e)));
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn loopback_lifecycle_connect_receive_send_close() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let ep = endpoint(&listener);
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(b"Welcome\r\n").unwrap();
            s.write_all(&[IAC, WILL, OPT_ECHO]).unwrap();
            s.write_all(b"Password: ").unwrap();
            // Read the DO ECHO reply and then the command line.
            let mut got = Vec::new();
            let mut buf = [0u8; 64];
            while !got.ends_with(b"\r\n") {
                let n = s.read(&mut buf).unwrap();
                assert!(n > 0);
                got.extend_from_slice(&buf[..n]);
            }
            s.write_all(b"\r\nBye\r\n").unwrap();
            got
        });
        let (waker, wakes) = waker();
        let session = Session::connect(SessionConfig::new(ep), waker);
        let mut all = Drained::default();
        drain_until(&session, &mut all, |d| d.text.contains("Password: "));
        assert!(matches!(all.events[0].1, SessionEvent::Connected { .. }));
        assert!(
            all.events
                .iter()
                .any(|(at, e)| *e == SessionEvent::ServerEcho(true) && *at == "Welcome\r\n".len())
        );
        assert!(session.send_line("secret"));
        drain_until(&session, &mut all, |d| {
            d.events.iter().any(|(_, e)| matches!(e, SessionEvent::Closed { .. }))
        });
        assert!(all.text.ends_with("Bye\r\n"));
        let got = server.join().unwrap();
        assert_eq!(got, [&[IAC, DO, OPT_ECHO][..], b"secret\r\n"].concat());
        assert!(session.is_finished());
        assert!(wakes.load(Ordering::Relaxed) >= 1);
        assert!(!session.send_line("after close"));
    }

    #[test]
    fn user_disconnect_reports_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let ep = endpoint(&listener);
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 16];
            // Block until the client goes away.
            while s.read(&mut buf).map(|n| n > 0).unwrap_or(false) {}
        });
        let (waker, _) = waker();
        let session = Session::connect(SessionConfig::new(ep), waker);
        let mut all = Drained::default();
        drain_until(&session, &mut all, |d| !d.events.is_empty());
        session.disconnect();
        drain_until(&session, &mut all, |d| {
            d.events.iter().any(|(_, e)| matches!(e, SessionEvent::Closed { .. }))
        });
        let closed = all.events.iter().find_map(|(_, e)| match e {
            SessionEvent::Closed { reason } => Some(reason.clone()),
            _ => None,
        });
        assert_eq!(closed.as_deref(), Some("Disconnected"));
        server.join().unwrap();
    }

    #[test]
    fn refused_connection_reports_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let ep = endpoint(&listener);
        drop(listener);
        let (waker, _) = waker();
        let session = Session::connect(SessionConfig::new(ep), waker);
        let mut all = Drained::default();
        drain_until(&session, &mut all, |d| !d.events.is_empty());
        assert!(matches!(&all.events[0].1, SessionEvent::Closed { reason } if reason.starts_with("Could not connect")));
    }

    #[test]
    fn full_inbox_applies_backpressure_without_losing_text() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let ep = endpoint(&listener);
        let total = 2 * 1024 * 1024;
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let line = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcde\n";
            let mut sent = 0;
            while sent < total {
                s.write_all(line).unwrap();
                sent += line.len();
            }
        });
        let (waker, wakes) = waker();
        let mut config = SessionConfig::new(ep);
        config.inbox_limit = 8 * 1024;
        let session = Session::connect(config, waker);
        // Let the reader fill the inbox and stall.
        thread::sleep(Duration::from_millis(200));
        let mut all = Drained::default();
        let mut peak = 0;
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut batch = Drained::default();
        while all.text.len() < total {
            assert!(Instant::now() < deadline);
            session.drain(&mut batch);
            peak = peak.max(batch.text.len());
            all.text.push_str(&batch.text);
        }
        assert_eq!(all.text.len(), total);
        assert!(all.text.lines().all(|l| l.len() == 63));
        // The inbox never held much more than its limit plus one read.
        assert!(peak <= 8 * 1024 + READ_CHUNK, "peak {peak}");
        // Far fewer wakes than reads of a 2 MB flood.
        assert!(wakes.load(Ordering::Relaxed) < total / 1024);
        server.join().unwrap();
    }

    /// Read from the server side until `want` returns true for everything read so far.
    fn read_until(s: &mut std::net::TcpStream, got: &mut Vec<u8>, want: impl Fn(&[u8]) -> bool) {
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut buf = [0u8; 256];
        while !want(got) {
            let n = s.read(&mut buf).unwrap();
            assert!(n > 0, "client closed early; got {got:?}");
            got.extend_from_slice(&buf[..n]);
        }
    }

    fn contains(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn naws_is_sent_when_asked_and_when_the_size_changes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let ep = endpoint(&listener);
        let (waker, _) = waker();
        let mut config = SessionConfig::new(ep);
        config.window = (80, 24);
        let session = Session::connect(config, waker);
        let (mut s, _) = listener.accept().unwrap();
        // Before the server asks, size changes send nothing.
        session.set_window_size(90, 30);
        s.write_all(&[IAC, DO, OPT_NAWS]).unwrap();
        let mut got = Vec::new();
        read_until(&mut s, &mut got, |g| contains(g, &telnet::naws(90, 30)));
        assert!(contains(&got, &[IAC, WILL, OPT_NAWS]));
        session.set_window_size(90, 30);
        session.set_window_size(132, 50);
        read_until(&mut s, &mut got, |g| contains(g, &telnet::naws(132, 50)));
        let naws_count = got.windows(3).filter(|w| *w == [IAC, SB, OPT_NAWS]).count();
        assert_eq!(naws_count, 2, "an unchanged size is not sent again");
    }

    #[test]
    fn msdp_is_negotiated_reported_and_requests_go_out_in_order() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let ep = endpoint(&listener);
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(&[IAC, WILL, OPT_MSDP]).unwrap();
            let mut got = Vec::new();
            read_until(&mut s, &mut got, |g| contains(g, b"\x01LIST\x02REPORTABLE_VARIABLES"));
            let mut out = Vec::new();
            telnet::subnegotiation(
                OPT_MSDP,
                b"\x01REPORTABLE_VARIABLES\x02\x05\x02HEALTH\x02HEALTHMAX\x06",
                &mut out,
            );
            telnet::subnegotiation(OPT_MSDP, b"\x01HEALTH\x02980", &mut out);
            out.extend_from_slice(b"ready\r\n");
            s.write_all(&out).unwrap();
            read_until(&mut s, &mut got, |g| contains(g, b"\x01SEND\x02HEALTH"));
            got
        });
        let (waker, _) = waker();
        let session = Session::connect(SessionConfig::new(ep), waker);
        let mut all = Drained::default();
        drain_until(&session, &mut all, |d| d.text.contains("ready"));
        assert!(all.events.iter().any(|(_, e)| matches!(e, SessionEvent::MsdpEnabled)));
        let payloads: Vec<&Vec<u8>> = all
            .events
            .iter()
            .filter_map(|(_, e)| match e {
                SessionEvent::Msdp(p) => Some(p),
                _ => None,
            })
            .collect();
        assert_eq!(payloads.len(), 2);
        assert_eq!(payloads[1].as_slice(), b"\x01HEALTH\x02980");
        assert!(session.send_subnegotiation(OPT_MSDP, b"\x01SEND\x02HEALTH"));
        let got = server.join().unwrap();
        assert!(contains(&got, &[IAC, DO, OPT_MSDP]));
        // The parser subscribed to the listed variables on its own.
        assert!(contains(&got, b"\x01REPORT\x02HEALTH\x02HEALTHMAX"));
    }

    #[test]
    fn gmcp_and_mssp_arrive_as_events() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let ep = endpoint(&listener);
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(&[IAC, WILL, OPT_GMCP, IAC, WILL, OPT_MSSP]).unwrap();
            let mut got = Vec::new();
            read_until(&mut s, &mut got, |g| contains(g, b"Core.Supports.Set"));
            let mut out = Vec::new();
            out.extend(telnet::gmcp(r#"Char.Vitals {"hp":10}"#));
            telnet::subnegotiation(OPT_MSSP, b"\x01NAME\x02Bench", &mut out);
            out.extend_from_slice(b"text\r\n");
            s.write_all(&out).unwrap();
            read_until(&mut s, &mut got, |g| contains(g, b"Core.Ping"));
            got
        });
        let (waker, _) = waker();
        let session = Session::connect(SessionConfig::new(ep), waker);
        let mut all = Drained::default();
        drain_until(&session, &mut all, |d| d.text.contains("text"));
        let gmcp = all.events.iter().find_map(|(_, e)| match e {
            SessionEvent::Gmcp(m) => Some(m.clone()),
            _ => None,
        });
        let gmcp = gmcp.expect("a GMCP event");
        assert_eq!(gmcp.package, "Char.Vitals");
        assert_eq!(gmcp.data.unwrap()["hp"], 10);
        assert!(all.events.iter().any(|(_, e)| matches!(e, SessionEvent::GmcpEnabled)));
        assert!(
            all.events
                .iter()
                .any(|(_, e)| matches!(e, SessionEvent::Mssp(t) if t.first("NAME") == Some("Bench")))
        );
        assert!(session.send_gmcp("Core.Ping"));
        let got = server.join().unwrap();
        assert!(contains(&got, &[IAC, DO, OPT_GMCP]) && contains(&got, b"Core.Hello {"));
        assert!(contains(&got, &[IAC, DO, OPT_MSSP]));
        assert!(!contains(&got, &[SE, SE]));
    }

    #[test]
    fn latin1_sessions_decode_and_encode_latin1() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let ep = endpoint(&listener);
        let (waker, _) = waker();
        let session = Session::connect(SessionConfig::new(ep).with_charset(Charset::Latin1), waker);
        let (mut s, _) = listener.accept().unwrap();
        s.write_all(b"caf\xe9\r\n").unwrap();
        let mut all = Drained::default();
        drain_until(&session, &mut all, |d| d.text.contains("café"));
        assert!(session.send_line("née"));
        let mut got = Vec::new();
        read_until(&mut s, &mut got, |g| g.ends_with(b"\r\n"));
        assert_eq!(got, b"n\xe9e\r\n");
    }

    #[cfg(feature = "tls")]
    #[test]
    fn tls_loopback_session() {
        use std::sync::Arc as StdArc;
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert_der = cert.cert.der().to_vec();
        let key_der = cert.signing_key.serialize_der();
        let provider = StdArc::new(rustls::crypto::ring::default_provider());
        let server_config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(cert_der.clone())],
                rustls::pki_types::PrivateKeyDer::Pkcs8(key_der.into()),
            )
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let conn = rustls::ServerConnection::new(StdArc::new(server_config)).unwrap();
            let mut tls = rustls::StreamOwned::new(conn, tcp);
            // Enough output to need several TLS records.
            let big = "x".repeat(40_000);
            tls.write_all(format!("Welcome over TLS\r\n{big}\r\n").as_bytes())
                .unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 64];
            while !got.ends_with(b"\r\n") {
                let n = tls.read(&mut buf).unwrap();
                assert!(n > 0);
                got.extend_from_slice(&buf[..n]);
            }
            tls.conn.send_close_notify();
            let _ = tls.flush();
            got
        });
        let mut ep = Endpoint::new("localhost", port);
        ep.tls = true;
        let mut config = SessionConfig::new(ep);
        config.transport = StdArc::new(crate::transport::tls::TlsTransport::with_roots(&[cert_der]));
        let (waker, _) = waker();
        let session = Session::connect(config, waker);
        let mut all = Drained::default();
        drain_until(&session, &mut all, |d| d.text.len() >= 40_000 + 20);
        assert!(all.text.starts_with("Welcome over TLS"));
        assert!(matches!(&all.events[0].1, SessionEvent::Connected { secure: true, .. }));
        assert!(session.send_line("hello"));
        assert_eq!(server.join().unwrap(), b"hello\r\n");
        drain_until(&session, &mut all, |d| {
            d.events.iter().any(|(_, e)| matches!(e, SessionEvent::Closed { .. }))
        });
    }

    #[cfg(feature = "tls")]
    #[test]
    fn tls_rejects_an_untrusted_certificate() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            // Accept and hang up after a moment; the client fails the handshake first or sees EOF.
            let (_s, _) = listener.accept().unwrap();
            thread::sleep(Duration::from_millis(300));
        });
        let mut ep = Endpoint::new("localhost", port);
        ep.tls = true;
        let mut config = SessionConfig::new(ep);
        config.transport = Arc::new(crate::transport::tls::TlsTransport::with_roots(&[]));
        let (waker, _) = waker();
        let session = Session::connect(config, waker);
        let mut all = Drained::default();
        drain_until(&session, &mut all, |d| !d.events.is_empty());
        assert!(
            matches!(&all.events[0].1, SessionEvent::Closed { reason } if reason.contains("TLS")),
            "{:?}",
            all.events
        );
        server.join().unwrap();
    }
}
