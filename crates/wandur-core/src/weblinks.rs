//! Which links the client may open in a browser, as the C# `WebLinks`. A world controls both a
//! link and the text around it, so only plain web addresses on the public internet qualify:
//! absolute http or https; no user name or password in the address
//! (`https://bank.example@evil.example/` reads as one site and goes to another); no invisible or
//! direction-changing characters, even percent-escaped (a right-to-left override makes `gnp.exe`
//! read as `exe.png`); and a host written in plain ASCII, so a Cyrillic letter or a fullwidth
//! dot cannot pass for a familiar name. Hosts on this computer or a private network are refused
//! too. Opening still needs the person's confirmation (the "Open this link?" prompt).

use std::net::{Ipv4Addr, Ipv6Addr};

use unicode_general_category::{GeneralCategory, get_general_category};
use url::{Host, Url};

/// Longer addresses are refused.
pub const MAXIMUM_LENGTH: usize = 2048;

/// Why a link was not offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not a plain web address: another scheme, relative, malformed, too long, user info in it,
    /// hidden or look-alike characters, or a host that is not plain ASCII.
    NotAWebLink,
    /// A web address for this computer or a private network.
    LocalNetwork,
}

/// The address to open, or why it must not be opened.
pub fn check(url: &str) -> Result<Url, Refusal> {
    let url = url.trim();
    if url.is_empty() || url.len() > MAXIMUM_LENGTH || url.chars().any(is_hidden) {
        return Err(Refusal::NotAWebLink);
    }
    let parsed = Url::parse(url).map_err(|_| Refusal::NotAWebLink)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(Refusal::NotAWebLink);
    }
    // The address as written must start with the scheme and "//" (no "http:example.com").
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(Refusal::NotAWebLink);
    }
    // Hidden characters written as percent escapes show decoded in a browser's address bar.
    let decoded = percent_decode(parsed.as_str());
    if decoded.chars().any(|c| c != ' ' && is_hidden(c)) {
        return Err(Refusal::NotAWebLink);
    }
    let authority = authority(url);
    if !parsed.username().is_empty() || parsed.password().is_some() || authority.contains('@') {
        return Err(Refusal::NotAWebLink);
    }
    if authority.is_empty() || !ascii(authority) {
        return Err(Refusal::NotAWebLink);
    }
    let Some(host) = parsed.host() else {
        return Err(Refusal::NotAWebLink);
    };
    if is_local(&host) {
        return Err(Refusal::LocalNetwork);
    }
    Ok(parsed)
}

/// The address to open, or `None`.
pub fn accept(url: &str) -> Option<Url> {
    check(url).ok()
}

/// Characters that draw nothing, change direction, or are not text: format, control, private
/// use, unassigned, line and paragraph separators, and spaces.
fn is_hidden(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            get_general_category(c),
            GeneralCategory::Format
                | GeneralCategory::Control
                | GeneralCategory::PrivateUse
                | GeneralCategory::Surrogate
                | GeneralCategory::Unassigned
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
        )
}

fn ascii(text: &str) -> bool {
    text.chars().all(|c| c > ' ' && c < '\u{7f}')
}

/// The text between "//" and the first '/', '?', '#' or '\', as written.
fn authority(url: &str) -> &str {
    let Some(start) = url.find("//") else { return "" };
    let rest = &url[start + 2..];
    let end = rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len());
    &rest[..end]
}

/// Percent escapes decoded as UTF-8 (lossy), for looking at what a browser would show.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) = (
                bytes.get(i + 1).and_then(|b| (*b as char).to_digit(16)),
                bytes.get(i + 2).and_then(|b| (*b as char).to_digit(16)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_local(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(name) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        Host::Ipv4(a) => local_v4(*a),
        Host::Ipv6(a) => match a.to_ipv4_mapped() {
            Some(v4) => local_v4(v4),
            None => local_v6(*a),
        },
    }
}

fn local_v4(a: Ipv4Addr) -> bool {
    let [b0, b1, ..] = a.octets();
    b0 == 0
        || b0 == 10
        || b0 == 127
        || (b0 == 169 && b1 == 254)
        || (b0 == 172 && (16..=31).contains(&b1))
        || (b0 == 192 && b1 == 168)
        || (b0 == 100 && (64..=127).contains(&b1))
}

fn local_v6(a: Ipv6Addr) -> bool {
    let first = a.segments()[0];
    a.is_loopback()
        || a.is_unspecified()
        || (first & 0xffc0) == 0xfe80 // link-local
        || (first & 0xffc0) == 0xfec0 // site-local
        || (first & 0xfe00) == 0xfc00 // unique local
}

