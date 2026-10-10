//! HTTP for the model providers (the C# `AgentHttpTransport`): one request at a time per
//! server origin, the profile's response timeout over the whole exchange, a 128 KiB response
//! limit, and errors that never carry the server's text or the API key.
//!
//! A cancelled call really stops, as cancelling a C# request does: the connection reads in
//! short slices and checks the cancel flag between them ([`CancelConnector`]), so within a
//! fraction of a second the call returns [`AgentError::Cancelled`] and its connection is closed
//! (a model server then stops generating). The origin's turn is released only when the call
//! has ended, so a server never has two requests from this client at once.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::profile::AgentProfile;
use super::{AgentError, CancelToken};

/// The largest response read (bytes).
pub const MAX_RESPONSE: u64 = 128 * 1024;

/// One origin's turn: the token of the call that holds it.
#[derive(Default)]
struct Gate {
    holder: Mutex<Option<CancelToken>>,
    changed: Condvar,
}

fn gates() -> &'static Mutex<HashMap<String, Arc<Gate>>> {
    static GATES: OnceLock<Mutex<HashMap<String, Arc<Gate>>>> = OnceLock::new();
    GATES.get_or_init(Default::default)
}

/// The turn of one call; released when dropped.
struct Turn {
    gate: Arc<Gate>,
    token: CancelToken,
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut holder = self.gate.holder.lock().unwrap_or_else(PoisonError::into_inner);
        if holder.as_ref().is_some_and(|h| h.same(&self.token)) {
            *holder = None;
        }
        self.gate.changed.notify_all();
    }
}

/// Wait for the origin's turn until `deadline`, giving up when `cancel` is set.
fn take_turn(origin: &str, cancel: &CancelToken, deadline: Instant) -> Result<Turn, AgentError> {
    let gate = {
        let mut gates = gates().lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(gates.entry(origin.to_ascii_lowercase()).or_default())
    };
    let mut holder = gate.holder.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        if cancel.is_cancelled() {
            return Err(AgentError::Cancelled);
        }
        if holder.is_none() {
            *holder = Some(cancel.clone());
            drop(holder);
            return Ok(Turn {
                gate,
                token: cancel.clone(),
            });
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(AgentError::Timeout);
        }
        // Short waits: a cancelled caller is noticed without a wake-up.
        let wait = (deadline - now).min(Duration::from_millis(25));
        holder = gate
            .changed
            .wait_timeout(holder, wait)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

/// Send `payload` (POST) or nothing (GET) to `route` under the profile's endpoint and read the
/// JSON answer.
pub fn send(
    profile: &AgentProfile,
    route: &str,
    payload: Option<&Value>,
    api_key: Option<&str>,
    cancel: &CancelToken,
) -> Result<Value, AgentError> {
    if api_key.is_some_and(|k| k.chars().any(char::is_control)) {
        return Err(AgentError::Invalid("Invalid API credential."));
    }
    let address = [profile.endpoint.trim_end_matches('/'), "/", route].concat();
    let url = url::Url::parse(&address).map_err(|_| AgentError::Invalid("Invalid agent endpoint."))?;
    let origin = url.origin().ascii_serialization();
    let timeout = Duration::from_secs(profile.response_timeout_seconds.max(1) as u64);
    let deadline = Instant::now() + timeout;
    let _turn = take_turn(&origin, cancel, deadline)?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(AgentError::Timeout);
    }
    let result = exchange(&address, payload, api_key, remaining, cancel);
    if cancel.is_cancelled() {
        return Err(AgentError::Cancelled);
    }
    result
}

