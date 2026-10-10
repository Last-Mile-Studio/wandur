//! World identity, as the C# `ClientDatabase.ResolveWorld`: a world has a random id; endpoints
//! (`host:port`, canonical) lead to it. Editing a saved world's address adds the new endpoint
//! for the same id and keeps the old one as an alias, so maps, scripts and usage keyed by the id
//! follow the world. Endpoint keys ignore TLS (one world, two ports at most) and compare host
//! names in lower-case ASCII (punycode) without a trailing dot.

use std::net::IpAddr;

use rusqlite::{Connection, OptionalExtension, params};

use super::DbError;

/// A new world id: 32 lower-case hex digits (the C# `Guid.ToString("N")` form).
pub fn new_world_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Whether `id` can be a world id.
pub fn valid_world_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
}

/// The canonical key of an endpoint: `host:port` with the host lower-case ASCII, without a
/// trailing dot, IPv6 in brackets. A key without a port (a named local world) is lower-cased.
/// A trailing `:True` or `:False` (the C# client's older `host:port:tls` keys) is dropped, as the
/// C# `CanonicalEndpointKey` does: TLS does not change the world.
pub fn canonical_endpoint_key(key: &str) -> Result<String, DbError> {
    let key = key.trim();
    let invalid = || DbError::InvalidEndpoint(key.chars().take(80).collect());
    if key.is_empty() || key.len() > 2048 || key.chars().any(char::is_control) {
        return Err(invalid());
    }
    let key = strip_tls_suffix(key);
    let Some((host, port)) = key.rsplit_once(':') else {
        return Ok(key.to_lowercase());
    };
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        // No port: a named local world (the offline demo), not a network address.
        return Ok(key.to_lowercase());
    }
    let port: u32 = port.parse().map_err(|_| invalid())?;
    if !(1..=65_535).contains(&port) {
        return Err(invalid());
    }
    Ok(format!("{}:{port}", canonical_host(host).ok_or_else(invalid)?))
}

/// `key` without a trailing `:True` or `:False` (any case, spaces allowed around the word, as
/// .NET's `bool.TryParse`).
pub fn strip_tls_suffix(key: &str) -> &str {
    match key.rsplit_once(':') {
        Some((rest, flag)) if flag.trim().eq_ignore_ascii_case("true") || flag.trim().eq_ignore_ascii_case("false") => {
            rest
        }
        _ => key,
    }
}

/// The key for a host and port.
pub fn endpoint_key(host: &str, port: u16) -> Result<String, DbError> {
    let host = host.trim();
    if host.contains(':') && !host.starts_with('[') {
        canonical_endpoint_key(&format!("[{host}]:{port}"))
    } else {
        canonical_endpoint_key(&format!("{host}:{port}"))
    }
}

fn canonical_host(host: &str) -> Option<String> {
    let mut host = host.trim().trim_end_matches('.');
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        host = inner;
    }
    if host.is_empty() {
        return None;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(match ip {
            IpAddr::V4(v4) => v4.to_string(),
            IpAddr::V6(v6) => format!("[{v6}]"),
        });
    }
    idna::domain_to_ascii(host)
        .ok()
        .filter(|h| !h.is_empty())
        .map(|h| h.to_lowercase())
}

/// The world an endpoint already belongs to. Reads only.
pub fn find_world(conn: &Connection, host: &str, port: u16) -> Result<Option<String>, DbError> {
    find_world_key(conn, &endpoint_key(host, port)?)
}

/// [`find_world`] by a key that may not be canonical yet.
pub fn find_world_key(conn: &Connection, key: &str) -> Result<Option<String>, DbError> {
    let key = canonical_endpoint_key(key)?;
    Ok(conn
        .query_row("SELECT world_id FROM endpoints WHERE endpoint_key=?1", [&key], |r| {
            r.get(0)
        })
        .optional()?)
}

/// The world for an endpoint, creating it (with `preferred` as its id, else a new one) when the
/// endpoint is new. An endpoint that already belongs to a world other than `preferred` is a
/// [`DbError::WorldConflict`]. Run inside a write transaction.
pub fn resolve_world(conn: &Connection, host: &str, port: u16, preferred: Option<&str>) -> Result<String, DbError> {
    let key = endpoint_key(host, port)?;
    if let Some(existing) = find_world_key(conn, &key)? {
        return match preferred {
            Some(p) if p != existing => Err(DbError::WorldConflict {
                endpoint: key,
                world: existing,
            }),
            _ => Ok(existing),
        };
    }
    let id = preferred.map_or_else(new_world_id, str::to_string);
    if !valid_world_id(&id) {
        return Err(DbError::InvalidEndpoint(id));
    }
    conn.execute("INSERT OR IGNORE INTO worlds(id) VALUES(?1)", [&id])?;
    conn.execute(
        "INSERT INTO endpoints(endpoint_key, world_id) VALUES(?1, ?2)",
        params![key, id],
    )?;
    Ok(id)
}

