//! The Mudlet Mapping Protocol's XML map (MMP): the map file a game publishes for clients to
//! download (GMCP `Client.Map`), also accepted by File > Import map as a `.xml` file. It is read
//! into the same Mudlet map model as the JSON export and converted by the same code
//! ([`super::convert`]), so ids, colours, special exits and the summary work the same way.
//!
//! Clean room: Mudlet is GPL-3.0 and none of its source was read. The format is taken from the
//! public Mudlet wiki only: "Standards:MMP" (the XML layout: `map`, `areas/area`, `rooms/room`
//! with `coord` and `exit`, `environments/environment`), "Manual:GMCP Extensions" (Client.Map)
//! and "Manual:Mapper Functions" (`loadMap` reads such `.xml` maps). Assumptions where the
//! documentation is silent are listed in `docs/mudlet-map-import.md`.
//!
//! The file is untrusted: no DTD is processed (a file that declares one is refused), no entity
//! other than the five built-in ones and character references is expanded, nesting is bounded,
//! and areas, environments, rooms and exits are counted against limits while reading.

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde_json::{Map, Value, json};

use super::{ImportError, MudletArea, MudletFile, MudletRoom, Prepared, Progress, SourceKind, Stage, convert};
use crate::l10n::{S, t, tf};
use crate::map::format::{self, MAX_LINKS, MAX_ROOMS};

/// Most areas a file may name.
pub const MAX_AREAS: usize = 10_000;
/// Most environments a file may define.
pub const MAX_ENVIRONMENTS: usize = 10_000;
/// Most exits one room may have.
pub const MAX_ROOM_EXITS: usize = 256;
/// Deepest element nesting accepted (the format needs four).
const MAX_DEPTH: usize = 16;

/// Whether the bytes look like an XML document (after a byte order mark and white space).
pub fn looks_like_xml(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    bytes.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'<')
}

fn invalid(detail: &str) -> ImportError {
    ImportError::Invalid(tf(S::MapImportXmlInvalid, &[&detail]))
}

fn too_many(what: S, limit: usize) -> ImportError {
    ImportError::Invalid(tf(S::MapImportXmlTooMany, &[&t(what), &limit]))
}

/// The attributes of an element, by local name. Values are unescaped; an entity other than the
/// built-in ones fails.
fn attributes(element: &BytesStart<'_>) -> Result<Vec<(String, String)>, ImportError> {
    let mut out = Vec::new();
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|e| invalid(&e.to_string()))?;
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|e| invalid(&e.to_string()))?;
        out.push((local(attribute.key.as_ref()), value.into_owned()));
    }
    Ok(out)
}

fn local(name: &[u8]) -> String {
    let name = String::from_utf8_lossy(name);
    match name.rsplit_once(':') {
        Some((_, local)) => local.to_ascii_lowercase(),
        None => name.to_ascii_lowercase(),
    }
}

fn get<'a>(attributes: &'a [(String, String)], names: &[&str]) -> Option<&'a str> {
    names
        .iter()
        .find_map(|n| attributes.iter().find(|(k, _)| k == n).map(|(_, v)| v.trim()))
        .filter(|v| !v.is_empty())
}

fn text_value(value: Option<&str>) -> Option<Value> {
    value.map(|v| Value::String(v.to_string()))
}

/// The colour of an MMP environment's `color`: an ANSI colour number (0 to 15), or a number of
/// the 256 colour palette, as `#RRGGBB`.
fn ansi_color(number: u32) -> Option<String> {
    let rgb = match number {
        0 => [0, 0, 0],
        1..=15 => {
            let c = super::DEFAULT_ENV[number as usize - 1];
            [c[0], c[1], c[2]]
        }
        16..=231 => {
            let n = number - 16;
            let level = |v: u32| if v == 0 { 0 } else { (55 + v * 40) as u8 };
            [level(n / 36), level((n / 6) % 6), level(n % 6)]
        }
        232..=255 => {
            let v = (8 + (number - 232) * 10) as u8;
            [v, v, v]
        }
        _ => return None,
    };
    Some(format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]))
}

/// One room while reading: its attributes, then its coordinates and exits.
struct OpenRoom {
    /// The element depth of the room element.
    depth: usize,
    area: Option<String>,
    room: MudletRoom,
    exits: Vec<Value>,
}

