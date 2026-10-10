//! The official map's files, per world, in the data directory:
//! `<data dir>/official-maps/<world>/map.xml` (the last imported file as it was downloaded;
//! `map.json` when the game publishes Mudlet's JSON export) and `meta.json` (where it came
//! from, its ETag and SHA-256, when it was imported, the last merge's counts, and whether the
//! person said Never). The file is the base of the next three-way merge ([`super::merge`]).
//!
//! Files are written to a temporary name and renamed, so a crash never leaves half a file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::merge::MergeReport;
use crate::map::store::MapWorld;

/// The folder under the data directory.
pub const FOLDER: &str = "official-maps";
const META: &str = "meta.json";

/// What is known about a world's official map.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    /// The address the imported file came from.
    pub url: Option<String>,
    /// The server's ETag for it, sent back as `If-None-Match`.
    pub etag: Option<String>,
    /// The server's `Last-Modified` for it, sent back as `If-Modified-Since` when there is no
    /// ETag.
    pub last_modified: Option<String>,
    /// SHA-256 of the imported file (lowercase hex).
    pub sha256: Option<String>,
    /// When it was imported (RFC 3339, UTC).
    pub imported_at: Option<String>,
    /// The file's name in the world's folder (`map.xml` or `map.json`).
    pub file: Option<String>,
    /// What the last import did.
    pub report: Option<MergeReport>,
    /// The person chose Never: the notice is not shown for this world again.
    pub never: bool,
    /// When the server was last asked whether its file changed (RFC 3339, UTC), and at which
    /// address: one check a day per world ([`super::offer::CHECK_INTERVAL_SECS`]).
    pub last_checked_at: Option<String>,
    pub last_checked_url: Option<String>,
    /// That check found a file different from the one imported.
    pub check_found_change: bool,
}

impl Record {
    /// Whether the file at `url` was imported before.
    pub fn imported(&self, url: &str) -> bool {
        self.url.as_deref() == Some(url) && self.sha256.is_some() && self.file.is_some()
    }

    /// What to send so the server can say the file at `url` has not changed: only for the file
    /// imported from that very address.
    pub fn validators(&self, url: &str) -> Validators {
        if self.imported(url) {
            Validators {
                etag: self.etag.clone(),
                last_modified: self.last_modified.clone(),
            }
        } else {
            Validators::default()
        }
    }
}

/// What the client knows of the file it has, sent so the server can answer 304: the ETag first,
/// else the `Last-Modified` date.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Validators {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// The folder name of a world: its saved id, or its address when it has none. Only letters,
/// digits, `-` and `_` are kept, so the name never leaves the folder.
pub fn world_key(world: &MapWorld) -> String {
    let raw = match world {
        MapWorld::Id(id) => id.clone(),
        MapWorld::Endpoint { host, port } => format!("endpoint-{}-{port}", host.to_lowercase()),
    };
    let key: String = raw
        .chars()
        .take(120)
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if key.is_empty() { "_".into() } else { key }
}

/// SHA-256 of `bytes`, lowercase hex.
pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Now, in seconds since 1970 (UTC).
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Now, as RFC 3339 (UTC, whole seconds).
pub fn now_text() -> String {
    time_text(now_secs())
}

/// A time as RFC 3339 (UTC, whole seconds).
pub fn time_text(secs: i64) -> String {
    let (y, m, d) = crate::directory::time::civil_from_days(secs.div_euclid(86_400));
    let s = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        s / 3600,
        s % 3600 / 60,
        s % 60
    )
}

/// The official maps of every world, under one data directory.
#[derive(Clone, Debug)]
pub struct OfficialStore {
    root: PathBuf,
}

