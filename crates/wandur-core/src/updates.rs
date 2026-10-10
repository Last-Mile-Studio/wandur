//! The update check (the C# `UpdateService`): `GET {directory}/client/latest` from the same base
//! address the directory uses (`WANDUR_DIRECTORY_URL` in tests, so they stay on loopback), at
//! most once a day, remembered in the settings. It only reads: nothing is ever downloaded or
//! installed. A build from source (version 0.0.0) never asks and is never offered anything,
//! unless [`OVERRIDE_VARIABLE`] names the release version it should check as.
//!
//! This prototype's builds are builds from source unless the packaging scripts set
//! `WANDUR_RELEASE_VERSION` at compile time (see [`build_version`]).

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::settings::Settings;

/// Set to a release version (for example 0.1.4) to let a build from source check as that
/// version.
pub const OVERRIDE_VARIABLE: &str = "WANDUR_UPDATE_CHECK_VERSION";
/// Where Download goes when the answer names no trusted page.
pub const DOWNLOADS_PAGE: &str = "https://www.wandur.net/client/downloads";
/// The least time between automatic checks.
pub const INTERVAL_SECS: i64 = 24 * 60 * 60;
/// A remembered check this far in the future means the clock was set back: check again.
const FUTURE_SLACK_SECS: i64 = 5 * 60;
/// Largest answer read.
pub const MAX_ANSWER_BYTES: u64 = 256 * 1024;

/// A release version as the client and wandur.net spell it (0.1.5, 0.1.6-rc.2), compared by
/// semantic version rules: numbers as numbers, a prerelease before its release, build metadata
/// after `+` ignored. 0.0.0 with any suffix is a build from source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub prerelease: Vec<String>,
}

impl ReleaseVersion {
    /// Read a version: optional `v`, three numbers of at most nine digits, an optional
    /// prerelease of dot-separated `[0-9A-Za-z-]` identifiers, optional `+metadata`; at most
    /// 64 characters, surrounding spaces ignored.
    pub fn parse(text: &str) -> Option<Self> {
        if text.trim().is_empty() || text.chars().count() > 64 {
            return None;
        }
        let value = text.trim();
        let value = value.strip_prefix('v').unwrap_or(value);
        let value = value.split_once('+').map_or(value, |(v, _)| v);
        let (core, pre) = match value.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (value, None),
        };
        let mut numbers = core.split('.');
        let mut number = || {
            let part = numbers.next()?;
            (!part.is_empty() && part.len() <= 9 && part.bytes().all(|b| b.is_ascii_digit()))
                .then(|| part.parse::<u32>().ok())
                .flatten()
        };
        let (major, minor, patch) = (number()?, number()?, number()?);
        if numbers.next().is_some() {
            return None;
        }
        let prerelease = match pre {
            None => Vec::new(),
            Some(pre) => {
                let ids: Vec<String> = pre.split('.').map(str::to_string).collect();
                if ids
                    .iter()
                    .any(|id| id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
                {
                    return None;
                }
                ids
            }
        };
        Some(Self {
            major,
            minor,
            patch,
            prerelease,
        })
    }

    pub fn is_prerelease(&self) -> bool {
        !self.prerelease.is_empty()
    }

    /// A build from source: no release is ever numbered 0.0.0.
    pub fn is_development(&self) -> bool {
        self.major == 0 && self.minor == 0 && self.patch == 0
    }
}

/// Numeric identifiers compare as numbers and sort before alphanumeric ones, which compare as
/// text.
fn compare_identifier(a: &str, b: &str) -> Ordering {
    let numeric = |s: &str| s.bytes().all(|c| c.is_ascii_digit());
    match (numeric(a), numeric(b)) {
        (true, true) => {
            let (x, y) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
            x.len().cmp(&y.len()).then_with(|| x.cmp(y))
        }
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => a.cmp(b),
    }
}

impl Ord for ReleaseVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.is_prerelease(), other.is_prerelease()) {
                // A release outranks any prerelease of the same number.
                (false, false) => Ordering::Equal,
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                (true, true) => self
                    .prerelease
                    .iter()
                    .zip(&other.prerelease)
                    .map(|(a, b)| compare_identifier(a, b))
                    .find(|o| o.is_ne())
                    .unwrap_or_else(|| self.prerelease.len().cmp(&other.prerelease.len())),
            })
    }
}

