//! Parsing what a person types as a MUD address: `host:port`, `host port`, `telnet://host:port`,
//! bracketed IPv6 (`[::1]:4000`), and `tls://` or `telnets://` for an encrypted connection.

use crate::l10n::{S, t, tf};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    /// Connect with TLS.
    pub tls: bool,
}

impl Endpoint {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            tls: false,
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.tls {
            write!(f, "tls://")?;
        }
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EndpointError {
    Empty,
    MissingPort,
    BadPort(String),
    BadHost(String),
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EndpointError::Empty => f.write_str(t(S::AddressEmpty)),
            EndpointError::MissingPort => f.write_str(t(S::AddressMissingPort)),
            EndpointError::BadPort(p) => f.write_str(&tf(S::AddressBadPort, &[p])),
            EndpointError::BadHost(h) => f.write_str(&tf(S::AddressBadHost, &[h])),
        }
    }
}

impl std::error::Error for EndpointError {}

impl std::str::FromStr for Endpoint {
    type Err = EndpointError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let mut s = input.trim();
        if s.is_empty() {
            return Err(EndpointError::Empty);
        }
        let mut tls = false;
        if let Some(rest) = s.strip_prefix("telnet://") {
            s = rest.trim_end_matches('/');
        } else if let Some(rest) = s.strip_prefix("tls://").or_else(|| s.strip_prefix("telnets://")) {
            s = rest.trim_end_matches('/');
            tls = true;
        }
        let (host, port) = if let Some(rest) = s.strip_prefix('[') {
            let close = rest.find(']').ok_or_else(|| EndpointError::BadHost(s.to_string()))?;
            let host = &rest[..close];
            let after = rest[close + 1..].trim_start_matches([':', ' ']);
            (host, after)
        } else if let Some((h, p)) = s.split_once(char::is_whitespace) {
            (h, p.trim())
        } else if s.matches(':').count() == 1 {
            s.split_once(':').unwrap_or((s, ""))
        } else {
            (s, "")
        };
        if host.is_empty() || host.chars().any(|c| c.is_whitespace() || c == '/') {
            return Err(EndpointError::BadHost(host.to_string()));
        }
        if port.is_empty() {
            return Err(EndpointError::MissingPort);
        }
        let port: u16 = match port.parse() {
            Ok(p) if p > 0 => p,
            _ => return Err(EndpointError::BadPort(port.to_string())),
        };
        Ok(Endpoint {
            host: host.to_string(),
            port,
            tls,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(s: &str) -> Result<Endpoint, EndpointError> {
        s.parse()
    }

    #[test]
    fn accepted_forms() {
        let want = Endpoint::new("mud.example.org", 4000);
        assert_eq!(ep("mud.example.org:4000"), Ok(want.clone()));
        assert_eq!(ep("  mud.example.org 4000 "), Ok(want.clone()));
        assert_eq!(ep("telnet://mud.example.org:4000/"), Ok(want));
        let v6 = ep("[::1]:4000").unwrap();
        assert_eq!(v6.host, "::1");
        assert_eq!(v6.to_string(), "[::1]:4000");
        let secure = ep("tls://mud.example.org:4443").unwrap();
        assert!(secure.tls);
        assert_eq!(secure.to_string(), "tls://mud.example.org:4443");
        assert_eq!(ep(&secure.to_string()), Ok(secure.clone()));
        assert_eq!(ep("telnets://mud.example.org:4443"), Ok(secure));
    }

    #[test]
    fn rejected_forms() {
        assert_eq!(ep(""), Err(EndpointError::Empty));
        assert_eq!(ep("example.org"), Err(EndpointError::MissingPort));
        assert!(matches!(ep("example.org:0"), Err(EndpointError::BadPort(_))));
        assert!(matches!(ep("example.org:70000"), Err(EndpointError::BadPort(_))));
        assert!(matches!(ep("::1"), Err(EndpointError::MissingPort)));
    }
}