impl OfficialStore {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join(FOLDER),
        }
    }

    /// A world's folder.
    pub fn dir(&self, key: &str) -> PathBuf {
        self.root.join(key)
    }

    /// The world's record; an empty one when there is none or it cannot be read.
    pub fn load(&self, key: &str) -> Record {
        std::fs::read(self.dir(key).join(META))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, key: &str, record: &Record) -> std::io::Result<()> {
        let text = serde_json::to_vec_pretty(record).map_err(std::io::Error::other)?;
        write_atomic(&self.dir(key), META, &text)
    }

    /// The last imported file, when the record names one and it is still there.
    pub fn base(&self, key: &str, record: &Record) -> Option<Vec<u8>> {
        let name = record.file.as_deref().filter(|n| valid_file_name(n))?;
        let bytes = std::fs::read(self.dir(key).join(name)).ok()?;
        // A file changed on disk since is no base: the merge would trust the wrong thing.
        (record.sha256.as_deref() == Some(sha256(&bytes).as_str())).then_some(bytes)
    }

    /// Keep a newly imported file as the world's base, and its record: the file first, so the
    /// record never names a file that is not there. Another kind of file left behind goes.
    pub fn keep(&self, key: &str, name: &str, bytes: &[u8], record: &Record) -> std::io::Result<()> {
        if !valid_file_name(name) {
            return Err(std::io::Error::other("invalid file name"));
        }
        let dir = self.dir(key);
        write_atomic(&dir, name, bytes)?;
        self.save(key, record)?;
        for other in FILE_NAMES.iter().filter(|n| **n != name) {
            let _ = std::fs::remove_file(dir.join(other));
        }
        Ok(())
    }
}

/// The names the world's file may have.
pub const FILE_NAMES: [&str; 2] = ["map.xml", "map.json"];

fn valid_file_name(name: &str) -> bool {
    FILE_NAMES.contains(&name)
}

/// The file's name for downloaded bytes: `map.xml` for XML, else `map.json`.
pub fn file_name(bytes: &[u8]) -> &'static str {
    if crate::map::mudlet::xml::looks_like_xml(bytes) {
        "map.xml"
    } else {
        "map.json"
    }
}

fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let temporary = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, dir.join(name))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wandur-official-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn world_keys_stay_inside_the_folder() {
        assert_eq!(world_key(&MapWorld::Id("w-1_a".into())), "w-1_a");
        assert_eq!(world_key(&MapWorld::Id("../../etc".into())), "______etc");
        assert_eq!(
            world_key(&MapWorld::Endpoint {
                host: "MUD.Fixture.Example".into(),
                port: 4000
            }),
            "endpoint-mud_fixture_example-4000"
        );
        assert_eq!(world_key(&MapWorld::Id(String::new())), "_");
    }

    #[test]
    fn records_and_files_round_trip_and_a_changed_file_is_no_base() {
        let dir = temp();
        let store = OfficialStore::new(&dir);
        assert_eq!(store.load("w"), Record::default(), "nothing yet");
        let bytes = b"<map/>".to_vec();
        let record = Record {
            url: Some("https://maps.fixture.example/map.xml".into()),
            etag: Some("\"v1\"".into()),
            sha256: Some(sha256(&bytes)),
            imported_at: Some(now_text()),
            file: Some(file_name(&bytes).into()),
            report: None,
            ..Record::default()
        };
        store.keep("w", "map.xml", &bytes, &record).unwrap();
        assert!(dir.join("official-maps/w/map.xml").exists());
        assert!(dir.join("official-maps/w/meta.json").exists());
        assert_eq!(store.load("w"), record);
        assert!(record.imported("https://maps.fixture.example/map.xml"));
        assert!(!record.imported("https://maps.fixture.example/other.xml"));
        assert_eq!(store.base("w", &record).as_deref(), Some(bytes.as_slice()));
        std::fs::write(dir.join("official-maps/w/map.xml"), b"<map></map>").unwrap();
        assert_eq!(store.base("w", &record), None, "changed on disk");
        // A JSON file replaces the XML one.
        let json = br#"{"areas":[]}"#;
        store.keep("w", file_name(json), json, &record).unwrap();
        assert!(!dir.join("official-maps/w/map.xml").exists());
        assert!(dir.join("official-maps/w/map.json").exists());
        assert!(store.keep("w", "../x", json, &record).is_err());
        // A broken record reads as none.
        std::fs::write(dir.join("official-maps/w/meta.json"), b"{broken").unwrap();
        assert_eq!(store.load("w"), Record::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_time_is_rfc3339() {
        let now = now_text();
        assert_eq!(now.len(), 20, "{now}");
        assert!(crate::directory::time::parse_rfc3339(&now).is_some(), "{now}");
    }
}