/// Every endpoint key leading to a world, sorted.
pub fn endpoints_of(conn: &Connection, world: &str) -> Result<Vec<String>, DbError> {
    let mut s = conn.prepare("SELECT endpoint_key FROM endpoints WHERE world_id=?1 ORDER BY endpoint_key")?;
    let keys = s
        .query_map([world], |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(keys)
}

/// Count one connection to a world at `at` (seconds since 1970).
pub fn record_connection(conn: &Connection, world: &str, at: u64) -> Result<(), DbError> {
    conn.execute("INSERT OR IGNORE INTO worlds(id) VALUES(?1)", [world])?;
    conn.execute(
        "INSERT INTO world_usage(world_id, connections, last_connected_at) VALUES(?1, 1, ?2)
         ON CONFLICT(world_id) DO UPDATE SET connections=connections+1, last_connected_at=excluded.last_connected_at",
        params![world, at as i64],
    )?;
    conn.execute(
        "INSERT INTO world_connections(world_id, connected_at) VALUES(?1, ?2)",
        params![world, at as i64],
    )?;
    Ok(())
}

/// A world's connection count and last connection time.
pub fn usage(conn: &Connection, world: &str) -> Result<Option<(u32, u64)>, DbError> {
    Ok(conn
        .query_row(
            "SELECT connections, last_connected_at FROM world_usage WHERE world_id=?1",
            [world],
            |r| Ok((r.get::<_, u32>(0)?, r.get::<_, i64>(1)? as u64)),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::super::Database;
    use super::*;

    #[test]
    fn endpoint_keys_are_canonical() {
        let k = |s: &str| canonical_endpoint_key(s).unwrap();
        assert_eq!(k(" MUD.Example.org.:4000 "), "mud.example.org:4000");
        assert_eq!(k("127.0.0.1:23"), "127.0.0.1:23");
        assert_eq!(k("[::1]:4000"), "[::1]:4000");
        assert_eq!(endpoint_key("::1", 4000).unwrap(), "[::1]:4000");
        assert_eq!(k("bücher.example:23"), "xn--bcher-kva.example:23");
        assert_eq!(k("Demo World"), "demo world");
        assert!(canonical_endpoint_key("").is_err());
        assert!(canonical_endpoint_key("host:0").is_err());
        assert!(canonical_endpoint_key("bad\u{1}host:23").is_err());
        assert!(canonical_endpoint_key(":23").is_err());
        // The C# client's older keys carried TLS; it does not change the world.
        assert_eq!(k("MUD.example.org:4000:True"), "mud.example.org:4000");
        assert_eq!(k("mud.example.org:4000:false"), "mud.example.org:4000");
        assert_eq!(k("2001:db8::5:4000:True"), "[2001:db8::5]:4000");
        assert_eq!(k("Demo World:true"), "demo world");
    }

    #[test]
    fn editing_the_address_keeps_the_world() {
        let dir = super::super::tests::temp_dir("worlds-edit");
        let (db, _) = Database::open(&dir).unwrap();
        let id = db.write(|tx| resolve_world(tx, "mud.example.org", 4000, None)).unwrap();
        assert_eq!(id.len(), 32);
        // The same endpoint, spelled differently, is the same world.
        let again = db
            .write(|tx| resolve_world(tx, "MUD.example.org.", 4000, None))
            .unwrap();
        assert_eq!(again, id);
        // The person edits host and port: the saved world keeps its id, the new endpoint joins it.
        let edited = db
            .write(|tx| resolve_world(tx, "new.example.org", 5000, Some(&id)))
            .unwrap();
        assert_eq!(edited, id);
        let conn = db.connect().unwrap();
        assert_eq!(
            endpoints_of(&conn, &id).unwrap(),
            ["mud.example.org:4000", "new.example.org:5000"]
        );
        assert_eq!(find_world(&conn, "new.example.org", 5000).unwrap(), Some(id.clone()));
        // Another world cannot take an endpoint that belongs to this one.
        let err = db
            .write(|tx| resolve_world(tx, "new.example.org", 5000, Some("other")))
            .unwrap_err();
        assert!(matches!(err, DbError::WorldConflict { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn usage_counts_connections() {
        let dir = super::super::tests::temp_dir("worlds-usage");
        let (db, _) = Database::open(&dir).unwrap();
        let id = new_world_id();
        db.write(|tx| record_connection(tx, &id, 100)).unwrap();
        db.write(|tx| record_connection(tx, &id, 250)).unwrap();
        assert_eq!(db.read(|c| usage(c, &id)).unwrap(), Some((2, 250)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
