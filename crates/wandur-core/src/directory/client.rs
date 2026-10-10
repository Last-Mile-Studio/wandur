//! Talking to the directory over HTTP: the snapshot (`GET {base}/directory`), artwork and the
//! update check (`GET {base}/client/latest`). Every request names the prototype in its
//! User-Agent (`WandurRustPrototype/<version> (<os>; <arch>)`) so wandur.net's statistics can
//! tell it from the C# client. The anonymous install id travels in its own header, only where
//! [`super::install`] allows it; redirects are followed here, one hop at a time, so the header is
//! decided again for every address and never rides a redirect to another host.
//!
//! `WANDUR_HTTP_LOG=<file>` appends one line per request (the address, and `+install` when the
//! header went with it), so a test run can show that nothing left loopback.

#[cfg(feature = "http")]
use std::time::Duration;

/// The public directory, used when neither the settings nor `WANDUR_DIRECTORY_URL` name another.
pub const PUBLIC_DIRECTORY: &str = "https://api.wandur.net/";
/// Largest snapshot accepted.
pub const MAX_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;
/// Largest picture accepted.
pub const MAX_ART_BYTES: u64 = 32 * 1024 * 1024;

/// The User-Agent value: product, version, operating system family, processor architecture.
pub fn user_agent() -> String {
    let os = if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(windows) {
        "Windows"
    } else if cfg!(target_os = "linux") {
        "Linux"
    } else {
        "Other"
    };
    format!(
        "WandurRustPrototype/{} ({os}; {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH
    )
}

/// The directory's base address with a trailing slash: `WANDUR_DIRECTORY_URL` when set, else the
/// setting, else the public directory. A value that is not an http or https address is ignored.
pub fn resolve_base(env: Option<&str>, setting: &str) -> String {
    for candidate in [env.unwrap_or(""), setting] {
        let c = candidate.trim();
        if (c.starts_with("http://") || c.starts_with("https://")) && c.len() > "https://".len() {
            return format!("{}/", c.trim_end_matches('/'));
        }
    }
    PUBLIC_DIRECTORY.into()
}

/// The address of a world's generated picture: the listing's path under the directory's base,
/// never anywhere else, with an optional size (`?size=400`).
pub fn generated_art_url(base: &str, path: &str, size: Option<&str>) -> Option<String> {
    let path = path.trim().trim_start_matches('/');
    if path.is_empty()
        || path.contains("..")
        || path.contains("://")
        || path.contains('?')
        || path.contains('#')
        || path.contains('\\')
        || path.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let mut url = format!("{base}{path}");
    if let Some(size) = size.filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric())) {
        url.push_str("?size=");
        url.push_str(size);
    }
    Some(url)
}

/// A supplied banner's address, if it is an absolute http or https address without credentials.
pub fn banner_url(url: &str) -> Option<String> {
    let url = url.trim();
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    (!authority.is_empty() && !authority.contains('@') && !url.chars().any(char::is_whitespace))
        .then(|| url.to_string())
}

/// An answer that says it is something other than a picture (HTML, JSON, text), is empty, or
/// opens like markup, is not one (the C# `LooksLikeImage`).
pub fn looks_like_image(content_type: Option<&str>, data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }
    if let Some(t) = content_type.map(str::trim).filter(|t| !t.is_empty()) {
        let t = t.to_ascii_lowercase();
        if !t.starts_with("image/") && !t.starts_with("application/octet-stream") {
            return false;
        }
    }
    let start = data.iter().position(|b| !b" \t\r\n".contains(b));
    start.is_some_and(|i| data[i] != b'<' && data[i] != b'{')
}

/// One HTTP answer.
#[derive(Debug)]
pub struct Fetched {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    /// The directory said it is serving a saved copy while it refreshes (`X-Wandur-Stale: true`).
    pub stale: bool,
}

/// Something that can GET a URL; the real one is [`HttpFetcher`], tests use their own.
pub trait Fetcher: Send + Sync {
    fn get(&self, url: &str, limit: u64) -> Result<Fetched, String>;
}

/// The real fetcher: ureq with rustls and the operating system's certificate verifier.
#[cfg(feature = "http")]
#[derive(Clone)]
pub struct HttpFetcher {
    agent: ureq::Agent,
    install: Option<super::install::InstallHeader>,
    log: Option<std::path::PathBuf>,
}

/// Redirect hops followed for one request (as C#).
pub const MAX_REDIRECTS: usize = 10;

/// Where a redirect from `current` points, or `None` when it is not one to follow: not a
/// redirect status, no `Location`, not http or https, or https to http.
pub fn redirect_target(current: &str, status: u16, location: Option<&str>) -> Option<String> {
    if !matches!(status, 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    let current = url::Url::parse(current).ok()?;
    let next = current.join(location?.trim()).ok()?;
    if !matches!(next.scheme(), "http" | "https") || (current.scheme() == "https" && next.scheme() == "http") {
        return None;
    }
    Some(next.to_string())
}

#[cfg(feature = "http")]
impl Default for HttpFetcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "http")]
impl HttpFetcher {
    pub fn new() -> Self {
        use ureq::tls::{RootCerts, TlsConfig};
        let config = ureq::Agent::config_builder()
            .user_agent(user_agent())
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_global(Some(Duration::from_secs(300)))
            .http_status_as_error(false)
            .max_redirects(0)
            .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
            .build();
        Self {
            agent: config.into(),
            install: None,
            log: std::env::var_os("WANDUR_HTTP_LOG").map(std::path::PathBuf::from),
        }
    }