impl PartialOrd for ReleaseVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for ReleaseVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if self.is_prerelease() {
            write!(f, "-{}", self.prerelease.join("."))?;
        }
        Ok(())
    }
}

/// What the directory says the newest release is: its version, the downloads page and the
/// release notes (both already checked: [`trusted_page`], [`trusted_notes`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateInfo {
    pub version: String,
    pub page: String,
    pub notes: Option<String>,
}

/// The last update check, kept in the settings. A failed check keeps the version learned
/// before and only moves the time.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateCheckRecord {
    /// Seconds since 1970 (UTC).
    pub checked_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateCheckStatus {
    UpToDate,
    Available,
    Failed,
    Disabled,
}

/// The outcome of one check, and the record to save (`None` when nothing was asked).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateCheckResult {
    pub status: UpdateCheckStatus,
    pub latest: Option<UpdateInfo>,
    pub record: Option<UpdateCheckRecord>,
}

/// Where the newest release comes from: [`HttpUpdateSource`] in the client, a fake in tests.
pub trait UpdateSource: Send + Sync {
    fn latest(&self) -> Result<UpdateInfo, String>;
}

/// The clock the service reads (tests set their own).
pub type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// The system clock.
pub fn system_clock() -> Clock {
    Arc::new(SystemTime::now)
}

fn unix(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

/// Reads `{directory}/client/latest`, asking for JSON, through the directory's HTTP client
/// (its User-Agent and, when it is on, the install id header).
#[cfg(feature = "http")]
pub struct HttpUpdateSource {
    pub address: String,
    fetcher: crate::directory::client::HttpFetcher,
}

#[cfg(feature = "http")]
impl HttpUpdateSource {
    /// `directory_base` ends with a slash (as [`crate::directory::client::resolve_base`] gives).
    pub fn new(directory_base: &str, fetcher: crate::directory::client::HttpFetcher) -> Self {
        Self {
            address: latest_address(directory_base),
            fetcher,
        }
    }
}

/// `{base}/client/latest`.
pub fn latest_address(directory_base: &str) -> String {
    format!("{}/client/latest", directory_base.trim_end_matches('/'))
}

#[cfg(feature = "http")]
impl UpdateSource for HttpUpdateSource {
    fn latest(&self) -> Result<UpdateInfo, String> {
        let answer = self
            .fetcher
            .request(&self.address, MAX_ANSWER_BYTES, Some("application/json"))?;
        if !(200..300).contains(&answer.status) {
            return Err(format!("HTTP {}", answer.status));
        }
        parse(std::str::from_utf8(&answer.body).map_err(|e| e.to_string())?)
    }
}

/// Read the answer. The version must be a release version; the downloads page is used only when
/// it is an https page on wandur.net, and the notes only on GitHub or wandur.net, so the client
/// never opens anywhere else.
pub fn parse(json: &str) -> Result<UpdateInfo, String> {
    let root: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let text = |name: &str| root.get(name).and_then(serde_json::Value::as_str);
    let version = text("version")
        .and_then(ReleaseVersion::parse)
        .filter(|v| !v.is_development())
        .ok_or_else(|| "The release version could not be read.".to_string())?;
    Ok(UpdateInfo {
        version: version.to_string(),
        page: text("page")
            .and_then(trusted_page)
            .unwrap_or_else(|| DOWNLOADS_PAGE.into()),
        notes: text("notes").and_then(trusted_notes),
    })
}

fn https_without_extras(text: &str) -> Option<url::Url> {
    let url = url::Url::parse(text.trim()).ok()?;
    (url.scheme() == "https" && url.username().is_empty() && url.password().is_none() && url.port().is_none())
        .then_some(url)
}

/// An https page on wandur.net (or a subdomain), on the default port, without user info.
pub fn trusted_page(text: &str) -> Option<String> {
    let url = https_without_extras(text)?;
    let host = url.host_str()?;
    (host == "wandur.net" || host.ends_with(".wandur.net")).then(|| url.to_string())
}

/// An https page on GitHub or wandur.net.
pub fn trusted_notes(text: &str) -> Option<String> {
    trusted_page(text).or_else(|| {
        let url = https_without_extras(text)?;
        (url.host_str()? == "github.com").then(|| url.to_string())
    })
}

/// This build's version: `WANDUR_RELEASE_VERSION` when the build set it (the packaging scripts
/// do), else `0.0.0-dev`, a build from source.
pub fn build_version() -> &'static str {
    match option_env!("WANDUR_RELEASE_VERSION") {
        Some(v) if !v.trim().is_empty() => v,
        _ => "0.0.0-dev",
    }
}

