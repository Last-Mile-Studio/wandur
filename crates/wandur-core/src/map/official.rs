//! A game's official map (the Mudlet Mapping Protocol): a server announces where its map file
//! is with GMCP `Client.Map {"url": "https://..."}`; the session offers it ("This game offers an
//! official map." with Download, Not now, Never); Download fetches the file over https, reads it
//! ([`super::mudlet`], XML or Mudlet's JSON) and brings it into the world's map.
//!
//! - [`client_map_url`]: the address a `Client.Map` message gives.
//! - [`store`]: the last imported file and what is known about it, per world, on disk at
//!   `<data dir>/official-maps/<world>/` (`map.xml` and `meta.json`).
//! - [`offer`]: whether to show the notice.
//! - [`merge`]: bringing a new version in over the old one without losing the person's edits
//!   (three-way, against the last imported file).
//! - [`download`]: https only, few redirects, a size cap while streaming, `If-None-Match`.
//!
//! Clean room: Mudlet is GPL-3.0 and none of its source was read. `Client.Map` is described on
//! the public Mudlet wiki page "Manual:GMCP Extensions" (Automatic map download), the file
//! format on "Standards:MMP".

use crate::protocol::GmcpMessage;

#[cfg(feature = "http")]
pub mod download;
pub mod merge;
pub mod offer;
pub mod store;

/// Longest map address accepted.
pub const MAX_URL_LENGTH: usize = 2048;

/// The map address of a `Client.Map` message: an absolute https address without credentials,
/// or `None` (another package, no `url`, not https).
pub fn client_map_url(message: &GmcpMessage) -> Option<String> {
    if !message.is("Client.Map") {
        return None;
    }
    let url = message.data.as_ref()?.as_object()?.get("url")?.as_str()?.trim();
    valid_url(url).then(|| url.to_string())
}

/// Whether `url` is one a map may be fetched from: absolute https, with a host, no user name or
/// password, not too long.
pub fn valid_url(url: &str) -> bool {
    if url.len() > MAX_URL_LENGTH || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    url::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str().is_some_and(|h| !h.is_empty())
            && u.username().is_empty()
            && u.password().is_none()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::parse_gmcp;

    fn url_of(payload: &str) -> Option<String> {
        client_map_url(&parse_gmcp(payload.as_bytes()).unwrap())
    }

    #[test]
    fn client_map_gives_an_https_address() {
        assert_eq!(
            url_of(r#"Client.Map {"url": "https://maps.fixture.example/world/map.xml"}"#).as_deref(),
            Some("https://maps.fixture.example/world/map.xml")
        );
        assert_eq!(
            url_of(r#"client.map {"url": " https://maps.fixture.example/m.xml "}"#).as_deref(),
            Some("https://maps.fixture.example/m.xml"),
            "package names compare without case; the address is trimmed"
        );
        assert_eq!(
            url_of(r#"Client.Map {"url": "http://maps.fixture.example/m.xml"}"#),
            None
        );
        assert_eq!(
            url_of(r#"Client.Map {"url": "https://user:pw@maps.fixture.example/m.xml"}"#),
            None
        );
        assert_eq!(
            url_of(r#"Client.Map {"url": "ftp://maps.fixture.example/m.xml"}"#),
            None
        );
        assert_eq!(url_of(r#"Client.Map {"url": 5}"#), None);
        assert_eq!(url_of(r#"Client.Map {}"#), None);
        assert_eq!(url_of(r#"Client.Map"#), None);
        assert_eq!(url_of(r#"Client.Map {broken"#), None);
        assert_eq!(
            url_of(r#"Client.GUI {"url": "https://maps.fixture.example/m.xml"}"#),
            None
        );
        let long = format!(
            r#"Client.Map {{"url": "https://maps.fixture.example/{}"}}"#,
            "a".repeat(MAX_URL_LENGTH)
        );
        assert_eq!(url_of(&long), None);
    }
}