/// The web address drawn at character `at` of a transcript line, found the way the C# terminal
/// finds one: `http://` or `https://` up to whitespace, a quote or an angle bracket, then
/// trailing punctuation trimmed (and a closing bracket that has no opening one). Whether the
/// address may be opened is [`check`]'s question, not this one's.
pub fn link_at(line: &str, at: usize) -> Option<&str> {
    let mut search = 0;
    while let Some(found) = line[search..].find("http") {
        let start = search + found;
        let rest = &line[start..];
        let scheme = if rest.starts_with("https://") {
            8
        } else if rest.starts_with("http://") {
            7
        } else {
            search = start + 4;
            continue;
        };
        let length = rest
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`'))
            .unwrap_or(rest.len());
        search = start + length.max(scheme);
        if length <= scheme {
            continue;
        }
        let url = trim_url_end(&rest[..length]);
        let first = line[..start].chars().count();
        let last = first + url.chars().count();
        if (first..last).contains(&at) {
            return Some(url);
        }
    }
    None
}

/// Trailing sentence punctuation is not part of an address, nor is a closing bracket without
/// its opening one ("(see https://example.org/a)").
fn trim_url_end(mut url: &str) -> &str {
    while let Some(last) = url.chars().last() {
        if matches!(last, '!' | '"' | '\'' | ',' | '.' | ':' | ';' | '?') {
            url = &url[..url.len() - 1];
            continue;
        }
        let open = match last {
            ')' => '(',
            ']' => '[',
            '}' => '{',
            _ => break,
        };
        if url.matches(open).count() >= url.matches(last).count() {
            break;
        }
        url = &url[..url.len() - 1];
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_is_found_under_its_characters_with_trailing_punctuation_trimmed() {
        let line = "See https://www.wandur.net/help, then come back.";
        let start = line.find("https").unwrap();
        assert_eq!(link_at(line, start), Some("https://www.wandur.net/help"));
        assert_eq!(link_at(line, start + 20), Some("https://www.wandur.net/help"));
        // The comma after it and the text before it are not the link.
        assert_eq!(link_at(line, start + 27), None);
        assert_eq!(link_at(line, 0), None);
        assert_eq!(
            link_at("(map: https://example.org/a_(b)) ok", 10),
            Some("https://example.org/a_(b)")
        );
        assert_eq!(link_at("at <http://example.org/x>.", 6), Some("http://example.org/x"));
        assert_eq!(link_at("hello http:// world", 8), None);
        assert_eq!(link_at("ftp://example.org/", 2), None);
        // Character positions, not bytes.
        assert_eq!(link_at("Är https://example.org", 4), Some("https://example.org"));
        // The second of two links.
        let two = "http://a.example/ and https://b.example/";
        assert_eq!(link_at(two, two.find("https").unwrap() + 3), Some("https://b.example/"));
    }

    #[test]
    fn plain_web_addresses_are_accepted() {
        for url in [
            "https://www.wandur.net/worlds?id=1#top",
            "http://example.test:4000/",
            "HTTPS://Example.Test/Path",
            "https://example.com/some%20page",
            "https://example.com/caf%C3%A9",
            "https://example.test/users/@someone?mail=a@b",
            "http://172.32.0.1/",
            "http://8.8.8.8/",
            "http://[2606:4700::1111]/",
            "https://localhost.example.com/",
        ] {
            assert!(accept(url).is_some(), "{url}");
        }
    }

    #[test]
    fn other_schemes_and_relative_links_are_refused() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "mailto:someone@example.test",
            "ftp://example.test/",
            "ssh://example.test",
            "send:kill orc",
            "data:text/html,<b>x</b>",
            "//example.test/relative",
            "/relative/path",
            "",
            "   ",
        ] {
            assert!(accept(url).is_none(), "{url}");
        }
    }

    #[test]
    fn addresses_with_user_info_are_refused() {
        for url in [
            "https://user@example.test/",
            "https://user:secret@example.test/",
            "https://bank.example@evil.example/login",
            "http://:@example.test/",
        ] {
            assert!(accept(url).is_none(), "{url}");
        }
    }

    #[test]
    fn control_characters_spaces_and_huge_addresses_are_refused() {
        assert!(accept("https://example.test/a\u{7}b").is_none());
        assert!(accept("https://example.test/a b").is_none());
        assert!(accept(&format!("https://example.test/{}", "a".repeat(MAXIMUM_LENGTH))).is_none());
    }

    #[test]
    fn hidden_and_look_alike_characters_are_refused() {
        for url in [
            "https://wandur.net\u{202e}gnp.exe",
            "https://\u{430}pple.com/",
            "https://app\u{200b}le.com/",
            "https://example\u{ff0e}com/",
            "https://example.com/\u{2066}path",
            "https://example.com/\u{e000}",
            "https://example.com/a\u{2028}b",
            "https://example.com/%E2%80%AEgnp.exe",
            "https://example.com/a%E2%80%8Bb",
            "https://xn--pple-43d.com/\u{378}",
        ] {
            assert_eq!(check(url).err(), Some(Refusal::NotAWebLink), "{url:?}");
        }
    }

    #[test]
    fn the_punycode_form_of_a_host_is_a_plain_ascii_host() {
        assert_eq!(
            accept("https://xn--pple-43d.com/").unwrap().as_str(),
            "https://xn--pple-43d.com/"
        );
    }

    #[test]
    fn loopback_link_local_and_private_hosts_are_refused() {
        for url in [
            "http://localhost/",
            "http://LOCALHOST:8080/admin",
            "http://printer.localhost/",
            "http://127.0.0.1/",
            "http://127.1.2.3/",
            "http://0x7f.1/",
            "http://2130706433/",
            "http://0.0.0.0/",
            "http://10.0.0.1/",
            "http://172.16.5.4/",
            "http://172.31.255.255/",
            "http://192.168.1.1/",
            "http://169.254.169.254/latest/meta-data",
            "http://100.64.0.1/",
            "http://[::1]/",
            "http://[::]/",
            "http://[fe80::1]/",
            "http://[fd12:3456::1]/",
            "http://[::ffff:192.168.0.1]/",
            "http://127.0.0.1./",
            "http://192.168.1.1./admin",
            "http://10.0.0.1./",
            "http://0x7f.1./",
            "http://2130706433./",
            "http://169.254.169.254./latest/meta-data",
        ] {
            assert_eq!(check(url).err(), Some(Refusal::LocalNetwork), "{url}");
        }
    }
}
