//! The C# `UpdateServiceTests` and `InstallIdentityTests` network cases, against the loopback
//! directory bench. Every request in these tests goes to 127.0.0.1; the last test checks the
//! bench's request log for that.

use std::sync::Arc;

use wandur_core::directory::client::{Fetcher, HttpFetcher, resolve_base, user_agent};
use wandur_core::directory::install::{HEADER, InstallHeader, new_id};
use wandur_core::settings::Settings;
use wandur_core::updates::{HttpUpdateSource, UpdateCheckStatus, UpdateService, system_clock};

use crate::directory_server::{DirectoryServer, Route, latest_json};

/// A header's value in a request head, by case-insensitive name.
fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|line| {
        let (n, v) = line.split_once(':')?;
        n.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

fn settings_with(id: &str, send: bool) -> Settings {
    Settings {
        install_id: Some(id.into()),
        send_install_id: send,
        ..Settings::default()
    }
}

fn base(server: &DirectoryServer) -> String {
    resolve_base(Some(&server.address()), "")
}

/// UpdateServiceTests.TheCheckAsksTheDirectoryHostWithTheClientsUserAgent.
#[test]
fn the_check_asks_the_directory_host_with_the_clients_user_agent() {
    let server = DirectoryServer::start_fixture(0).unwrap();
    server.set_latest("200 OK", latest_json("0.1.6"));
    let source = HttpUpdateSource::new(&base(&server), HttpFetcher::new());
    let updates = UpdateService::new(Arc::new(source), system_clock(), "0.1.5");
    let result = updates.check(&Settings::default());
    assert_eq!(result.status, UpdateCheckStatus::Available);
    assert_eq!(result.latest.unwrap().version, "0.1.6");
    assert_eq!(
        result.record.unwrap().notes.as_deref(),
        Some("https://github.com/Last-Mile-Studio/wandur/releases/tag/v0.1.6")
    );
    let requests = server.requests();
    let request = requests.last().unwrap();
    assert!(request.starts_with("GET /client/latest "), "{request}");
    assert_eq!(header(request, "User-Agent"), Some(user_agent().as_str()));
    assert_eq!(header(request, "Accept"), Some("application/json"));

    // The site has no list yet: a 503, reported as a failure.
    server.set_latest("503 Service Unavailable", r#"{"error":"later"}"#.into());
    assert_eq!(updates.check(&Settings::default()).status, UpdateCheckStatus::Failed);
}

/// UpdateServiceTests.TestsNeverReachTheRealSite: the address is the configured directory's,
/// and a closed loopback port is a failure, not a fallback to wandur.net.
#[test]
fn the_check_follows_the_configured_directory_and_never_falls_back_to_the_site() {
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let base = resolve_base(Some(&format!("http://127.0.0.1:{closed}")), "");
    let source = HttpUpdateSource::new(&base, HttpFetcher::new());
    assert_eq!(source.address, format!("http://127.0.0.1:{closed}/client/latest"));
    let updates = UpdateService::new(Arc::new(source), system_clock(), "0.1.5");
    assert_eq!(updates.check(&Settings::default()).status, UpdateCheckStatus::Failed);
}

/// InstallIdentityTests.ArtworkAndOtherRequestsToTheSiteCarryNoId.
#[test]
fn artwork_and_other_requests_to_the_site_carry_no_id() {
    let server = DirectoryServer::start_fixture(0).unwrap();
    let install = InstallHeader::new(&base(&server), &settings_with(&new_id(), true));
    let http = HttpFetcher::new().with_install(install);
    let art = http
        .get(&format!("{}/worlds/emberwild/art", server.address()), 4 << 20)
        .unwrap();
    assert_eq!(art.status, 200);
    let request = server.requests().pop().unwrap();
    assert!(request.starts_with("GET /worlds/emberwild/art "), "{request}");
    assert_eq!(header(&request, HEADER), None);
}

/// InstallIdentityTests.TheCatalogSendsTheIdToTheDirectoryOnlyAndNeverInTheUserAgent.
#[test]
fn the_directory_request_carries_the_id_and_never_in_the_user_agent() {
    let server = DirectoryServer::start_fixture(0).unwrap();
    let id = new_id();
    let install = InstallHeader::new(&base(&server), &settings_with(&id, true));
    let http = HttpFetcher::new().with_install(install.clone());
    assert_eq!(
        http.get(&format!("{}/directory", server.address()), 1 << 20)
            .unwrap()
            .status,
        200
    );
    let request = server.requests().pop().unwrap();
    assert!(request.starts_with("GET /directory "), "{request}");
    assert_eq!(header(&request, HEADER), Some(id.as_str()));
    assert_eq!(header(&request, "User-Agent"), Some(user_agent().as_str()));
    assert!(!user_agent().contains(&id));

    // Off in the settings: the next request carries nothing.
    install.apply(&settings_with(&id, false));
    http.get(&format!("{}/directory", server.address()), 1 << 20).unwrap();
    let off = server.requests().pop().unwrap();
    assert!(off.starts_with("GET /directory "));
    assert_eq!(header(&off, HEADER), None);
    assert!(!off.contains(&id));
}

/// InstallIdentityTests.OtherHostsNeverSeeTheIdEvenThroughARedirect.
#[test]
fn other_hosts_never_see_the_id_even_through_a_redirect() {
    let other = DirectoryServer::start_fixture(0).unwrap();
    other.set_route(
        "banner.png",
        Route {
            status: "200 OK",
            content_type: "image/png",
            body: b"PNG!".to_vec(),
            location: None,
        },
    );
    let site = DirectoryServer::start_fixture(0).unwrap();
    site.set_route(
        "directory",
        Route {
            status: "302 Found",
            content_type: "text/plain",
            body: Vec::new(),
            location: Some(format!("{}/banner.png", other.address())),
        },
    );
    let id = new_id();
    let http = HttpFetcher::new().with_install(InstallHeader::new(&base(&site), &settings_with(&id, true)));

    // A banner on another host, asked for directly: no id.
    assert_eq!(
        http.get(&format!("{}/banner.png", other.address()), 1024).unwrap().body,
        b"PNG!"
    );
    assert_eq!(header(&other.requests().pop().unwrap(), HEADER), None);

    // The site redirects to the other host: the site sees the id, the other host does not.
    let answer = http.get(&format!("{}/directory", site.address()), 1024).unwrap();
    assert_eq!(answer.body, b"PNG!");
    assert_eq!(header(&site.requests().pop().unwrap(), HEADER), Some(id.as_str()));
    let redirected = other.requests().pop().unwrap();
    assert!(redirected.starts_with("GET /banner.png "), "{redirected}");
    assert_eq!(header(&redirected, HEADER), None);
    assert!(!redirected.contains(&id));
}

/// InstallIdentityTests.TheUpdateCheckSendsTheIdWhenItIsOn.
#[test]
fn the_update_check_sends_the_id_when_it_is_on() {
    let server = DirectoryServer::start_fixture(0).unwrap();
    server.set_latest("200 OK", r#"{ "version": "0.1.6" }"#.into());
    let id = new_id();
    let install = InstallHeader::new(&base(&server), &settings_with(&id, true));
    let source = HttpUpdateSource::new(&base(&server), HttpFetcher::new().with_install(install.clone()));
    let updates = UpdateService::new(Arc::new(source), system_clock(), "0.1.5");
    assert_eq!(updates.check(&Settings::default()).latest.unwrap().version, "0.1.6");
    let request = server.requests().pop().unwrap();
    assert!(request.starts_with("GET /client/latest "));
    assert_eq!(header(&request, HEADER), Some(id.as_str()));

    install.apply(&settings_with(&id, false));
    updates.check(&Settings::default());
    assert_eq!(header(&server.requests().pop().unwrap(), HEADER), None);

    // Without an install header at all.
    let plain = UpdateService::new(
        Arc::new(HttpUpdateSource::new(&base(&server), HttpFetcher::new())),
        system_clock(),
        "0.1.5",
    );
    plain.check(&Settings::default());
    let last = server.requests().pop().unwrap();
    assert_eq!(header(&last, HEADER), None);

    // Every request the bench received named a loopback host.
    for head in server.requests() {
        let host = header(&head, "Host").unwrap_or("");
        assert!(host.starts_with("127.0.0.1:"), "{head}");
    }
}