/// The version the check runs as: the build's own, or for a build from source the override
/// when it is a release version.
pub fn effective_version(build: &str, override_version: Option<&str>) -> String {
    let dev = ReleaseVersion::parse(build).is_some_and(|v| v.is_development());
    match override_version.and_then(ReleaseVersion::parse) {
        Some(chosen) if dev && !chosen.is_development() => chosen.to_string(),
        _ => build.to_string(),
    }
}

/// When to ask about a newer release and what to offer.
#[derive(Clone)]
pub struct UpdateService {
    source: Arc<dyn UpdateSource>,
    clock: Clock,
    running: String,
}

impl UpdateService {
    pub fn new(source: Arc<dyn UpdateSource>, clock: Clock, running_version: impl Into<String>) -> Self {
        Self {
            source,
            clock,
            running: running_version.into(),
        }
    }

    /// The service for this build: its own version, or the override from the environment when
    /// this is a build from source.
    pub fn for_this_build(source: Arc<dyn UpdateSource>, clock: Clock) -> Self {
        let override_version = std::env::var(OVERRIDE_VARIABLE).ok();
        Self::new(
            source,
            clock,
            effective_version(build_version(), override_version.as_deref()),
        )
    }

    pub fn running_version(&self) -> &str {
        &self.running
    }

    /// Seconds since 1970 by the service's clock.
    pub fn now(&self) -> i64 {
        unix((self.clock)())
    }

    /// False for a build from source, or for a version that cannot be read.
    pub fn checks_allowed(&self) -> bool {
        ReleaseVersion::parse(&self.running).is_some_and(|v| !v.is_development())
    }

    /// Whether an automatic check should run now: checks are on, this is a release build, and
    /// the last check is a day old (or lies in the future, after the clock was set back).
    pub fn is_due(&self, settings: &Settings) -> bool {
        if !settings.check_for_updates || !self.checks_allowed() {
            return false;
        }
        let Some(last) = &settings.last_update_check else {
            return true;
        };
        let now = self.now();
        now - last.checked_at >= INTERVAL_SECS || last.checked_at > now + FUTURE_SLACK_SECS
    }

    /// Whether `version` is newer than this build. Never for a build from source.
    pub fn is_newer(&self, version: &str) -> bool {
        if !self.checks_allowed() {
            return false;
        }
        match (ReleaseVersion::parse(&self.running), ReleaseVersion::parse(version)) {
            (Some(running), Some(candidate)) => candidate > running,
            _ => false,
        }
    }

    /// The release to show from the remembered check, or `None`: nothing newer, the version was
    /// skipped, or automatic checks are off.
    pub fn offer(&self, settings: &Settings) -> Option<UpdateInfo> {
        if !settings.check_for_updates {
            return None;
        }
        let last = settings.last_update_check.as_ref()?;
        let version = last.version.as_deref()?;
        if !self.is_newer(version) || is_skipped(settings, version) {
            return None;
        }
        Some(UpdateInfo {
            version: version.to_string(),
            page: last
                .page
                .as_deref()
                .and_then(trusted_page)
                .unwrap_or_else(|| DOWNLOADS_PAGE.into()),
            notes: last.notes.as_deref().and_then(trusted_notes),
        })
    }

