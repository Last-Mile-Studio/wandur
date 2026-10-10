//! The wandur.net pages the client links to (Help > Getting Started and Other MUD Clients), as
//! the C# `WandurSite`. The address follows the directory's configuration: `WANDUR_SITE_URL`
//! when set, else `WANDUR_DIRECTORY_URL` (a local site serves the pages and the API from one
//! address, and tests point it at loopback), else the public site. So a development build or a
//! test run never opens a production page it did not ask for.

/// The public site.
pub const PUBLIC_ADDRESS: &str = "https://www.wandur.net/";

/// The base address from the two settings: the site's own first, then the directory's, then the
/// public site. A value that is not an absolute http or https address is ignored. Always ends
/// with a slash.
pub fn resolve(site: Option<&str>, directory: Option<&str>) -> String {
    for candidate in [site, directory].into_iter().flatten() {
        let trimmed = candidate.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            continue;
        }
        let with_slash = format!("{trimmed}/");
        if let Ok(url) = url::Url::parse(&with_slash)
            && matches!(url.scheme(), "http" | "https")
            && url.host().is_some()
        {
            return url.to_string();
        }
    }
    PUBLIC_ADDRESS.to_string()
}

/// The base address from the environment.
pub fn base() -> String {
    let site = std::env::var("WANDUR_SITE_URL").ok();
    let directory = std::env::var("WANDUR_DIRECTORY_URL").ok();
    resolve(site.as_deref(), directory.as_deref())
}

/// A page under the base address.
pub fn page(base: &str, path: &str) -> String {
    format!("{}{}", base, path.trim_start_matches('/'))
}

/// The online help's home page.
pub fn help(base: &str) -> String {
    page(base, "client/help")
}

/// The site's page about other MUD clients.
pub fn other_clients(base: &str) -> String {
    page(base, "clients")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_site_follows_its_own_setting_then_the_directory_then_the_public_site() {
        assert_eq!(
            resolve(Some("http://127.0.0.1:5298"), Some("http://127.0.0.1:9/")),
            "http://127.0.0.1:5298/"
        );
        assert_eq!(resolve(None, Some("http://127.0.0.1:9")), "http://127.0.0.1:9/");
        assert_eq!(resolve(Some("  "), Some("ftp://x/")), PUBLIC_ADDRESS);
        assert_eq!(resolve(Some("not a url"), None), PUBLIC_ADDRESS);
        let base = resolve(None, Some("http://127.0.0.1:9/api/"));
        assert_eq!(help(&base), "http://127.0.0.1:9/api/client/help");
        assert_eq!(other_clients(&base), "http://127.0.0.1:9/api/clients");
    }
}
