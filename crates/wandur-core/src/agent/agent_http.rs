//! HTTP for the model providers (the C# `AgentHttpTransport`): one request at a time per
//! server origin, the profile's response timeout over the whole exchange, a 128 KiB response
//! limit, and errors that never carry the server's text or the API key.
//!
//! ureq calls cannot be interrupted. A cancelled call is therefore abandoned: the caller gets
//! [`AgentError::Cancelled`] at once (or as soon as it was waiting for its turn), the request
//! finishes on its own thread within the timeout, and its answer is dropped. The origin's turn
//! is released when its holder is cancelled, as cancelling a C# request releases its gate.

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
        if holder.as_ref().is_none_or(CancelToken::is_cancelled) {
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
        // Short waits: a cancelled holder or caller is noticed without a wake-up.
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
    let result = exchange(&address, payload, api_key, remaining);
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
) -> Result<Value, AgentError> {
    use ureq::tls::{RootCerts, TlsConfig};
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
        .build()
        .into();
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
    fn a_turn_is_released_by_drop_or_by_cancelling_its_holder() {
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
        // The holder is cancelled (Stop): the turn is free although its call still runs.
        first.cancel();
        let fourth = CancelToken::new();
        let next = take_turn(origin, &fourth, deadline()).unwrap();
        drop(turn);
        // The old turn's drop does not release the new holder.
        let fifth = CancelToken::new();
        assert_eq!(
            take_turn(origin, &fifth, Instant::now() + Duration::from_millis(30)).err(),
            Some(AgentError::Timeout)
        );
        drop(next);
        assert!(take_turn(origin, &fifth, deadline()).is_ok());
    }

    #[test]
    fn a_control_character_in_the_key_is_refused_before_any_request() {
        let profile = AgentProfile::default();
        let err = send(&profile, "models", None, Some("bad\nkey"), &CancelToken::new()).unwrap_err();
        assert_eq!(err, AgentError::Invalid("Invalid API credential."));
    }
}