fn exchange(
    address: &str,
    payload: Option<&Value>,
    api_key: Option<&str>,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<Value, AgentError> {
    use ureq::tls::{RootCerts, TlsConfig};
    use ureq::unversioned::resolver::DefaultResolver;
    use ureq::unversioned::transport::{ConnectProxyConnector, Connector as _, RustlsConnector, TcpConnector};
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
        .build();
    // ureq's own chain (a CONNECT proxy, TCP, TLS) with the cancel check over the socket, under
    // TLS, so a cancel also ends a handshake or an encrypted read.
    let connector =
        ().chain(ConnectProxyConnector::default())
            .chain(TcpConnector::default())
            .chain(CancelConnector(cancel.clone()))
            .chain(RustlsConnector::default());
    let agent = ureq::Agent::with_parts(config, connector, DefaultResolver::default());
    let bearer = api_key
        .filter(|k| !k.trim().is_empty())
        .map(|k| ["Bearer ", k].concat());
    let response = match payload {
        Some(body) => {
            let mut request = agent.post(address).header("Content-Type", "application/json");
            if let Some(bearer) = &bearer {
                request = request.header("Authorization", bearer.as_str());
            }
            request.send(body.to_string())
        }
        None => {
            let mut request = agent.get(address);
            if let Some(bearer) = &bearer {
                request = request.header("Authorization", bearer.as_str());
            }
            request.call()
        }
    }
    .map_err(transport_error)?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(AgentError::Http(Some(status)));
    }
    let declared = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    if declared.is_some_and(|n| n > MAX_RESPONSE) {
        return Err(AgentError::InvalidResponse);
    }
    let body = response
        .into_body()
        .with_config()
        .limit(MAX_RESPONSE)
        .read_to_vec()
        .map_err(|e| match e {
            ureq::Error::Timeout(_) => AgentError::Timeout,
            ureq::Error::BodyExceedsLimit(_) => AgentError::InvalidResponse,
            _ => AgentError::Http(None),
        })?;
    serde_json::from_slice(&body).map_err(|_| AgentError::InvalidResponse)
}

/// How long one read waits before the cancel flag is looked at again.
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// Wraps each connection of one call in [`Cancellable`].
#[derive(Debug)]
struct CancelConnector(CancelToken);

impl<In: ureq::unversioned::transport::Transport> ureq::unversioned::transport::Connector<In> for CancelConnector {
    type Out = Cancellable<In>;

    fn connect(
        &self,
        _: &ureq::unversioned::transport::ConnectionDetails,
        chained: Option<In>,
    ) -> Result<Option<Self::Out>, ureq::Error> {
        Ok(chained.map(|inner| Cancellable {
            inner,
            cancel: self.0.clone(),
        }))
    }
}

/// A connection that gives up once its call is cancelled: reads wait at most [`CANCEL_POLL`]
/// at a time, within the timeout ureq asked for. Dropping it (the call ends) closes the socket.
#[derive(Debug)]
struct Cancellable<T> {
    inner: T,
    cancel: CancelToken,
}

impl<T> Cancellable<T> {
    fn check(&self) -> Result<(), ureq::Error> {
        if self.cancel.is_cancelled() {
            // Not `Interrupted`: readers retry that kind.
            return Err(ureq::Error::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionAborted,
                "cancelled",
            )));
        }
        Ok(())
    }
}

impl<T: ureq::unversioned::transport::Transport> ureq::unversioned::transport::Transport for Cancellable<T> {
    fn buffers(&mut self) -> &mut dyn ureq::unversioned::transport::Buffers {
        self.inner.buffers()
    }

