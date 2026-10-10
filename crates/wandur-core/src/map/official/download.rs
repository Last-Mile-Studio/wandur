//! Fetching an official map: https only, at most [`MAX_REDIRECTS`] redirects and each to an
//! https address (another host is fine: release downloads usually redirect to a file host),
//! [`MAX_BYTES`] at most, counted while the body streams in, and `If-None-Match` with the ETag
//! of the file imported before, or `If-Modified-Since` with its `Last-Modified` when the server
//! gave no ETag (a 304 means it has not changed). Runs on a worker thread.

use std::io::Read;
use std::time::Duration;

pub use super::store::Validators;
use crate::l10n::{S, t, tf};
use crate::map::format;

/// Redirects followed for one download.
pub const MAX_REDIRECTS: usize = 3;
/// Largest file accepted (the map file limit).
pub const MAX_BYTES: usize = format::MAX_BYTES;

/// Why a download failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DownloadError {
    /// The address is not https.
    NotHttps,
    /// A redirect pointed at an address that is not https.
    RedirectNotHttps,
    /// More than [`MAX_REDIRECTS`] redirects.
    TooManyRedirects,
    /// The file is larger than [`MAX_BYTES`].
    TooLarge,
    /// The server answered with this status.
    Status(u16),
    /// The connection failed (the reason, as the HTTP library words it).
    Network(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::NotHttps => f.write_str(t(S::OfficialMapNotHttps)),
            DownloadError::RedirectNotHttps => f.write_str(t(S::OfficialMapRedirectNotHttps)),
            DownloadError::TooManyRedirects => f.write_str(&tf(S::OfficialMapTooManyRedirects, &[&MAX_REDIRECTS])),
            DownloadError::TooLarge => f.write_str(t(S::MapImportFileTooLarge)),
            DownloadError::Status(code) => f.write_str(&tf(S::OfficialMapHttpStatus, &[code])),
            DownloadError::Network(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for DownloadError {}

/// What the server sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Downloaded {
    /// 304: the file has not changed since the ETag sent.
    NotModified,
    /// The file, with its ETag when the server gave one.
    File {
        bytes: Vec<u8>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
}

/// The agent for map downloads: the operating system's certificate verifier, no redirects of
/// its own (they are followed here, checked one by one), timeouts.
pub fn agent() -> ureq::Agent {
    agent_with(ureq::tls::RootCerts::PlatformVerifier)
}

fn agent_with(roots: ureq::tls::RootCerts) -> ureq::Agent {
    use ureq::tls::TlsConfig;
    ureq::Agent::config_builder()
        .user_agent(crate::directory::client::user_agent())
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(300)))
        .http_status_as_error(false)
        .max_redirects(0)
        .https_only(true)
        .tls_config(TlsConfig::builder().root_certs(roots).build())
        .build()
        .into()
}

/// An agent trusting only these certificates (DER), for tests against a local server.
#[doc(hidden)]
pub fn agent_trusting(certificates: &[Vec<u8>]) -> ureq::Agent {
    let roots = certificates
        .iter()
        .map(|c| ureq::tls::Certificate::from_der(c).to_owned())
        .collect();
    agent_with(ureq::tls::RootCerts::Specific(std::sync::Arc::new(roots)))
}

fn https(url: &str) -> Option<url::Url> {
    url::Url::parse(url).ok().filter(|u| u.scheme() == "https")
}

/// Download `url`, sending the validators of the file the client has.
pub fn download(agent: &ureq::Agent, url: &str, validators: &Validators) -> Result<Downloaded, DownloadError> {
    fetch(agent, url, validators, MAX_BYTES)
}

fn fetch(
    agent: &ureq::Agent,
    url: &str,
    validators: &Validators,
    max_bytes: usize,
) -> Result<Downloaded, DownloadError> {
    let mut address = https(url).ok_or(DownloadError::NotHttps)?;
    let mut redirects = 0;
    loop {
        let mut request = agent.get(address.as_str());
        if let Some(etag) = &validators.etag {
            request = request.header("If-None-Match", etag);
        } else if let Some(date) = &validators.last_modified {
            request = request.header("If-Modified-Since", date);
        }
        let response = request.call().map_err(|e| DownloadError::Network(e.to_string()))?;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(|v| v.trim().to_string())
        };
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let location = header("location").ok_or(DownloadError::Status(status))?;
            let next = address.join(&location).map_err(|_| DownloadError::RedirectNotHttps)?;
            if next.scheme() != "https" {
                return Err(DownloadError::RedirectNotHttps);
            }
            redirects += 1;
            if redirects > MAX_REDIRECTS {
                return Err(DownloadError::TooManyRedirects);
            }
            address = next;
            continue;
        }
        if status == 304 {
            return Ok(Downloaded::NotModified);
        }
        if status != 200 {
            return Err(DownloadError::Status(status));
        }
        if header("content-length")
            .and_then(|v| v.parse::<u64>().ok())
            .is_some_and(|n| n > max_bytes as u64)
        {
            return Err(DownloadError::TooLarge);
        }
        let etag = header("etag").filter(|e| !e.is_empty() && e.len() <= 512);
        let last_modified = header("last-modified").filter(|d| !d.is_empty() && d.len() <= 128);
        // The cap is counted on the bytes as they arrive (after decompression), so a server
        // that lies about the length, or sends no length, still stops at the limit.
        let mut bytes = Vec::new();
        response
            .into_body()
            .into_with_config()
            .limit(u64::MAX)
            .reader()
            .take(max_bytes as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| DownloadError::Network(e.to_string()))?;
        if bytes.len() > max_bytes {
            return Err(DownloadError::TooLarge);
        }
        return Ok(Downloaded::File {
            bytes,
            etag,
            last_modified,
        });
    }
}

#[cfg(all(test, feature = "tls"))]
mod tests;
