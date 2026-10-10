//! The lifecycle of one world's connection across sessions: connect, run, close, and reconnect
//! by hand or automatically with exponential backoff. Time is passed in, so the policy is
//! testable without waiting.
//!
//! States: `Connecting` -> `Connected` -> `Closed`; with automatic reconnect on, a close the
//! person did not ask for goes to `Waiting` and then `Connecting` again. A person's Disconnect
//! cancels any waiting. The attempt counter resets once a connection has stayed up for
//! [`STABLE_AFTER`], so a server that accepts and drops at once backs off instead of looping.

use std::time::{Duration, Instant};

use crate::session::{Drained, Session, SessionConfig, SessionEvent, Waker};

/// A connection that lasted this long counts as having worked: the backoff starts over.
pub const STABLE_AFTER: Duration = Duration::from_secs(30);

/// Delays between automatic attempts: `base`, doubling, capped at `max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Backoff {
    pub base: Duration,
    pub max: Duration,
    /// Give up after this many attempts in a row (0: never).
    pub max_attempts: u32,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(2),
            max: Duration::from_secs(60),
            max_attempts: 10,
        }
    }
}

impl Backoff {
    /// Delay before attempt `attempt` (1 is the first retry).
    pub fn delay(&self, attempt: u32) -> Duration {
        let factor = 1u32 << attempt.saturating_sub(1).min(16);
        self.base.saturating_mul(factor).min(self.max)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected {
        peer: String,
        secure: bool,
    },
    Closed {
        reason: String,
    },
    /// Waiting to try again automatically.
    Waiting {
        reason: String,
        until: Instant,
        attempt: u32,
    },
}

/// Something the person should be told, beyond the session's own events.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// Will try again in `delay` (attempt number).
    RetryScheduled { delay: Duration, attempt: u32 },
    /// Trying again now.
    Reconnecting { attempt: u32 },
    /// Gave up after `attempts`.
    GaveUp { attempts: u32 },
}

pub struct Connection {
    config: SessionConfig,
    waker: Waker,
    session: Option<Session>,
    state: ConnectionState,
    pub auto_reconnect: bool,
    pub backoff: Backoff,
    attempt: u32,
    connected_at: Option<Instant>,
    user_closed: bool,
    /// Bumped on every new session, so a consumer can reset per-connection state.
    generation: u64,
}

impl Connection {
    /// Start connecting at once.
    pub fn open(config: SessionConfig, waker: Waker) -> Self {
        let session = Session::connect(config.clone(), waker.clone());
        Self {
            config,
            waker,
            session: Some(session),
            state: ConnectionState::Connecting,
            auto_reconnect: false,
            backoff: Backoff::default(),
            attempt: 0,
            connected_at: None,
            user_closed: false,
            generation: 1,
        }
    }

