//! Saved maps in `wandur.db` (the C# `SqliteRoomMapStore`): one normalized set of rows per world
//! (rooms, exits, areas, aliases, labels, tombstones, and the labels' pictures), keyed by the world id so a map follows a world
//! whose address changes. Every save merges with what is stored inside one write transaction
//! ([`super::merge::combine`]), so two sessions of one world keep both their discoveries and an
//! older session cannot bring back what another deleted. The player's position is never saved.
//!
//! [`MapWorker`] does loads and saves on a thread of its own, so the UI never waits on SQLite.
//!
//! The C# store also imports its older per-endpoint JSON map files once; the Rust client never
//! wrote those, so there is nothing to import.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use rusqlite::{Connection, OptionalExtension, params};

use super::format;
use super::merge::combine;
use super::model::*;
use crate::db::{Database, DbError, worlds};

/// Which world a map belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MapWorld {
    /// A saved world's id.
    Id(String),
    /// An address, resolved to its world (one is made when the address is new).
    Endpoint { host: String, port: u16 },
}

#[derive(Clone, Debug)]
pub struct MapStore {
    db: Database,
}

fn invalid(e: impl std::fmt::Display) -> DbError {
    DbError::Invalid(e.to_string())
}

const TABLES: [&str; 8] = [
    "map_rooms",
    "map_links",
    "map_areas",
    "map_room_aliases",
    "map_room_deletions",
    "map_link_deletions",
    "map_labels",
    "map_label_deletions",
];

/// Whether the database has this table (the C# client's has no label tables).
fn has_table(conn: &Connection, table: &str) -> Result<bool, DbError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

impl MapStore {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// The world's saved map, or `None` when it has none.
    pub fn load(&self, world: &MapWorld) -> Result<Option<MapSnapshot>, DbError> {
        self.db.read(|conn| {
            let id = match world {
                MapWorld::Id(id) => Some(id.clone()),
                MapWorld::Endpoint { host, port } => worlds::find_world(conn, host, *port)?,
            };
            match id {
                Some(id) => read_map(conn, &id),
                None => Ok(None),
            }
        })
    }

    /// Merge `snapshot` into the world's saved map; returns the merged map.
    pub fn save(&self, world: &MapWorld, snapshot: &MapSnapshot) -> Result<MapSnapshot, DbError> {
        format::validate(snapshot).map_err(invalid)?;
        self.db.write(|tx| {
            let id = match world {
                MapWorld::Id(id) => {
                    if !worlds::valid_world_id(id) {
                        return Err(DbError::InvalidEndpoint(id.clone()));
                    }
                    tx.execute("INSERT OR IGNORE INTO worlds(id) VALUES(?1)", [id])?;
                    id.clone()
                }
                MapWorld::Endpoint { host, port } => worlds::resolve_world(tx, host, *port, None)?,
            };
            let merged = match read_map(tx, &id)? {
                Some(current) => combine(&current, snapshot),
                None => snapshot.clone(),
            };
            write_map(tx, &id, &merged)?;
            Ok(merged)
        })
    }

    /// Rooms of a world whose observed name or description holds every term.
    pub fn search_rooms(&self, world: &MapWorld, query: &str) -> Result<Vec<MapRoom>, DbError> {
        Ok(self
            .load(world)?
            .map(|map| super::search::search(&map.rooms, query).into_iter().cloned().collect())
            .unwrap_or_default())
    }
}