    fn transmit_output(
        &mut self,
        amount: usize,
        timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<(), ureq::Error> {
        self.check()?;
        self.inner.transmit_output(amount, timeout)
    }

    fn await_input(&mut self, timeout: ureq::unversioned::transport::NextTimeout) -> Result<bool, ureq::Error> {
        use ureq::unversioned::transport::{NextTimeout, time};
        let started = Instant::now();
        loop {
            self.check()?;
            // `NotHappening` reads as a very long time, so this also covers no timeout at all.
            let left = timeout.after.saturating_sub(started.elapsed());
            if left.is_zero() {
                return Err(ureq::Error::Timeout(timeout.reason));
            }
            let slice = NextTimeout {
                after: time::Duration::Exact(left.min(CANCEL_POLL)),
                reason: timeout.reason,
            };
            match self.inner.await_input(slice) {
                Err(ureq::Error::Timeout(_)) if left > CANCEL_POLL => continue,
                other => return other,
            }
        }
    }

    fn is_open(&mut self) -> bool {
        !self.cancel.is_cancelled() && self.inner.is_open()
    }

    fn is_tls(&self) -> bool {
        self.inner.is_tls()
    }
}

fn transport_error(e: ureq::Error) -> AgentError {
    match e {
        ureq::Error::Timeout(_) => AgentError::Timeout,
        ureq::Error::StatusCode(status) => AgentError::Http(Some(status)),
        ureq::Error::BodyExceedsLimit(_) => AgentError::InvalidResponse,
        _ => AgentError::Http(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_is_released_only_when_its_call_ends() {
        let origin = "http://gate-test.invalid:1";
        let deadline = || Instant::now() + Duration::from_secs(5);
        let first = CancelToken::new();
        let turn = take_turn(origin, &first, deadline()).unwrap();
        // A second caller waits; cancelling it returns at once.
        let second = CancelToken::new();
        let waiting = {
            let second = second.clone();
            std::thread::spawn(move || take_turn(origin, &second, Instant::now() + Duration::from_secs(5)).err())
        };
        std::thread::sleep(Duration::from_millis(60));
        second.cancel();
        assert_eq!(waiting.join().unwrap(), Some(AgentError::Cancelled));
        // A short deadline times out.
        let third = CancelToken::new();
        assert_eq!(
            take_turn(origin, &third, Instant::now() + Duration::from_millis(30)).err(),
            Some(AgentError::Timeout)
        );
        // The holder is cancelled (Stop): the turn stays taken until its call has ended.
        first.cancel();
        let fourth = CancelToken::new();
        assert_eq!(
            take_turn(origin, &fourth, Instant::now() + Duration::from_millis(30)).err(),
            Some(AgentError::Timeout)
        );
        drop(turn);
        let next = take_turn(origin, &fourth, deadline()).unwrap();
        drop(next);
        assert!(take_turn(origin, &CancelToken::new(), deadline()).is_ok());
    }

    /// A model server that never answers: it counts the requests open at once (the most ever)
    /// and reports each connection the client closed.
    struct SlowServer {
        port: u16,
        most_open: Arc<std::sync::atomic::AtomicUsize>,
        arrived: std::sync::mpsc::Receiver<()>,
        closed: std::sync::mpsc::Receiver<()>,
    }

    fn slow_server() -> SlowServer {
        use std::io::Read as _;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let open = Arc::new(AtomicUsize::new(0));
        let most_open = Arc::new(AtomicUsize::new(0));
        let (arrived_tx, arrived) = std::sync::mpsc::channel();
        let (closed_tx, closed) = std::sync::mpsc::channel();
        let most = Arc::clone(&most_open);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let (open, most) = (Arc::clone(&open), Arc::clone(&most));
                let (arrived, closed) = (arrived_tx.clone(), closed_tx.clone());
                std::thread::spawn(move || {
                    let now = open.fetch_add(1, Ordering::SeqCst) + 1;
                    most.fetch_max(now, Ordering::SeqCst);
                    let _ = arrived.send(());
                    // Hold the request: read until the client goes (or 20 s pass).
                    stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
                    let mut buf = [0u8; 4096];
                    while matches!(stream.read(&mut buf), Ok(n) if n > 0) {}
                    open.fetch_sub(1, Ordering::SeqCst);
                    let _ = closed.send(());
                });
            }
        });
        SlowServer {
            port,
            most_open,
            arrived,
            closed,
        }
    }

    /// Stop really stops a request in flight: the call returns at once, the server sees its
    /// connection close, and the next request is never sent while the old one is still open.
    #[test]
    fn a_cancelled_request_ends_before_the_server_gets_another() {
        let server = slow_server();
        let profile = AgentProfile {
            endpoint: format!("http://127.0.0.1:{}/v1", server.port),
            response_timeout_seconds: 30,
            ..AgentProfile::default()
        };
        let call = |cancel: &CancelToken| {
            let (profile, cancel) = (profile.clone(), cancel.clone());
            std::thread::spawn(move || send(&profile, "models", None, None, &cancel))
        };
        let first = CancelToken::new();
        let running = call(&first);
        server.arrived.recv_timeout(Duration::from_secs(10)).unwrap();
        let stopped = Instant::now();
        first.cancel();
        // The next call starts at once (Play again), before the first has been seen to end.
        let second = CancelToken::new();
        let next = call(&second);
        assert_eq!(running.join().unwrap().unwrap_err(), AgentError::Cancelled);
        server
            .closed
            .recv_timeout(Duration::from_secs(5))
            .expect("the server saw it close");
        assert!(stopped.elapsed() < Duration::from_secs(3), "{:?}", stopped.elapsed());
        server.arrived.recv_timeout(Duration::from_secs(10)).unwrap();
        second.cancel();
        assert_eq!(next.join().unwrap().unwrap_err(), AgentError::Cancelled);
        server.closed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            server.most_open.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "never two requests on the server"
        );
    }

    #[test]
    fn a_control_character_in_the_key_is_refused_before_any_request() {
        let profile = AgentProfile::default();
        let err = send(&profile, "models", None, Some("bad\nkey"), &CancelToken::new()).unwrap_err();
        assert_eq!(err, AgentError::Invalid("Invalid API credential."));
    }
}