    pub fn state(&self) -> &ConnectionState {
        &self.state
    }

    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.state, ConnectionState::Connected { .. })
    }

    /// Closed or waiting: input cannot be sent.
    pub fn is_down(&self) -> bool {
        matches!(
            self.state,
            ConnectionState::Closed { .. } | ConnectionState::Waiting { .. }
        )
    }

    /// Take what the session received, updating the state from its events. Notices about
    /// automatic reconnects are appended to `notices`.
    pub fn drain(&mut self, now: Instant, into: &mut Drained, notices: &mut Vec<Notice>) {
        into.clear();
        let Some(session) = &self.session else { return };
        session.drain(into);
        for (_, event) in &into.events {
            match event {
                SessionEvent::Connected { peer, secure } => {
                    self.state = ConnectionState::Connected {
                        peer: peer.clone(),
                        secure: *secure,
                    };
                    self.connected_at = Some(now);
                }
                SessionEvent::Closed { reason } => self.closed(now, reason.clone(), notices),
                _ => {}
            }
        }
    }

    fn closed(&mut self, now: Instant, reason: String, notices: &mut Vec<Notice>) {
        if self
            .connected_at
            .take()
            .is_some_and(|at| now.duration_since(at) >= STABLE_AFTER)
        {
            self.attempt = 0;
        }
        if !self.auto_reconnect || self.user_closed {
            self.state = ConnectionState::Closed { reason };
            return;
        }
        let attempt = self.attempt + 1;
        if self.backoff.max_attempts != 0 && attempt > self.backoff.max_attempts {
            notices.push(Notice::GaveUp { attempts: self.attempt });
            self.state = ConnectionState::Closed { reason };
            return;
        }
        let delay = self.backoff.delay(attempt);
        notices.push(Notice::RetryScheduled { delay, attempt });
        self.state = ConnectionState::Waiting {
            reason,
            until: now + delay,
            attempt,
        };
    }

    /// Start the scheduled attempt if it is due. Returns true when a new session started.
    pub fn poll(&mut self, now: Instant, notices: &mut Vec<Notice>) -> bool {
        if let ConnectionState::Waiting { until, attempt, .. } = self.state
            && now >= until
        {
            self.attempt = attempt;
            notices.push(Notice::Reconnecting { attempt });
            self.start();
            return true;
        }
        false
    }

    /// When [`Self::poll`] has something to do.
    pub fn deadline(&self) -> Option<Instant> {
        match self.state {
            ConnectionState::Waiting { until, .. } => Some(until),
            _ => None,
        }
    }

    /// The person asked to disconnect: close and do not reconnect automatically.
    pub fn disconnect(&mut self) {
        self.user_closed = true;
        if let ConnectionState::Waiting { reason, .. } = &self.state {
            self.state = ConnectionState::Closed { reason: reason.clone() };
        }
        if let Some(session) = &self.session {
            session.disconnect();
        }
    }

    /// The person asked to reconnect now: a new session, the backoff starts over.
    pub fn reconnect(&mut self) {
        self.attempt = 0;
        self.start();
    }

    fn start(&mut self) {
        if let Some(old) = self.session.take() {
            old.disconnect();
        }
        self.user_closed = false;
        self.connected_at = None;
        self.generation += 1;
        self.session = Some(Session::connect(self.config.clone(), self.waker.clone()));
        self.state = ConnectionState::Connecting;
    }

    pub fn send_line(&self, line: &str) -> bool {
        self.session.as_ref().is_some_and(|s| s.send_line(line))
    }

    pub fn set_window_size(&mut self, columns: u16, rows: u16) {
        self.config.window = (columns, rows);
        if let Some(session) = &self.session {
            session.set_window_size(columns, rows);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::Endpoint;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let b = Backoff {
            base: Duration::from_secs(2),
            max: Duration::from_secs(60),
            max_attempts: 0,
        };
        let delays: Vec<u64> = (1..=7).map(|a| b.delay(a).as_secs()).collect();
        assert_eq!(delays, [2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(b.delay(1_000).as_secs(), 60);
    }

    /// A server that greets each connection with its number and hangs up after `hold`.
    fn flaky_server(hold: Duration) -> (Endpoint, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let ep = Endpoint::new("127.0.0.1", listener.local_addr().unwrap().port());
        let accepted = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&accepted);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { break };
                let n = count.fetch_add(1, Ordering::SeqCst) + 1;
                let _ = s.write_all(format!("hello {n}\r\n").as_bytes());
                thread::sleep(hold);
            }
        });
        (ep, accepted)
    }

    fn run_until(conn: &mut Connection, done: impl Fn(&Connection, &str) -> bool) -> (String, Vec<Notice>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut text = String::new();
        let mut notices = Vec::new();
        let mut batch = Drained::default();
        while !done(conn, &text) {
            assert!(Instant::now() < deadline, "timed out in {:?}", conn.state());
            let now = Instant::now();
            conn.drain(now, &mut batch, &mut notices);
            text.push_str(&batch.text);
            conn.poll(now, &mut notices);
            thread::sleep(Duration::from_millis(5));
        }
        (text, notices)
    }

    fn fast(conn: &mut Connection) {
        conn.auto_reconnect = true;
        conn.backoff = Backoff {
            base: Duration::from_millis(30),
            max: Duration::from_millis(100),
            max_attempts: 3,
        };
    }

    #[test]
    fn automatic_reconnect_retries_with_backoff_and_gives_up() {
        let (ep, accepted) = flaky_server(Duration::from_millis(20));
        let mut conn = Connection::open(SessionConfig::new(ep), Arc::new(|| {}));
        fast(&mut conn);
        let (text, notices) = run_until(&mut conn, |c, _| matches!(c.state(), ConnectionState::Closed { .. }));
        // The first connection plus three retries, then it gives up.
        assert_eq!(accepted.load(Ordering::SeqCst), 4);
        assert!(text.contains("hello 1") && text.contains("hello 4"));
        let scheduled: Vec<u32> = notices
            .iter()
            .filter_map(|n| match n {
                Notice::RetryScheduled { attempt, .. } => Some(*attempt),
                _ => None,
            })
            .collect();
        assert_eq!(scheduled, [1, 2, 3]);
        assert_eq!(notices.last(), Some(&Notice::GaveUp { attempts: 3 }));
        assert_eq!(conn.generation(), 4);
    }

    #[test]
    fn user_disconnect_cancels_automatic_reconnect() {
        let (ep, accepted) = flaky_server(Duration::from_secs(5));
        let mut conn = Connection::open(SessionConfig::new(ep), Arc::new(|| {}));
        fast(&mut conn);
        run_until(&mut conn, |c, _| c.is_connected());
        conn.disconnect();
        let (_, notices) = run_until(&mut conn, |c, _| matches!(c.state(), ConnectionState::Closed { .. }));
        assert!(notices.is_empty(), "{notices:?}");
        thread::sleep(Duration::from_millis(150));
        let mut batch = Drained::default();
        let mut more = Vec::new();
        conn.drain(Instant::now(), &mut batch, &mut more);
        assert!(!conn.poll(Instant::now(), &mut more));
        assert_eq!(accepted.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn manual_reconnect_opens_a_new_session() {
        let (ep, accepted) = flaky_server(Duration::from_millis(10));
        let mut conn = Connection::open(SessionConfig::new(ep), Arc::new(|| {}));
        run_until(&mut conn, |c, _| matches!(c.state(), ConnectionState::Closed { .. }));
        assert_eq!(conn.deadline(), None, "no automatic retry by default");
        conn.reconnect();
        let (text, _) = run_until(&mut conn, |_, t| t.contains("hello 2"));
        assert!(text.contains("hello 2"));
        assert_eq!(accepted.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn waiting_state_reports_its_deadline() {
        let (ep, _) = flaky_server(Duration::from_millis(5));
        let mut conn = Connection::open(SessionConfig::new(ep), Arc::new(|| {}));
        conn.auto_reconnect = true;
        conn.backoff.base = Duration::from_secs(5);
        run_until(&mut conn, |c, _| matches!(c.state(), ConnectionState::Waiting { .. }));
        let deadline = conn.deadline().unwrap();
        assert!(deadline > Instant::now() + Duration::from_secs(4));
        conn.disconnect();
        assert!(matches!(conn.state(), ConnectionState::Closed { .. }));
        assert_eq!(conn.deadline(), None);
    }
}