    /// The same fetcher, adding the install id header where [`super::install`] allows it.
    pub fn with_install(mut self, install: super::install::InstallHeader) -> Self {
        self.install = Some(install);
        self
    }

    fn note(&self, url: &str, install: bool) {
        if let Some(path) = &self.log {
            use std::io::Write as _;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                // One write per line, so lines from several threads never interleave.
                let line = format!("GET {url}{}\n", if install { " +install" } else { "" });
                let _ = f.write_all(line.as_bytes());
            }
        }
    }

    /// GET `url`, asking for `accept` when given, following redirects of GET itself.
    pub fn request(&self, url: &str, limit: u64, accept: Option<&str>) -> Result<Fetched, String> {
        let mut address = url.to_string();
        for hop in 0..=MAX_REDIRECTS {
            let install = self.install.as_ref().and_then(|i| i.value_for(&address));
            self.note(&address, install.is_some());
            let mut request = self.agent.get(&address);
            if let Some(accept) = accept {
                request = request.header("Accept", accept);
            }
            if let Some(id) = &install {
                request = request.header(super::install::HEADER, id);
            }
            let response = request.call().map_err(|e| e.to_string())?;
            let status = response.status().as_u16();
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            if hop < MAX_REDIRECTS
                && let Some(next) = redirect_target(&address, status, location.as_deref())
            {
                address = next;
                continue;
            }
            return Self::read(response, limit);
        }
        Err("too many redirects".into())
    }

    fn read(response: ureq::http::Response<ureq::Body>, limit: u64) -> Result<Fetched, String> {
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let content_type = header("content-type");
        let stale = header("x-wandur-stale").is_some_and(|v| v.eq_ignore_ascii_case("true"));
        let body = response
            .into_body()
            .with_config()
            .limit(limit)
            .read_to_vec()
            .map_err(|e| e.to_string())?;
        Ok(Fetched {
            status,
            content_type,
            body,
            stale,
        })
    }
}

#[cfg(feature = "http")]
impl Fetcher for HttpFetcher {
    fn get(&self, url: &str, limit: u64) -> Result<Fetched, String> {
        self.request(url, limit, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_names_the_prototype_only() {
        let ua = user_agent();
        assert!(ua.starts_with("WandurRustPrototype/0."), "{ua}");
        assert!(ua.contains("; ") && ua.ends_with(')'));
        assert!(!ua.contains("WandurMudClient"));
    }

    #[test]
    fn base_address_resolution() {
        assert_eq!(
            resolve_base(Some("http://127.0.0.1:4401"), ""),
            "http://127.0.0.1:4401/"
        );
        assert_eq!(
            resolve_base(None, "https://example.org/api/"),
            "https://example.org/api/"
        );
        assert_eq!(resolve_base(Some("ftp://x"), "nonsense"), PUBLIC_DIRECTORY);
        assert_eq!(resolve_base(Some(" "), ""), PUBLIC_DIRECTORY);
    }

    #[test]
    fn artwork_addresses_stay_under_the_directory() {
        let base = "http://127.0.0.1:9/";
        assert_eq!(
            generated_art_url(base, "worlds/a/art", Some("400")).as_deref(),
            Some("http://127.0.0.1:9/worlds/a/art?size=400")
        );
        assert_eq!(
            generated_art_url(base, "/worlds/a/art", None).as_deref(),
            Some("http://127.0.0.1:9/worlds/a/art")
        );
        assert!(generated_art_url(base, "../secret", None).is_none());
        assert!(generated_art_url(base, "https://evil.example/x", None).is_none());
        assert!(generated_art_url(base, "", None).is_none());
        assert!(banner_url("https://cdn.example.org/b.png").is_some());
        assert!(banner_url("https://user:pw@cdn.example.org/b.png").is_none());
        assert!(banner_url("file:///etc/passwd").is_none());
    }

    #[test]
    fn redirects_are_followed_only_where_safe() {
        let here = "https://api.wandur.net/directory";
        assert_eq!(
            redirect_target(here, 302, Some("/v2/directory")).as_deref(),
            Some("https://api.wandur.net/v2/directory")
        );
        assert_eq!(
            redirect_target("http://127.0.0.1:9/directory", 307, Some("http://127.0.0.1:10/x")).as_deref(),
            Some("http://127.0.0.1:10/x")
        );
        assert_eq!(redirect_target(here, 302, Some("http://api.wandur.net/")), None);
        assert_eq!(redirect_target(here, 302, Some("ftp://x/")), None);
        assert_eq!(redirect_target(here, 200, Some("/x")), None);
        assert_eq!(redirect_target(here, 301, None), None);
    }

    #[test]
    fn image_sniffing() {
        assert!(looks_like_image(Some("image/jpeg"), b"\xff\xd8\xff"));
        assert!(looks_like_image(None, b"\x89PNG"));
        assert!(!looks_like_image(Some("text/html"), b"\xff\xd8"));
        assert!(!looks_like_image(None, b"  <html>"));
        assert!(!looks_like_image(Some("application/json"), b"{}"));
        assert!(!looks_like_image(Some("image/png"), b""));
    }
}
