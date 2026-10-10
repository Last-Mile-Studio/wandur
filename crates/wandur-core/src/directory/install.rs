//! The anonymous install id (the C# `InstallIdentity`): a random UUID kept in the settings and
//! sent in its own header ([`HEADER`]) so the site can count installs. It is never part of the
//! User-Agent. It goes only to wandur.net (https, any subdomain, default port) and to the
//! directory address the client is configured with, and even there only on the two requests the
//! site counts installs from ([`is_counted`]): the world list (`/directory`) and the update check
//! (`/client/latest`). Never to artwork, banners on other hosts, MUD servers or model servers,
//! never through a redirect to another host, and not at all when
//! [`Settings::send_install_id`](crate::settings::Settings::send_install_id) is off.

use std::path::Path;
use std::sync::{Arc, RwLock};

use crate::settings::Settings;

/// The request header that carries the id.
pub const HEADER: &str = "X-Wandur-Install";

/// A new random id, hyphenated (the C# `Guid.ToString("D")` form).
pub fn new_id() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string()
}

/// The id in its hyphenated lowercase form, or `None` when it is not a UUID or is all zeros.
pub fn normalize_id(text: &str) -> Option<String> {
    uuid::Uuid::parse_str(text.trim())
        .ok()
        .filter(|id| !id.is_nil())
        .map(|id| id.hyphenated().to_string())
}

/// The header value these settings allow: `None` when sending is off or there is no id yet.
pub fn header_value(settings: &Settings) -> Option<String> {
    if !settings.send_install_id {
        return None;
    }
    settings.install_id.as_deref().and_then(normalize_id)
}

/// The settings to save when `stored` is the id already in the settings file: the stored id
/// when there is one, else the id they carry, else a new one. `None` when nothing changes.
pub fn keep(settings: &Settings, stored: Option<String>) -> Option<Settings> {
    let own = settings.install_id.as_deref().and_then(normalize_id);
    let id = stored.or_else(|| own.clone()).unwrap_or_else(new_id);
    (settings.install_id.as_deref() != Some(id.as_str())).then(|| Settings {
        install_id: Some(id),
        ..settings.clone()
    })
}