    /// Ask once. No retries: a failure is reported and the time recorded, so the next try is a
    /// day later. Blocks on the network; call it off the UI thread.
    pub fn check(&self, settings: &Settings) -> UpdateCheckResult {
        if !self.checks_allowed() {
            return UpdateCheckResult {
                status: UpdateCheckStatus::Disabled,
                latest: None,
                record: None,
            };
        }
        match self.source.latest() {
            Ok(latest) => UpdateCheckResult {
                status: if self.is_newer(&latest.version) {
                    UpdateCheckStatus::Available
                } else {
                    UpdateCheckStatus::UpToDate
                },
                record: Some(UpdateCheckRecord {
                    checked_at: self.now(),
                    version: Some(latest.version.clone()),
                    page: Some(latest.page.clone()),
                    notes: latest.notes.clone(),
                }),
                latest: Some(latest),
            },
            Err(_) => UpdateCheckResult {
                status: UpdateCheckStatus::Failed,
                latest: None,
                record: Some(UpdateCheckRecord {
                    checked_at: self.now(),
                    ..settings.last_update_check.clone().unwrap_or_default()
                }),
            },
        }
    }
}

/// Whether the person skipped exactly this version.
pub fn is_skipped(settings: &Settings, version: &str) -> bool {
    match (
        settings
            .skipped_update_version
            .as_deref()
            .and_then(ReleaseVersion::parse),
        ReleaseVersion::parse(version),
    ) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
    use std::time::Duration;

    /// 2026-10-03 12:00 UTC, the C# tests' start.
    const START: u64 = 1_791_028_800;

    struct TestClock(Mutex<SystemTime>);
    impl TestClock {
        fn new() -> Arc<Self> {
            Arc::new(Self(Mutex::new(UNIX_EPOCH + Duration::from_secs(START))))
        }
        fn set_hours(&self, hours: i64) {
            let at = if hours >= 0 {
                UNIX_EPOCH + Duration::from_secs(START + hours.unsigned_abs() * 3600)
            } else {
                UNIX_EPOCH + Duration::from_secs(START - hours.unsigned_abs() * 3600)
            };
            *self.0.lock().unwrap() = at;
        }
        fn clock(self: &Arc<Self>) -> Clock {
            let me = Arc::clone(self);
            Arc::new(move || *me.0.lock().unwrap())
        }
    }

    #[derive(Default)]
    struct FakeSource {
        version: Mutex<String>,
        calls: AtomicUsize,
        fail: AtomicBool,
    }
    impl FakeSource {
        fn new(version: &str) -> Arc<Self> {
            let source = Self::default();
            *source.version.lock().unwrap() = version.into();
            Arc::new(source)
        }
    }
    impl UpdateSource for FakeSource {
        fn latest(&self) -> Result<UpdateInfo, String> {
            self.calls.fetch_add(1, AtomicOrdering::SeqCst);
            if self.fail.load(AtomicOrdering::SeqCst) {
                return Err("offline".into());
            }
            let version = self.version.lock().unwrap().clone();
            Ok(UpdateInfo {
                notes: Some(format!(
                    "https://github.com/Last-Mile-Studio/wandur/releases/tag/v{version}"
                )),
                version,
                page: DOWNLOADS_PAGE.into(),
            })
        }
    }

    fn v(text: &str) -> ReleaseVersion {
        ReleaseVersion::parse(text).unwrap_or_else(|| panic!("{text}"))
    }

    #[test]
    fn versions_compare_by_semantic_version_rules() {
        for (newer, older) in [
            ("0.1.10", "0.1.9"),
            ("0.2.0", "0.1.99"),
            ("1.0.0", "0.9.9"),
            ("0.1.5", "0.1.5-rc.2"),
            ("0.1.5-rc.10", "0.1.5-rc.2"),
            ("0.1.5-rc.2", "0.1.4"),
            ("1.0.0-alpha.1", "1.0.0-alpha"),
            ("1.0.0-alpha.beta", "1.0.0-alpha.1"),
            ("1.0.0-beta", "1.0.0-alpha.beta"),
            ("1.0.0-rc.1", "1.0.0-beta.11"),
            ("0.1.5", "0.0.0-dev"),
        ] {
            assert!(v(newer) > v(older), "{newer} > {older}");
            assert!(v(older) < v(newer), "{older} < {newer}");
        }
    }

    #[test]
    fn prefixes_build_metadata_and_spaces_are_ignored() {
        for (text, expected) in [
            ("v0.1.5", "0.1.5"),
            ("0.1.5+abc123", "0.1.5"),
            (" 0.1.6-rc.1 ", "0.1.6-rc.1"),
        ] {
            assert_eq!(v(text).to_string(), expected);
            assert_eq!(v(text).cmp(&v(expected)), Ordering::Equal);
        }
    }

    #[test]
    fn unreadable_versions_are_refused() {
        for text in ["", "0.1", "0.1.x", "0.1.5-", "0.1.5 beta", "99999999999.0.0"] {
            assert!(ReleaseVersion::parse(text).is_none(), "{text}");
        }
    }

    #[test]
    fn a_build_from_source_never_checks_and_is_never_offered_anything() {
        let source = FakeSource::new("0.1.5");
        let dev = UpdateService::new(source.clone(), TestClock::new().clock(), "0.0.0-dev");
        let settings = Settings {
            last_update_check: Some(UpdateCheckRecord {
                checked_at: START as i64 - 3 * 86_400,
                version: Some("0.1.5".into()),
                ..Default::default()
            }),
            ..Settings::default()
        };
        assert!(v("0.0.0-dev").is_development());
        assert!(!dev.checks_allowed());
        assert!(!dev.is_due(&Settings::default()));
        assert!(!dev.is_newer("0.1.5"));
        assert_eq!(dev.offer(&settings), None);
        let result = dev.check(&settings);
        assert_eq!(result.status, UpdateCheckStatus::Disabled);
        assert_eq!(result.record, None);
        assert_eq!(source.calls.load(AtomicOrdering::SeqCst), 0);
    }

    #[test]
    fn the_owner_can_let_a_build_from_source_check_as_a_release() {
        assert_eq!(effective_version("0.0.0-dev", Some("0.1.4")), "0.1.4");
        assert_eq!(effective_version("0.0.0-dev", Some("v0.1.4")), "0.1.4");
        assert_eq!(effective_version("0.0.0-dev", None), "0.0.0-dev");
        assert_eq!(effective_version("0.0.0-dev", Some("nonsense")), "0.0.0-dev");
        assert_eq!(effective_version("0.0.0-dev", Some("0.0.0-dev")), "0.0.0-dev");
        // A release build is what it is; the override only applies to a build from source.
        assert_eq!(effective_version("0.1.5", Some("0.1.1")), "0.1.5");
        // Tests are built from source.
        if option_env!("WANDUR_RELEASE_VERSION").is_none() {
            assert_eq!(build_version(), "0.0.0-dev");
        }
    }

    #[test]
    fn it_asks_at_most_once_a_day_and_a_restart_within_the_day_does_not_ask_again() {
        let clock = TestClock::new();
        let source = FakeSource::new("0.1.6");
        let updates = UpdateService::new(source.clone(), clock.clock(), "0.1.5");
        let mut settings = Settings::default();
        assert!(updates.is_due(&settings));
        let result = updates.check(&settings);
        assert_eq!(result.status, UpdateCheckStatus::Available);
        assert_eq!(result.record.as_ref().unwrap().checked_at, START as i64);
        settings.last_update_check = result.record;

        let restarted = UpdateService::new(source.clone(), clock.clock(), "0.1.5");
        *clock.0.lock().unwrap() = UNIX_EPOCH + Duration::from_secs(START + 23 * 3600 + 59 * 60);
        assert!(!restarted.is_due(&settings));
        assert_eq!(restarted.offer(&settings).unwrap().version, "0.1.6");
        clock.set_hours(24);
        assert!(restarted.is_due(&settings));

        // A clock set back far behind the last check does not stall checks forever.
        clock.set_hours(-48);
        assert!(restarted.is_due(&settings));

        // Turned off in Settings: never due, and nothing remembered is offered.
        clock.set_hours(5 * 24);
        let off = Settings {
            check_for_updates: false,
            ..settings.clone()
        };
        assert!(!restarted.is_due(&off));
        assert_eq!(restarted.offer(&off), None);
        assert_eq!(source.calls.load(AtomicOrdering::SeqCst), 1);
    }

    #[test]
    fn a_failure_is_reported_once_keeps_what_was_known_and_waits_a_day() {
        let clock = TestClock::new();
        let source = FakeSource::new("0.1.6");
        let updates = UpdateService::new(source.clone(), clock.clock(), "0.1.5");
        let mut settings = Settings {
            last_update_check: updates.check(&Settings::default()).record,
            ..Settings::default()
        };
        clock.set_hours(24);
        source.fail.store(true, AtomicOrdering::SeqCst);
        let failed = updates.check(&settings);
        assert_eq!(failed.status, UpdateCheckStatus::Failed);
        assert_eq!(source.calls.load(AtomicOrdering::SeqCst), 2, "one attempt, no retries");
        let record = failed.record.unwrap();
        assert_eq!(record.checked_at, START as i64 + 86_400);
        assert_eq!(record.version.as_deref(), Some("0.1.6"));
        settings.last_update_check = Some(record);
        assert!(!updates.is_due(&settings));
        assert_eq!(updates.offer(&settings).unwrap().version, "0.1.6");
    }

    #[test]
    fn only_a_newer_release_is_offered() {
        let clock = TestClock::new();
        for (running, latest, status) in [
            ("0.1.5", "0.1.5", UpdateCheckStatus::UpToDate),
            ("0.1.6", "0.1.5", UpdateCheckStatus::UpToDate),
            ("0.1.5-rc.2", "0.1.5", UpdateCheckStatus::Available),
            ("0.1.9", "0.1.10", UpdateCheckStatus::Available),
        ] {
            let updates = UpdateService::new(FakeSource::new(latest), clock.clock(), running);
            let result = updates.check(&Settings::default());
            assert_eq!(result.status, status);
            let offer = updates.offer(&Settings {
                last_update_check: result.record,
                ..Settings::default()
            });
            assert_eq!(offer.is_some(), status == UpdateCheckStatus::Available);
        }
    }

    #[test]
    fn a_skipped_version_stays_hidden_and_a_newer_one_shows_again() {
        let updates = UpdateService::new(FakeSource::new("0.1.6"), TestClock::new().clock(), "0.1.5");
        let record = UpdateCheckRecord {
            checked_at: START as i64,
            version: Some("0.1.6".into()),
            page: Some(DOWNLOADS_PAGE.into()),
            notes: None,
        };
        let settings = Settings {
            last_update_check: Some(record.clone()),
            skipped_update_version: Some("0.1.6".into()),
            ..Settings::default()
        };
        assert_eq!(updates.offer(&settings), None);
        let v_prefixed = Settings {
            skipped_update_version: Some("v0.1.6".into()),
            ..settings.clone()
        };
        assert_eq!(updates.offer(&v_prefixed), None);
        let newer = Settings {
            last_update_check: Some(UpdateCheckRecord {
                version: Some("0.1.7".into()),
                ..record
            }),
            ..settings
        };
        assert_eq!(updates.offer(&newer).unwrap().version, "0.1.7");
    }

    #[test]
    fn only_pages_on_wandur_and_notes_on_github_are_opened() {
        let info = parse(r#"{"version":"0.1.6","page":"https://evil.example/download","notes":"http://github.com/x"}"#)
            .unwrap();
        assert_eq!(info.page, DOWNLOADS_PAGE);
        assert_eq!(info.notes, None);
        let info = parse(
            r#"{"version":"0.1.6","page":"https://www.wandur.net/client/downloads","notes":"https://github.com/Last-Mile-Studio/wandur/releases/tag/v0.1.6"}"#,
        )
        .unwrap();
        assert_eq!(info.page, "https://www.wandur.net/client/downloads");
        assert_eq!(
            info.notes.as_deref(),
            Some("https://github.com/Last-Mile-Studio/wandur/releases/tag/v0.1.6")
        );
        assert_eq!(trusted_page("https://user@www.wandur.net/"), None);
        assert_eq!(trusted_page("https://wandur.net.evil.example/"), None);
        assert_eq!(trusted_page("https://www.wandur.net:8443/"), None);
        assert!(parse(r#"{"version":"0.0.0-dev"}"#).is_err());
        assert!(parse("not json").is_err());
    }

    #[test]
    fn the_address_is_under_the_directory_base() {
        assert_eq!(
            latest_address("http://127.0.0.1:5298/"),
            "http://127.0.0.1:5298/client/latest"
        );
        assert_eq!(
            latest_address("https://api.wandur.net/"),
            "https://api.wandur.net/client/latest"
        );
    }
}