/// Read an MMP XML map into a map with its summary. Runs on a worker thread: `progress` is
/// told how far it got.
pub fn read(bytes: &[u8], progress: &dyn Fn(Progress)) -> Result<Prepared, ImportError> {
    if bytes.len() > format::MAX_BYTES {
        return Err(ImportError::Invalid(t(S::MapImportFileTooLarge).into()));
    }
    progress(Progress {
        stage: Stage::Reading,
        done: 0,
        total: 0,
    });
    let file = parse(bytes)?;
    let mut prepared = convert(&file, progress)?;
    prepared.summary.kind = SourceKind::MudletXml;
    Ok(prepared)
}

/// The document as Mudlet's map model: areas in the order named (rooms of an area the file
/// does not name get one of their own), environment colours as custom colours.
fn parse(bytes: &[u8]) -> Result<MudletFile, ImportError> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    reader.config_mut().check_end_names = true;
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut saw_root = false;
    let mut areas: Vec<MudletArea> = Vec::new();
    let mut area_index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut colors = Map::new();
    let mut environments = 0usize;
    let mut rooms: Vec<(Option<String>, MudletRoom)> = Vec::new();
    let mut room_count = 0usize;
    let mut exit_count = 0usize;
    let mut open: Option<OpenRoom> = None;
    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|e| invalid(&e.to_string()))?;
        let (element, empty) = match event {
            Event::Start(e) => (Some(e.into_owned()), false),
            Event::Empty(e) => (Some(e.into_owned()), true),
            Event::End(_) => {
                // Only the room element's own end closes the room.
                if let Some(room) = open.take_if(|r| r.depth == depth) {
                    finish_room(room, &mut rooms);
                }
                depth = depth.saturating_sub(1);
                buffer.clear();
                continue;
            }
            Event::DocType(_) => return Err(ImportError::Invalid(t(S::MapImportXmlDtd).into())),
            Event::GeneralRef(reference) => {
                // Text between elements carries nothing; a reference to anything but the
                // built-in entities would come from a DTD, which is never processed.
                let name = reference.decode().map_err(|e| invalid(&e.to_string()))?;
                if !reference.is_char_ref() && !matches!(name.as_ref(), "lt" | "gt" | "amp" | "apos" | "quot") {
                    return Err(ImportError::Invalid(t(S::MapImportXmlDtd).into()));
                }
                buffer.clear();
                continue;
            }
            Event::Eof => break,
            // The declaration, text, comments and processing instructions carry nothing.
            _ => {
                buffer.clear();
                continue;
            }
        };
        let Some(element) = element else { continue };
        let name = local(element.name().as_ref());
        if !saw_root {
            if name != "map" {
                return Err(ImportError::Unknown);
            }
            saw_root = true;
            if !empty {
                depth += 1;
            }
            buffer.clear();
            continue;
        }
        if depth == 0 {
            return Err(invalid("content after the map element"));
        }
        if !empty {
            depth += 1;
            if depth > MAX_DEPTH {
                return Err(invalid("elements nested too deeply"));
            }
        }
        match name.as_str() {
            "area" => {
                let a = attributes(&element)?;
                if let Some(id) = get(&a, &["id"]) {
                    if area_index.contains_key(id) {
                        // A second area with the same id: the first one's name is kept.
                    } else {
                        if areas.len() >= MAX_AREAS {
                            return Err(too_many(S::MapImportXmlAreas, MAX_AREAS));
                        }
                        area_index.insert(id.to_string(), areas.len());
                        areas.push(MudletArea {
                            id: text_value(Some(id)),
                            name: text_value(get(&a, &["name"])),
                            ..MudletArea::default()
                        });
                    }
                }
            }
            "environment" => {
                let a = attributes(&element)?;
                environments += 1;
                if environments > MAX_ENVIRONMENTS {
                    return Err(too_many(S::MapImportXmlEnvironments, MAX_ENVIRONMENTS));
                }
                let color = get(&a, &["htmlcolor", "htmlcolour"])
                    .filter(|c| c.starts_with('#'))
                    .map(str::to_string)
                    .or_else(|| {
                        get(&a, &["color", "colour"])
                            .and_then(|c| c.parse::<u32>().ok())
                            .and_then(ansi_color)
                    });
                if let (Some(id), Some(color)) = (get(&a, &["id"]), color) {
                    colors.insert(id.to_string(), Value::String(color));
                }
            }
            "room" => {
                if let Some(room) = open.take() {
                    // A room inside a room: the outer one ends here.
                    finish_room(room, &mut rooms);
                }
                let a = attributes(&element)?;
                room_count += 1;
                let room = MudletRoom {
                    // The room's id is the game's own room number, which GMCP Room.Info also
                    // gives: it doubles as the room hash so the rooms line up with tracking.
                    id: text_value(get(&a, &["id"])),
                    hash: text_value(get(&a, &["id"])),
                    name: text_value(get(&a, &["title", "name"])),
                    environment: text_value(get(&a, &["environment", "env"])),
                    ..MudletRoom::default()
                };
                let open_room = OpenRoom {
                    depth,
                    area: get(&a, &["area"]).map(str::to_string),
                    room,
                    exits: Vec::new(),
                };
                if empty {
                    finish_room(open_room, &mut rooms);
                } else {
                    open = Some(open_room);
                }
                if room_count > MAX_ROOMS {
                    // Keep counting (rooms and exits) to say how big the file is; keep nothing.
                    rooms.clear();
                }
            }
            "coord" | "coords" | "coordinates" => {
                let a = attributes(&element)?;
                if let Some(room) = &mut open {
                    let number = |k: &str| get(&a, &[k]).and_then(|v| v.parse::<f64>().ok());
                    room.room.x = number("x");
                    room.room.y = number("y");
                    room.room.z = number("z");
                }
            }
            "exit" => {
                let a = attributes(&element)?;
                exit_count += 1;
                if let Some(room) = &mut open
                    && room_count <= MAX_ROOMS
                {
                    if room.exits.len() >= MAX_ROOM_EXITS {
                        return Err(too_many(S::MapImportXmlRoomExits, MAX_ROOM_EXITS));
                    }
                    let mut exit = Map::new();
                    if let Some(d) = get(&a, &["direction", "dir", "name"]) {
                        exit.insert("name".into(), json!(d));
                    }
                    if let Some(to) = get(&a, &["target", "to"]) {
                        exit.insert("exitId".into(), json!(to));
                    }
                    if let Some(door) = get(&a, &["door"]) {
                        // A number (Mudlet's door numbers) or a word.
                        let value = door.parse::<i64>().map_or_else(|_| json!(door), |n| json!(n));
                        exit.insert("door".into(), value);
                    }
                    if let Some(weight) = get(&a, &["weight"]) {
                        exit.insert("weight".into(), json!(weight));
                    }
                    if let Some(locked) = get(&a, &["locked", "lock"]) {
                        exit.insert("locked".into(), json!(locked));
                    }
                    room.exits.push(Value::Object(exit));
                }
            }
            // `areas`, `rooms`, `environments` and anything the documentation does not name.
            _ => {}
        }
        buffer.clear();
    }
    if !saw_root {
        return Err(ImportError::Unknown);
    }
    if depth != 0 || open.is_some() {
        return Err(invalid("unclosed element"));
    }
    if room_count > MAX_ROOMS || exit_count > MAX_LINKS {
        return Err(ImportError::TooBig {
            rooms: room_count,
            exits: exit_count,
        });
    }
    for (area, room) in rooms {
        let key = area.unwrap_or_default();
        let index = match area_index.get(&key) {
            Some(i) => *i,
            None => {
                if areas.len() >= MAX_AREAS {
                    return Err(too_many(S::MapImportXmlAreas, MAX_AREAS));
                }
                area_index.insert(key.clone(), areas.len());
                areas.push(MudletArea {
                    id: text_value(Some(&key)),
                    ..MudletArea::default()
                });
                areas.len() - 1
            }
        };
        areas[index].rooms.push(room);
    }
    Ok(MudletFile {
        areas,
        custom_env_colors: (!colors.is_empty()).then_some(Value::Object(colors)),
        user_data: None,
    })
}

fn finish_room(mut open: OpenRoom, rooms: &mut Vec<(Option<String>, MudletRoom)>) {
    if !open.exits.is_empty() {
        open.room.exits = Some(Value::Array(std::mem::take(&mut open.exits)));
    }
    rooms.push((open.area, open.room));
}

#[cfg(test)]
mod tests;