/// The install id already in a settings file, or `None` when there is none or the file cannot
/// be read (or is over 1 MiB, as C#).
pub fn stored_id(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > 1_048_576 {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("install_id")?.as_str().and_then(normalize_id)
}

/// Whether `address` is a request to wandur.net: an https address on wandur.net or one of its
/// subdomains on the default port, or the same scheme, host and port as the configured
/// directory. Addresses with user info never are.
pub fn is_wandur_net(address: &url::Url, directory_base: &url::Url) -> bool {
    if has_user_info(address) {
        return false;
    }
    let host = address
        .host_str()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if address.scheme() == "https"
        && address.port().is_none()
        && (host == "wandur.net" || host.ends_with(".wandur.net"))
    {
        return true;
    }
    !has_user_info(directory_base)
        && address.scheme() == directory_base.scheme()
        && address.host_str().map(str::to_ascii_lowercase) == directory_base.host_str().map(str::to_ascii_lowercase)
        && address.port_or_known_default() == directory_base.port_or_known_default()
}

fn has_user_info(url: &url::Url) -> bool {
    !url.username().is_empty() || url.password().is_some()
}

/// Whether `address` is one the site counts installs from: the world list (`/directory`) or
/// the update check (`/client/latest`), under any base path; a trailing slash and a query are
/// allowed.
pub fn is_counted(address: &url::Url) -> bool {
    let path = address.path().trim_end_matches('/');
    path.ends_with("/directory") || path.ends_with("/client/latest")
}

/// The header in use, shared by the client's HTTP fetchers and updated when the settings or the
/// directory address change (the C# `InstallHeader`).
#[derive(Clone, Default)]
pub struct InstallHeader {
    inner: Arc<RwLock<Policy>>,
}

#[derive(Default)]
struct Policy {
    base: Option<url::Url>,
    value: Option<String>,
}

impl InstallHeader {
    pub fn new(directory_base: &str, settings: &Settings) -> Self {
        let header = Self::default();
        header.set_base(directory_base);
        header.apply(settings);
        header
    }

    /// Follow the settings: the id when sending is on, nothing otherwise.
    pub fn apply(&self, settings: &Settings) {
        if let Ok(mut policy) = self.inner.write() {
            policy.value = header_value(settings);
        }
    }

    /// The configured directory address (its scheme, host and port count as wandur.net).
    pub fn set_base(&self, directory_base: &str) {
        if let Ok(mut policy) = self.inner.write() {
            policy.base = url::Url::parse(directory_base).ok();
        }
    }

    /// The header value to send to `address`, if any.
    pub fn value_for(&self, address: &str) -> Option<String> {
        let address = url::Url::parse(address).ok()?;
        let policy = self.inner.read().ok()?;
        let value = policy.value.clone()?;
        let wandur = match &policy.base {
            Some(base) => is_wandur_net(&address, base),
            None => is_wandur_net(&address, &url::Url::parse("https://api.wandur.net/").ok()?),
        };
        (wandur && is_counted(&address)).then_some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> url::Url {
        url::Url::parse(text).unwrap()
    }

    /// InstallIdentityTests.OnlyWandurNetAndTheConfiguredDirectoryCount.
    #[test]
    fn only_wandur_net_and_the_configured_directory_count() {
        let base = url("http://127.0.0.1:5298/");
        for (address, expected) in [
            ("https://api.wandur.net/directory", true),
            ("https://www.wandur.net/client/downloads", true),
            ("https://wandur.net/", true),
            ("http://api.wandur.net/directory", false),
            ("https://api.wandur.net:8443/directory", false),
            ("https://evilwandur.net/", false),
            ("https://wandur.net.example.com/", false),
            ("https://user@api.wandur.net/", false),
            ("https://www.mudconnector.com/mud/x", false),
            ("https://api.openai.com/v1/chat/completions", false),
            ("http://127.0.0.1:5298/directory", true),
            ("http://127.0.0.1:5299/directory", false),
            ("http://localhost:5298/directory", false),
        ] {
            assert_eq!(is_wandur_net(&url(address), &base), expected, "{address}");
        }
    }

    /// InstallIdentityTests.OnlyTheWorldListAndTheUpdateCheckCarryTheId.
    #[test]
    fn only_the_world_list_and_the_update_check_carry_the_id() {
        for (address, expected) in [
            ("https://api.wandur.net/directory", true),
            ("https://api.wandur.net/directory/", true),
            ("https://api.wandur.net/directory?since=1", true),
            ("https://api.wandur.net/client/latest", true),
            ("http://127.0.0.1:5298/base/directory", true),
            ("https://api.wandur.net/", false),
            ("https://api.wandur.net/art/world-1.png", false),
            ("https://api.wandur.net/themes/ember.png", false),
            ("https://api.wandur.net/directory/world-1", false),
            ("https://api.wandur.net/client/downloads", false),
        ] {
            assert_eq!(is_counted(&url(address)), expected, "{address}");
        }
    }

    /// InstallIdentityTests.TheHeaderValueFollowsTheSetting.
    #[test]
    fn the_header_value_follows_the_setting() {
        let id = new_id();
        let on = Settings {
            install_id: Some(id.clone()),
            ..Settings::default()
        };
        assert_eq!(header_value(&on).as_deref(), Some(id.as_str()));
        let off = Settings {
            send_install_id: false,
            ..on.clone()
        };
        assert_eq!(header_value(&off), None);
        assert_eq!(header_value(&Settings::default()), None);

        let header = InstallHeader::new("http://127.0.0.1:5298/", &on);
        assert_eq!(header.value_for("http://127.0.0.1:5298/directory"), Some(id.clone()));
        assert_eq!(
            header.value_for("http://127.0.0.1:5298/client/latest"),
            Some(id.clone())
        );
        assert_eq!(header.value_for("http://127.0.0.1:5298/worlds/a/art"), None);
        assert_eq!(header.value_for("http://127.0.0.1:5299/directory"), None);
        assert_eq!(header.value_for("https://cdn.example.org/directory"), None);
        header.apply(&off);
        assert_eq!(header.value_for("http://127.0.0.1:5298/directory"), None);
    }

    #[test]
    fn ids_are_normalized_and_kept() {
        assert_eq!(normalize_id("00000000-0000-0000-0000-000000000000"), None);
        assert_eq!(normalize_id("nonsense"), None);
        let id = new_id();
        assert_eq!(normalize_id(&id.to_uppercase()), Some(id.clone()));
        let settings = Settings {
            install_id: Some(new_id()),
            ..Settings::default()
        };
        // The stored id wins; without one the settings' own; without that a new one.
        assert_eq!(keep(&settings, Some(id.clone())).unwrap().install_id, Some(id));
        assert!(keep(&settings, None).is_none());
        let made = keep(&Settings::default(), None).unwrap();
        assert!(made.install_id.as_deref().and_then(normalize_id).is_some());
    }
}