fn rows<T: serde::de::DeserializeOwned>(conn: &Connection, table: &str, world: &str) -> Result<Vec<T>, DbError> {
    let mut s = conn.prepare(&format!("SELECT payload FROM {table} WHERE world_id=?1 ORDER BY rowid"))?;
    let payloads = s
        .query_map([world], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    payloads
        .iter()
        .map(|p| serde_json::from_str(p).map_err(invalid))
        .collect()
}

pub(crate) fn read_map(conn: &Connection, world: &str) -> Result<Option<MapSnapshot>, DbError> {
    let metadata: Option<String> = conn
        .query_row("SELECT payload FROM map_snapshots WHERE world_id=?1", [world], |r| {
            r.get(0)
        })
        .optional()?;
    let Some(metadata) = metadata else {
        return Ok(None);
    };
    let mut map: MapSnapshot = serde_json::from_str(&metadata).map_err(invalid)?;
    map.rooms = rows(conn, "map_rooms", world)?;
    map.links = rows(conn, "map_links", world)?;
    map.area_settings = rows(conn, "map_areas", world)?;
    let mut s =
        conn.prepare("SELECT source_id, target_id, revision FROM map_room_aliases WHERE world_id=?1 ORDER BY rowid")?;
    map.room_aliases = s
        .query_map([world], |r| {
            Ok(MapRoomAlias {
                source_id: r.get(0)?,
                target_id: r.get(1)?,
                revision: r.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let mut s = conn.prepare("SELECT room_id, revision FROM map_room_deletions WHERE world_id=?1 ORDER BY rowid")?;
    map.deleted_rooms = s
        .query_map([world], |r| {
            Ok(MapRoomDeletion {
                id: r.get(0)?,
                revision: r.get(1)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let mut s =
        conn.prepare("SELECT from_id, direction, revision FROM map_link_deletions WHERE world_id=?1 ORDER BY rowid")?;
    map.deleted_links = s
        .query_map([world], |r| {
            Ok(MapLinkDeletion {
                from_id: r.get(0)?,
                direction: r.get(1)?,
                revision: r.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    if has_table(conn, "map_labels")? {
        map.labels = rows(conn, "map_labels", world)?;
        let mut s =
            conn.prepare("SELECT label_id, revision FROM map_label_deletions WHERE world_id=?1 ORDER BY rowid")?;
        map.deleted_labels = s
            .query_map([world], |r| {
                Ok(MapLabelDeletion {
                    id: r.get(0)?,
                    revision: r.get(1)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        let mut s = conn.prepare(
            "SELECT hash, bytes FROM map_images WHERE world_id=?1 AND hash IN
                 (SELECT image FROM map_labels WHERE world_id=?1 AND image IS NOT NULL) ORDER BY rowid",
        )?;
        map.images = s
            .query_map([world], |r| {
                Ok(MapImage {
                    hash: r.get(0)?,
                    data: std::sync::Arc::from(r.get::<_, Vec<u8>>(1)?),
                })
            })?
            .collect::<Result<_, _>>()?;
    } else {
        map.labels.clear();
        map.deleted_labels.clear();
        map.images.clear();
    }
    map.current_room_id = None;
    map.candidate_room_ids.clear();
    map.state = TrackingState::Unknown;
    // Stored rows are checked before anything uses them.
    format::validate(&map).map_err(invalid)?;
    Ok(Some(map))
}

pub(crate) fn write_map(conn: &Connection, world: &str, snapshot: &MapSnapshot) -> Result<(), DbError> {
    for table in TABLES {
        conn.execute(&format!("DELETE FROM {table} WHERE world_id=?1"), [world])?;
    }
    {
        let mut s = conn.prepare_cached(
            "INSERT INTO map_labels(world_id, label_id, area, z, image, payload) VALUES(?1,?2,?3,?4,?5,?6)",
        )?;
        for label in &snapshot.labels {
            s.execute(params![world, label.id, label.area, label.z, label.image, json(label)?])?;
        }
    }
    {
        let mut s =
            conn.prepare_cached("INSERT INTO map_label_deletions(world_id, label_id, revision) VALUES(?1,?2,?3)")?;
        for d in &snapshot.deleted_labels {
            s.execute(params![world, d.id, d.revision])?;
        }
    }
    {
        // Pictures are written once (their hash is their key) and go when no label shows them.
        let mut s = conn.prepare_cached("INSERT OR IGNORE INTO map_images(world_id, hash, bytes) VALUES(?1,?2,?3)")?;
        for image in &snapshot.images {
            s.execute(params![world, image.hash, &image.data[..]])?;
        }
        conn.execute(
            "DELETE FROM map_images WHERE world_id=?1 AND hash NOT IN
                 (SELECT image FROM map_labels WHERE world_id=?1 AND image IS NOT NULL)",
            [world],
        )?;
    }
    {
        let mut s = conn.prepare_cached(
            "INSERT INTO map_rooms(world_id, room_id, area, x, y, z, payload) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        )?;
        for room in &snapshot.rooms {
            s.execute(params![world, room.id, room.area, room.x, room.y, room.z, json(room)?])?;
        }
    }
    {
        let mut s = conn.prepare_cached(
            "INSERT INTO map_links(world_id, from_id, direction, to_id, payload) VALUES(?1,?2,?3,?4,?5)",
        )?;
        for link in &snapshot.links {
            s.execute(params![world, link.from_id, link.direction, link.to_id, json(link)?])?;
        }
    }
    {
        let mut s = conn.prepare_cached("INSERT INTO map_areas(world_id, area, payload) VALUES(?1,?2,?3)")?;
        for area in &snapshot.area_settings {
            s.execute(params![world, area.area, json(area)?])?;
        }
    }
    {
        let mut s = conn.prepare_cached(
            "INSERT INTO map_room_aliases(world_id, source_id, target_id, revision) VALUES(?1,?2,?3,?4)",
        )?;
        for a in &snapshot.room_aliases {
            s.execute(params![world, a.source_id, a.target_id, a.revision])?;
        }
    }
    {
        let mut s =
            conn.prepare_cached("INSERT INTO map_room_deletions(world_id, room_id, revision) VALUES(?1,?2,?3)")?;
        for d in &snapshot.deleted_rooms {
            s.execute(params![world, d.id, d.revision])?;
        }
    }
    {
        let mut s = conn.prepare_cached(
            "INSERT INTO map_link_deletions(world_id, from_id, direction, revision) VALUES(?1,?2,?3,?4)",
        )?;
        for d in &snapshot.deleted_links {
            s.execute(params![world, d.from_id, d.direction, d.revision])?;
        }
    }
    let metadata = MapSnapshot {
        state: TrackingState::Unknown,
        source: snapshot.source,
        observation_count: snapshot.observation_count,
        ..MapSnapshot::default()
    };
    conn.execute(
        "INSERT INTO map_snapshots(world_id, payload) VALUES(?1, ?2)
         ON CONFLICT(world_id) DO UPDATE SET payload=excluded.payload",
        params![world, json(&metadata)?],
    )?;
    Ok(())
}

fn json<T: serde::Serialize>(value: &T) -> Result<String, DbError> {
    serde_json::to_string(value).map_err(invalid)
}

/// What a load returns.
pub type LoadResult = Result<Option<MapSnapshot>, String>;

enum Job {
    Load {
        world: MapWorld,
        reply: Sender<LoadResult>,
        wake: Box<dyn Fn() + Send>,
    },
    Save {
        world: MapWorld,
        snapshot: Box<MapSnapshot>,
        reply: Option<Sender<Result<(), String>>>,
    },
    Flush(Sender<()>),
}

/// Loads and saves maps on one thread of its own, in order (so saves of one world merge one
/// after another, never interleaved).
pub struct MapWorker {
    jobs: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl MapWorker {
    pub fn spawn(store: MapStore) -> std::io::Result<Self> {
        let (jobs, inbox) = channel::<Job>();
        let thread = std::thread::Builder::new().name("wandur-maps".into()).spawn(move || {
            for job in inbox {
                match job {
                    Job::Load { world, reply, wake } => {
                        let _ = reply.send(store.load(&world).map_err(|e| e.to_string()));
                        wake();
                    }
                    Job::Save { world, snapshot, reply } => {
                        let result = store.save(&world, &snapshot).map(|_| ()).map_err(|e| e.to_string());
                        if let Some(reply) = reply {
                            let _ = reply.send(result);
                        }
                    }
                    Job::Flush(done) => {
                        let _ = done.send(());
                    }
                }
            }
        })?;
        Ok(Self {
            jobs: Some(jobs),
            thread: Some(thread),
        })
    }

    /// Read a world's map; the answer arrives on the receiver, then `wake` is called.
    pub fn load(&self, world: MapWorld, wake: Box<dyn Fn() + Send>) -> Receiver<LoadResult> {
        let (reply, answer) = channel();
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(Job::Load { world, reply, wake });
        }
        answer
    }

    /// Merge a map into the world's saved one. The receiver gets the outcome.
    pub fn save(&self, world: MapWorld, snapshot: MapSnapshot) -> Receiver<Result<(), String>> {
        let (reply, answer) = channel();
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(Job::Save {
                world,
                snapshot: Box::new(snapshot),
                reply: Some(reply),
            });
        }
        answer
    }

    /// Wait until everything queued so far is done.
    pub fn flush(&self) {
        let (done, wait) = channel();
        if let Some(jobs) = &self.jobs
            && jobs.send(Job::Flush(done)).is_ok()
        {
            let _ = wait.recv();
        }
    }
}

impl Drop for MapWorker {
    fn drop(&mut self) {
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
