//! Whether to show "This game offers an official map." when a server sends `Client.Map`.
//!
//! - Never (saved per world in [`super::store::Record`]) hides it for good.
//! - Not now hides it for the rest of the session.
//! - A file imported before is checked first ([`Offer::Check`]): the notice shows only when the
//!   server's file changed since ([`after_check`]). The server is asked at most once a day per
//!   world ([`CHECK_INTERVAL_SECS`]); within that day the last check's answer stands. A new
//!   address is checked at once (the same file moved there is no news).

use super::store::Record;

/// Seconds between two checks of the same address.
pub const CHECK_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// What to do with a `Client.Map` address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offer {
    /// Show nothing.
    Silent,
    /// Show the notice.
    Ask,
    /// The file was imported before: ask the server whether it changed, and ask only if so.
    Check,
}

/// The decision for `url`, given the world's record, whether Not now was chosen in this session,
/// and the time now (seconds since 1970).
pub fn offer(record: &Record, url: &str, not_now: bool, now: i64) -> Offer {
    if record.never || not_now {
        Offer::Silent
    } else if record.sha256.is_none() {
        // Nothing imported yet: a new map.
        Offer::Ask
    } else if checked_recently(record, url, now) {
        // Within a day of the last check of this address: its answer stands.
        if record.check_found_change {
            Offer::Ask
        } else {
            Offer::Silent
        }
    } else {
        Offer::Check
    }
}

/// Whether `url` was checked less than [`CHECK_INTERVAL_SECS`] ago. A check time in the future
/// (the clock went back) does not count.
pub fn checked_recently(record: &Record, url: &str, now: i64) -> bool {
    record.last_checked_url.as_deref() == Some(url)
        && record
            .last_checked_at
            .as_deref()
            .and_then(crate::directory::time::parse_rfc3339)
            .is_some_and(|at| at <= now && now - at < CHECK_INTERVAL_SECS)
}

/// After a check: the server said it has not changed (`None`, a 304), or sent the file (its
/// SHA-256). Ask only when the file differs from the one imported (from whatever address).
pub fn after_check(record: &Record, downloaded_sha256: Option<&str>) -> Offer {
    match downloaded_sha256 {
        None => Offer::Silent,
        Some(sha) if record.sha256.as_deref() == Some(sha) => Offer::Silent,
        Some(_) => Offer::Ask,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::official::store::{self, OfficialStore, sha256};

    const URL: &str = "https://maps.fixture.example/map.xml";

    fn imported() -> Record {
        Record {
            url: Some(URL.into()),
            sha256: Some(sha256(b"v1")),
            file: Some("map.xml".into()),
            ..Record::default()
        }
    }

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn a_new_map_is_offered_and_not_now_hides_it_for_the_session() {
        assert_eq!(offer(&Record::default(), URL, false, NOW), Offer::Ask);
        assert_eq!(offer(&Record::default(), URL, true, NOW), Offer::Silent);
    }

    #[test]
    fn never_is_remembered_for_the_world() {
        let dir = std::env::temp_dir().join(format!("wandur-offer-{}", uuid::Uuid::new_v4().simple()));
        let store = OfficialStore::new(&dir);
        let mut record = store.load("world");
        record.never = true;
        store.save("world", &record).unwrap();
        // A later session reads it back.
        let later = OfficialStore::new(&dir).load("world");
        assert_eq!(offer(&later, URL, false, NOW), Offer::Silent);
        assert_eq!(
            offer(&store.load("other-world"), URL, false, NOW),
            Offer::Ask,
            "per world"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unchanged_imported_map_does_not_nag() {
        let record = imported();
        assert_eq!(offer(&record, URL, false, NOW), Offer::Check);
        assert_eq!(after_check(&record, None), Offer::Silent, "304 Not Modified");
        assert_eq!(after_check(&record, Some(&sha256(b"v1"))), Offer::Silent, "same bytes");
        assert_eq!(after_check(&record, Some(&sha256(b"v2"))), Offer::Ask, "changed");
    }

    #[test]
    fn the_server_is_asked_at_most_once_a_day_unless_the_address_changes() {
        let checked = |ago: i64, found_change: bool| Record {
            last_checked_at: Some(store::time_text(NOW - ago)),
            last_checked_url: Some(URL.into()),
            check_found_change: found_change,
            ..imported()
        };
        assert_eq!(
            offer(&checked(3600, false), URL, false, NOW),
            Offer::Silent,
            "checked an hour ago"
        );
        assert_eq!(
            offer(&checked(3600, true), URL, false, NOW),
            Offer::Ask,
            "that check found a change"
        );
        assert_eq!(
            offer(&checked(CHECK_INTERVAL_SECS, false), URL, false, NOW),
            Offer::Check,
            "a day later"
        );
        assert_eq!(
            offer(&checked(-60, false), URL, false, NOW),
            Offer::Check,
            "a check in the future"
        );
        // A new address is checked at once.
        let moved = "https://maps.fixture.example/new.xml";
        assert_eq!(offer(&checked(60, false), moved, false, NOW), Offer::Check);
        assert!(!checked_recently(&checked(60, false), moved, NOW));
    }
}
