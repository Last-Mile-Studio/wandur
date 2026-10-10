//! File > Import map: a map file read into a map to bring in, with a summary to show first.
//!
//! Three kinds of file are read: this client's own map file ([`super::format`], which replaces
//! the map, as the C# client's import does), Mudlet's JSON map export (`saveJsonMap`, or the
//! mapper's export) and the Mudlet Mapping Protocol's XML map ([`xml`], what games publish
//! through GMCP `Client.Map`), both merged into the world's map. Mudlet's binary `.dat` map is
//! recognised and refused with a note on how to make the JSON export.
//!
//! Clean room: Mudlet is GPL-3.0 and none of its source was read. The reader follows Mudlet's
//! public documentation of the export and of its mapper API, and is tolerant: unknown fields are
//! ignored, every field is optional, and several spellings are accepted where the documentation
//! leaves the exact shape open. Every assumption is listed in `docs/mudlet-map-import.md`.
//!
//! What a Mudlet map becomes:
//!
//! - Rooms keep a stable id, so importing the same file again changes nothing: `s:<hash>` when
//!   the room has Mudlet's room hash (the server's room id a mapper script recorded; the room
//!   then lines up with GMCP tracking), else `mudlet:<room id>`.
//! - Coordinates are Mudlet's; Mudlet's mapper API puts north at +Y as this map does. The exits
//!   are checked: when most north exits lead to a lower Y, the file is upside down and Y is
//!   flipped for rooms and labels.
//! - Environments become the room's colour (Mudlet's custom environment colours, else the 16
//!   default ANSI colours); exits (compass, up, down, in, out) become exits, special exits
//!   become exits with their command; doors, exit locks and weights, room locks and weights,
//!   symbols, the area's grid mode come along; room user data becomes the room's notes (a
//!   `description` entry its description); stubs become known exits.
//! - Labels: text labels keep their text, colours and size; picture labels keep their picture
//!   (scaled down when too large, [`super::images::prepare`]).
//! - Not supported (counted in the summary, with the reason): special exits that run Lua
//!   (`script:`), exits to rooms not in the file, custom exit lines, symbol colours, labels that
//!   keep their size when zooming, area and map user data.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::Value;

use super::format::{self, MAX_LINKS, MAX_ROOMS};
use super::images::{self, ImageError, MAX_IMAGES_BYTES, MAX_LABELS};
use super::model::*;
use crate::l10n::{S, t, tf};

/// What kind of file was picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// This client's (or the C# client's) map file: it replaces the map.
    Wandur,
    /// Mudlet's JSON export: merged into the map.
    Mudlet,
    /// A Mudlet Mapping Protocol XML map (a game's official map): merged into the map.
    MudletXml,
}

/// Why a file could not be imported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    /// Mudlet's binary map (`.dat`): Wandur reads the JSON export.
    MudletBinary,
    /// Not a map file this client reads.
    Unknown,
    /// More rooms or exits than a map holds.
    TooBig { rooms: usize, exits: usize },
    /// A map file that does not pass its checks (the reason).
    Invalid(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::MudletBinary => f.write_str(t(S::MapImportMudletDat)),
            ImportError::Unknown => f.write_str(t(S::MapImportUnknownFile)),
            ImportError::TooBig { rooms, exits } => {
                f.write_str(&tf(S::MapImportTooBig, &[rooms, &MAX_ROOMS, exits, &MAX_LINKS]))
            }
            ImportError::Invalid(reason) => f.write_str(&tf(S::MapImportFailed, &[reason])),
        }
    }
}

impl std::error::Error for ImportError {}

/// Something in the file that was left out or changed, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Skip {
    /// A special exit whose command runs Lua (`script:`).
    ScriptExit,
    /// An exit to a room the file does not have.
    MissingRoom,
    /// Two exits of a room by the same direction or command (the first is kept).
    DuplicateExit,
    /// A room without a usable id or position.
    BadRoom,
    /// Custom exit lines (drawn straight).
    CustomLine,
    /// Symbol colours (symbols take the map's colour).
    SymbolColor,
    /// Labels that keep their size when zooming (they scale with the map).
    FixedSizeLabel,
    /// Labels with neither text nor a readable picture, or without a position or size.
    EmptyLabel,
    /// Pictures that could not be read.
    UnreadablePicture,
    /// Pictures past the map's 20 MiB of pictures.
    PictureLimit,
    /// Labels past the map's label limit.
    LabelLimit,
    /// Area user data.
    AreaUserData,
    /// The map's own user data.
    MapUserData,
}

impl Skip {
    pub const ALL: [Skip; 13] = [
        Skip::ScriptExit,
        Skip::MissingRoom,
        Skip::DuplicateExit,
        Skip::BadRoom,
        Skip::CustomLine,
        Skip::SymbolColor,
        Skip::FixedSizeLabel,
        Skip::EmptyLabel,
        Skip::UnreadablePicture,
        Skip::PictureLimit,
        Skip::LabelLimit,
        Skip::AreaUserData,
        Skip::MapUserData,
    ];

    /// The summary's line for `n` of these.
    pub fn text(self, n: usize) -> String {
        tf(
            match self {
                Skip::ScriptExit => S::MapImportSkipScriptExit,
                Skip::MissingRoom => S::MapImportSkipMissingRoom,
                Skip::DuplicateExit => S::MapImportSkipDuplicateExit,
                Skip::BadRoom => S::MapImportSkipBadRoom,
                Skip::CustomLine => S::MapImportSkipCustomLine,
                Skip::SymbolColor => S::MapImportSkipSymbolColor,
                Skip::FixedSizeLabel => S::MapImportSkipFixedLabel,
                Skip::EmptyLabel => S::MapImportSkipEmptyLabel,
                Skip::UnreadablePicture => S::MapImportSkipPicture,
                Skip::PictureLimit => S::MapImportSkipPictureLimit,
                Skip::LabelLimit => S::MapImportSkipLabelLimit,
                Skip::AreaUserData => S::MapImportSkipAreaData,
                Skip::MapUserData => S::MapImportSkipMapData,
            },
            &[&n],
        )
    }
}

/// What a file holds, counted for the summary shown before anything changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportSummary {
    pub kind: SourceKind,
    pub areas: usize,
    pub rooms: usize,
    pub exits: usize,
    /// Of the exits: special exits (a command of their own).
    pub special_exits: usize,
    /// Of the exits: with a door.
    pub doors: usize,
    /// Of the exits: locked (kept out of routes).
    pub locked_exits: usize,
    /// Rooms whose user data became notes.
    pub notes: usize,
    pub symbols: usize,
    pub text_labels: usize,
    pub picture_labels: usize,
    /// The file was upside down (north at -Y) and Y was flipped.
    pub flipped: bool,
    /// What was left out, and why (only kinds that happened, in [`Skip::ALL`] order).
    pub skipped: Vec<(Skip, usize)>,
}

impl ImportSummary {
    fn new(kind: SourceKind) -> Self {
        Self {
            kind,
            areas: 0,
            rooms: 0,
            exits: 0,
            special_exits: 0,
            doors: 0,
            locked_exits: 0,
            notes: 0,
            symbols: 0,
            text_labels: 0,
            picture_labels: 0,
            flipped: false,
            skipped: Vec::new(),
        }
    }

    /// How many of a kind were left out.
    pub fn skipped(&self, skip: Skip) -> usize {
        self.skipped.iter().find(|(s, _)| *s == skip).map_or(0, |(_, n)| *n)
    }

    /// The counts, one line each, in the current language (kinds with none left out, except
    /// rooms and exits).
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            tf(S::MapImportCountAreas, &[&self.areas]),
            tf(S::MapImportCountRooms, &[&self.rooms]),
            tf(S::MapImportCountExits, &[&self.exits]),
        ];
        for (n, key) in [
            (self.special_exits, S::MapImportCountSpecialExits),
            (self.doors, S::MapImportCountDoors),
            (self.locked_exits, S::MapImportCountLockedExits),
            (self.notes, S::MapImportCountNotes),
            (self.symbols, S::MapImportCountSymbols),
            (self.text_labels, S::MapImportCountTextLabels),
            (self.picture_labels, S::MapImportCountPictureLabels),
        ] {
            if n > 0 {
                lines.push(tf(key, &[&n]));
            }
        }
        lines
    }

    /// The lines for what was left out.
    pub fn skipped_lines(&self) -> Vec<String> {
        self.skipped.iter().map(|(s, n)| s.text(*n)).collect()
    }
}

/// A file read and turned into a map, waiting for the person's go-ahead.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub map: MapSnapshot,
    pub summary: ImportSummary,
}

impl Prepared {
    /// The whole map is replaced (this client's file), not merged (Mudlet's).
    pub fn replaces(&self) -> bool {
        self.summary.kind == SourceKind::Wandur
    }
}

/// Progress of a read: what is being done, and how far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub stage: Stage,
    pub done: usize,
    pub total: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Reading,
    Rooms,
    Labels,
}

/// Whether the bytes look like Mudlet's binary map: a `.dat` file, or a file that starts with a
/// small big-endian number (the binary format's version) instead of JSON.
pub fn is_mudlet_binary(extension: Option<&str>, bytes: &[u8]) -> bool {
    if extension.is_some_and(|e| e.eq_ignore_ascii_case("dat")) {
        return true;
    }
    let first = bytes.iter().find(|b| !b.is_ascii_whitespace());
    if first.is_some_and(|b| *b == b'{' || *b == b'[') {
        return false;
    }
    bytes.len() >= 4 && {
        let version = i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        (1..=256).contains(&version)
    }
}

#[derive(Deserialize)]
struct Probe {
    #[serde(default)]
    areas: Option<IgnoredAny>,
    #[serde(default, rename = "Version")]
    version: Option<IgnoredAny>,
    #[serde(default, rename = "Rooms")]
    rooms: Option<IgnoredAny>,
}

/// Read a picked file (`extension` is its file name's) into a map with its summary. Runs on a
/// worker thread: `progress` is told how far it got.
pub fn read(extension: Option<&str>, bytes: &[u8], progress: &dyn Fn(Progress)) -> Result<Prepared, ImportError> {
    if extension.is_some_and(|e| e.eq_ignore_ascii_case("xml")) || xml::looks_like_xml(bytes) {
        return xml::read(bytes, progress);
    }
    if is_mudlet_binary(extension, bytes) {
        return Err(ImportError::MudletBinary);
    }
    if bytes.len() > format::MAX_BYTES {
        return Err(ImportError::Invalid(t(S::MapImportFileTooLarge).into()));
    }
    let text = std::str::from_utf8(bytes).map_err(|_| ImportError::Unknown)?;
    let text = text.trim_start_matches('\u{feff}');
    progress(Progress {
        stage: Stage::Reading,
        done: 0,
        total: 0,
    });
    let probe: Probe = serde_json::from_str(text).map_err(|_| ImportError::Unknown)?;
    if probe.version.is_some() || probe.rooms.is_some() {
        let map = format::deserialize(text).map_err(|e| ImportError::Invalid(e.0))?;
        let mut summary = ImportSummary::new(SourceKind::Wandur);
        summary.areas = map.rooms.iter().map(MapRoom::area_key).collect::<HashSet<_>>().len();
        summary.rooms = map.rooms.len();
        summary.exits = map.links.len();
        summary.doors = map.links.iter().filter(|l| l.door_state != DoorState::None).count();
        summary.locked_exits = map.links.iter().filter(|l| l.is_locked).count();
        summary.special_exits = map
            .links
            .iter()
            .filter(|l| normalize_direction(&l.direction).is_none())
            .count();
        summary.notes = map.rooms.iter().filter(|r| !r.notes.is_empty()).count();
        summary.symbols = map
            .rooms
            .iter()
            .filter(|r| r.symbol.as_deref().is_some_and(|s| !s.is_empty()))
            .count();
        summary.picture_labels = map.labels.iter().filter(|l| l.image.is_some()).count();
        summary.text_labels = map.labels.len() - summary.picture_labels;
        return Ok(Prepared { map, summary });
    }
    if probe.areas.is_none() {
        return Err(ImportError::Unknown);
    }
    let file: MudletFile = serde_json::from_str(text).map_err(|e| ImportError::Invalid(e.to_string()))?;
    convert(&file, progress)
}

// ---- Mudlet's JSON, as documented, read loosely ---------------------------------------------

/// A list that may also be `null` or missing.
fn list<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct MudletFile {
    #[serde(deserialize_with = "list")]
    areas: Vec<MudletArea>,
    #[serde(alias = "envColors", alias = "environmentColors")]
    custom_env_colors: Option<Value>,
    user_data: Option<Value>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct MudletArea {
    id: Option<Value>,
    name: Option<Value>,
    #[serde(deserialize_with = "list")]
    rooms: Vec<MudletRoom>,
    #[serde(deserialize_with = "list")]
    labels: Vec<Value>,
    grid_mode: Option<Value>,
    user_data: Option<Value>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct MudletRoom {
    id: Option<Value>,
    name: Option<Value>,
    #[serde(alias = "coords", alias = "position")]
    coordinates: Option<Value>,
    x: Option<f64>,
    y: Option<f64>,
    z: Option<f64>,
    #[serde(alias = "env")]
    environment: Option<Value>,
    exits: Option<Value>,
    special_exits: Option<Value>,
    #[serde(alias = "stubs")]
    stub_exits: Option<Value>,
    doors: Option<Value>,
    exit_weights: Option<Value>,
    exit_locks: Option<Value>,
    user_data: Option<Value>,
    #[serde(alias = "roomHash")]
    hash: Option<Value>,
    #[serde(alias = "char")]
    symbol: Option<Value>,
    weight: Option<Value>,
    #[serde(alias = "isLocked")]
    locked: Option<Value>,
    custom_lines: Option<Value>,
}

/// An id given as a number or a string.
fn id_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Number(n) => n.as_i64().map(|n| n.to_string()).or_else(|| Some(n.to_string())),
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|v: &f64| v.is_finite())
}

/// `[x, y, z]`, `[x, y]` or `{x, y, z}`.
fn triple(value: Option<&Value>) -> Option<(f64, f64, f64)> {
    match value? {
        Value::Array(items) if items.len() >= 2 => Some((
            number(items.first())?,
            number(items.get(1))?,
            number(items.get(2)).unwrap_or(0.0),
        )),
        Value::Object(o) => Some((
            number(o.get("x"))?,
            number(o.get("y"))?,
            number(o.get("z")).unwrap_or(0.0),
        )),
        _ => None,
    }
}

/// `[w, h]` or `{width, height}`.
fn pair(value: Option<&Value>) -> Option<(f64, f64)> {
    match value? {
        Value::Array(items) if items.len() >= 2 => Some((number(items.first())?, number(items.get(1))?)),
        Value::Object(o) => Some((
            number(o.get("width").or_else(|| o.get("w")))?,
            number(o.get("height").or_else(|| o.get("h")))?,
        )),
        _ => None,
    }
}

/// A colour as red, green, blue and alpha: `[r, g, b]`, `[r, g, b, a]`, an object with
/// `color24RGB` or `color32RGBA` (or `r`, `g`, `b`, `a`), or `#RRGGBB`, `#AARRGGBB`.
fn rgba(value: Option<&Value>) -> Option<[u8; 4]> {
    let byte = |v: Option<&Value>| number(v).map(|n| n.clamp(0.0, 255.0) as u8);
    match value? {
        Value::Array(items) if items.len() >= 3 => Some([
            byte(items.first())?,
            byte(items.get(1))?,
            byte(items.get(2))?,
            byte(items.get(3)).unwrap_or(255),
        ]),
        Value::Object(o) => {
            if let Some(v) = o
                .get("color32RGBA")
                .or_else(|| o.get("color24RGB"))
                .or_else(|| o.get("color"))
                .or_else(|| o.get("rgb"))
            {
                rgba(Some(v))
            } else {
                Some([
                    byte(o.get("r").or_else(|| o.get("red")))?,
                    byte(o.get("g").or_else(|| o.get("green")))?,
                    byte(o.get("b").or_else(|| o.get("blue")))?,
                    byte(o.get("a").or_else(|| o.get("alpha"))).unwrap_or(255),
                ])
            }
        }
        Value::String(s) => {
            let hex = s.trim().strip_prefix('#')?;
            let v = u32::from_str_radix(hex, 16).ok()?;
            match hex.len() {
                6 => Some([(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]),
                8 => Some([(v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8]),
                _ => None,
            }
        }
        _ => None,
    }
}

fn hex(c: [u8; 4]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// Mudlet's default environment colours 1 to 16: the ANSI colours, normal then bright.
const DEFAULT_ENV: [[u8; 3]; 16] = [
    [128, 0, 0],
    [0, 128, 0],
    [128, 128, 0],
    [0, 0, 128],
    [128, 0, 128],
    [0, 128, 128],
    [192, 192, 192],
    [64, 64, 64],
    [255, 0, 0],
    [0, 255, 0],
    [255, 255, 0],
    [0, 0, 255],
    [255, 0, 255],
    [0, 255, 255],
    [255, 255, 255],
    [128, 128, 128],
];

/// The environment colours: custom ones over the defaults. Custom colours come as a list of
/// `{id, color...}` or an object of id to colour.
fn environment_colors(custom: Option<&Value>) -> HashMap<i64, String> {
    let mut colors: HashMap<i64, String> = DEFAULT_ENV
        .iter()
        .enumerate()
        .map(|(i, c)| (i as i64 + 1, hex([c[0], c[1], c[2], 255])))
        .collect();
    match custom {
        Some(Value::Array(items)) => {
            for item in items {
                let Some(o) = item.as_object() else { continue };
                let id = number(o.get("id").or_else(|| o.get("envId")).or_else(|| o.get("environment")));
                if let (Some(id), Some(c)) = (id, rgba(Some(item))) {
                    colors.insert(id as i64, hex(c));
                }
            }
        }
        Some(Value::Object(o)) => {
            for (id, value) in o {
                if let (Ok(id), Some(c)) = (id.trim().parse::<i64>(), rgba(Some(value))) {
                    colors.insert(id, hex(c));
                }
            }
        }
        _ => {}
    }
    colors
}

/// One exit as the file gives it.
struct RawExit {
    name: String,
    to: Option<String>,
    door: Option<DoorState>,
    weight: Option<f64>,
    locked: Option<bool>,
    custom_line: bool,
}

fn door(value: Option<&Value>) -> Option<DoorState> {
    match value? {
        Value::String(s) => match s.trim().to_lowercase().as_str() {
            "open" => Some(DoorState::Open),
            "closed" => Some(DoorState::Closed),
            "locked" => Some(DoorState::Locked),
            "none" | "" => Some(DoorState::None),
            _ => None,
        },
        v => number(Some(v)).and_then(|n| DoorState::from_number(n as i64)),
    }
}

fn truthy(value: Option<&Value>) -> Option<bool> {
    match value? {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|n| n != 0.0),
        Value::String(s) => Some(matches!(s.trim().to_lowercase().as_str(), "true" | "yes" | "1")),
        _ => None,
    }
}

/// Exits given as a list of objects (`{name, exitId, door, weight, locked}`), or an object of
/// name to destination (a number, or such an object).
fn raw_exits(value: Option<&Value>) -> Vec<RawExit> {
    let one = |name: Option<String>, item: &Value| -> Option<RawExit> {
        match item {
            Value::Object(o) => {
                let name = name.or_else(|| {
                    ["name", "direction", "dir", "command", "cmd"]
                        .iter()
                        .find_map(|k| o.get(*k).and_then(Value::as_str).map(str::to_string))
                })?;
                Some(RawExit {
                    name,
                    to: id_text(
                        ["exitId", "id", "to", "target", "roomId", "destination"]
                            .iter()
                            .find_map(|k| o.get(*k)),
                    ),
                    door: door(o.get("door").or_else(|| o.get("doorState"))),
                    weight: number(o.get("weight")),
                    locked: truthy(o.get("locked").or_else(|| o.get("lock"))),
                    custom_line: o.get("customLine").is_some_and(|v| !v.is_null()),
                })
            }
            v => Some(RawExit {
                name: name?,
                to: id_text(Some(v)),
                door: None,
                weight: None,
                locked: None,
                custom_line: false,
            }),
        }
    };
    match value {
        Some(Value::Array(items)) => items.iter().filter_map(|i| one(None, i)).collect(),
        Some(Value::Object(o)) => o.iter().filter_map(|(k, v)| one(Some(k.clone()), v)).collect(),
        _ => Vec::new(),
    }
}

/// A per-direction table (`doors`, `exitWeights`, `exitLocks`): an object of name to value, or
/// a list of names (each `true`).
fn table(value: Option<&Value>) -> HashMap<String, Value> {
    match value {
        Some(Value::Object(o)) => o.iter().map(|(k, v)| (k.to_lowercase(), v.clone())).collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(|k| (k.to_lowercase(), Value::Bool(true)))
            .collect(),
        _ => HashMap::new(),
    }
}

/// Text cut to `max` UTF-16 units, control characters (but line breaks and tabs when
/// `multiline`) dropped.
fn clean(text: &str, max: usize, multiline: bool) -> String {
    let mut out = String::new();
    let mut units = 0;
    for c in text.chars() {
        if c.is_control() && !(multiline && matches!(c, '\n' | '\t')) {
            continue;
        }
        units += c.len_utf16();
        if units > max {
            break;
        }
        out.push(c);
    }
    out
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

/// Which way up the file is: `true` when most compass exits between rooms of one floor point
/// the wrong way for north at +Y.
fn upside_down(rooms: &HashMap<String, (String, f64, f64, f64)>, exits: &[(String, String, String)]) -> bool {
    let (mut right, mut wrong) = (0usize, 0usize);
    for (from, direction, to) in exits {
        let (Some(a), Some(b)) = (rooms.get(from), rooms.get(to)) else {
            continue;
        };
        if a.0 != b.0 || a.3 != b.3 {
            continue;
        }
        let expected = match direction.as_str() {
            "north" | "northeast" | "northwest" => 1.0,
            "south" | "southeast" | "southwest" => -1.0,
            _ => continue,
        };
        let dy = b.2 - a.2;
        if dy * expected > 0.0 {
            right += 1;
        } else if dy * expected < 0.0 {
            wrong += 1;
        }
    }
    wrong > right
}

/// A label of the file with its index, position and size.
type LabelPlace<'a> = (usize, &'a Value, (f64, f64, f64), (f64, f64));

fn convert(file: &MudletFile, progress: &dyn Fn(Progress)) -> Result<Prepared, ImportError> {
    let mut summary = ImportSummary::new(SourceKind::Mudlet);
    let mut skipped: IndexMap<Skip, usize> = IndexMap::new();
    let mut skip = |s: Skip, n: usize| {
        if n > 0 {
            *skipped.entry(s).or_default() += n;
        }
    };
    let total_rooms: usize = file.areas.iter().map(|a| a.rooms.len()).sum();
    if total_rooms > MAX_ROOMS {
        let exits = file
            .areas
            .iter()
            .flat_map(|a| &a.rooms)
            .map(|r| raw_exits(r.exits.as_ref()).len() + raw_exits(r.special_exits.as_ref()).len())
            .sum();
        return Err(ImportError::TooBig {
            rooms: total_rooms,
            exits,
        });
    }
    if file
        .user_data
        .as_ref()
        .is_some_and(|v| v.as_object().is_some_and(|o| !o.is_empty()))
    {
        skip(Skip::MapUserData, 1);
    }
    let colors = environment_colors(file.custom_env_colors.as_ref());
    // ---- ids: Mudlet's room id to ours, stable across imports ----
    let mut hashes_seen: HashMap<String, usize> = HashMap::new();
    for room in file.areas.iter().flat_map(|a| &a.rooms) {
        if let Some(h) = id_text(room.hash.as_ref()) {
            *hashes_seen.entry(h).or_default() += 1;
        }
    }
    let mut ours: HashMap<String, (String, Option<String>)> = HashMap::new();
    // Our id: (area key, x, y, z) as the file gives them.
    let mut placed: HashMap<String, (String, f64, f64, f64)> = HashMap::new();
    let mut area_names: Vec<(String, Option<String>)> = Vec::new();
    for area in &file.areas {
        let area_id = id_text(area.id.as_ref()).unwrap_or_default();
        let name = area
            .name
            .as_ref()
            .map(|n| clean(&value_text(n), 512, false))
            .filter(|n| !n.trim().is_empty() && area_id != "-1");
        area_names.push((area_id, name.clone()));
        for room in &area.rooms {
            let Some(id) = id_text(room.id.as_ref()) else {
                skip(Skip::BadRoom, 1);
                continue;
            };
            let at = triple(room.coordinates.as_ref())
                .or_else(|| Some((room.x?, room.y?, room.z.unwrap_or(0.0))))
                .filter(|(x, y, z)| format::coordinate(*x) && format::coordinate(*y) && format::coordinate(*z));
            let Some(at) = at else {
                skip(Skip::BadRoom, 1);
                continue;
            };
            if ours.contains_key(&id) {
                skip(Skip::BadRoom, 1);
                continue;
            }
            let hash = id_text(room.hash.as_ref()).filter(|h| {
                hashes_seen.get(h) == Some(&1) && h.chars().count() <= 120 && !h.chars().any(char::is_control)
            });
            let our_id = match &hash {
                Some(h) => format!("s:{h}"),
                None => format!("mudlet:{}", clean(&id, 240, false)),
            };
            ours.insert(id, (our_id.clone(), hash));
            placed.insert(our_id, (name.clone().unwrap_or_default(), at.0, at.1, at.2));
        }
    }
    // ---- exits first, for the way up ----
    let mut compass: Vec<(String, String, String)> = Vec::new();
    for room in file.areas.iter().flat_map(|a| &a.rooms) {
        let Some((from, _)) = id_text(room.id.as_ref()).and_then(|id| ours.get(&id)) else {
            continue;
        };
        for exit in raw_exits(room.exits.as_ref()) {
            if let (Some(d), Some((to, _))) = (
                normalize_direction(&exit.name),
                exit.to.as_ref().and_then(|t| ours.get(t)),
            ) {
                compass.push((from.clone(), d.to_string(), to.clone()));
            }
        }
    }
    let flip = upside_down(&placed, &compass);
    summary.flipped = flip;
    let y = |v: f64| if flip { -v } else { v };
    // ---- rooms and exits ----
    let mut rooms: Vec<MapRoom> = Vec::with_capacity(total_rooms);
    let mut links: Vec<MapLink> = Vec::new();
    let mut area_settings: Vec<MapAreaSettings> = Vec::new();
    let mut done = 0;
    for (area, (_, area_name)) in file.areas.iter().zip(&area_names) {
        if area
            .user_data
            .as_ref()
            .is_some_and(|v| v.as_object().is_some_and(|o| !o.is_empty()))
        {
            skip(Skip::AreaUserData, 1);
        }
        if truthy(area.grid_mode.as_ref()) == Some(true) {
            area_settings.push(MapAreaSettings::new(area_name.as_deref().unwrap_or(""), true));
        }
        for room in &area.rooms {
            done += 1;
            if done % 500 == 0 {
                progress(Progress {
                    stage: Stage::Rooms,
                    done,
                    total: total_rooms,
                });
            }
            let Some(mudlet_id) = id_text(room.id.as_ref()) else {
                continue;
            };
            let Some((id, hash)) = ours.get(&mudlet_id) else {
                continue;
            };
            let Some(&(_, x, ry, z)) = placed.get(id) else {
                continue;
            };
            let name = room
                .name
                .as_ref()
                .map(|n| clean(&value_text(n), 512, false))
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| tf(S::MapImportRoomName, &[&mudlet_id]));
            let mut r = MapRoom::new(id, name.trim(), "", area_name.as_deref(), x, y(ry), z, false);
            r.server_id = hash.clone();
            r.weight = number(room.weight.as_ref())
                .filter(|w| *w > 0.0 && *w <= 1e9)
                .unwrap_or(1.0);
            r.is_locked = truthy(room.locked.as_ref()).unwrap_or(false);
            if let Some(env) = number(room.environment.as_ref()).map(|e| e as i64)
                && let Some(c) = colors.get(&env)
            {
                r.color = Some(c.clone());
            }
            match room.symbol.as_ref() {
                Some(Value::String(s)) if !s.trim().is_empty() => r.symbol = Some(clean(s.trim(), 32, false)),
                Some(Value::Object(o)) => {
                    if let Some(s) = ["text", "symbol", "char"]
                        .iter()
                        .find_map(|k| o.get(*k).and_then(Value::as_str))
                        .filter(|s| !s.trim().is_empty())
                    {
                        r.symbol = Some(clean(s.trim(), 32, false));
                    }
                    if ["color", "color24RGB", "color32RGBA", "textColor"]
                        .iter()
                        .any(|k| o.get(*k).is_some_and(|v| !v.is_null()))
                    {
                        skip(Skip::SymbolColor, 1);
                    }
                }
                _ => {}
            }
            if r.symbol.as_deref().is_some_and(|s| s.is_empty()) {
                r.symbol = None;
            }
            summary.symbols += usize::from(r.symbol.is_some());
            // User data: a description entry is the description, the rest notes.
            if let Some(Value::Object(data)) = room.user_data.as_ref() {
                let mut notes = Vec::new();
                for (key, value) in data {
                    let text = value_text(value);
                    if text.trim().is_empty() {
                        continue;
                    }
                    if key.eq_ignore_ascii_case("description") && r.description.is_empty() {
                        r.description = clean(&text, 16_000, true);
                    } else {
                        notes.push(format!("{key}: {text}"));
                    }
                }
                if !notes.is_empty() {
                    r.notes = clean(&notes.join("\n"), 16_000, true);
                    summary.notes += 1;
                }
            }
            if room.custom_lines.as_ref().is_some_and(|v| match v {
                Value::Object(o) => !o.is_empty(),
                Value::Array(a) => !a.is_empty(),
                _ => false,
            }) {
                skip(Skip::CustomLine, 1);
            }
            // Exits.
            let doors = table(room.doors.as_ref());
            let weights = table(room.exit_weights.as_ref());
            let locks = table(room.exit_locks.as_ref());
            let mut keys: HashSet<String> = HashSet::new();
            let mut stubs: Vec<String> = Vec::new();
            match room.stub_exits.as_ref() {
                Some(Value::Array(items)) => {
                    for item in items {
                        let name = match item {
                            Value::String(s) => normalize_direction(s),
                            v => number(Some(v)).and_then(|n| stub_number(n as i64)),
                        };
                        if let Some(d) = name {
                            stubs.push(d.to_string());
                        }
                    }
                }
                Some(Value::Object(o)) => {
                    stubs.extend(o.keys().filter_map(|k| normalize_direction(k)).map(str::to_string));
                }
                _ => {}
            }
            let normal = raw_exits(room.exits.as_ref()).into_iter().map(|e| (e, false));
            let special = raw_exits(room.special_exits.as_ref()).into_iter().map(|e| (e, true));
            for (exit, listed_special) in normal.chain(special) {
                let standard = normalize_direction(&exit.name).filter(|_| !listed_special);
                let command = exit.name.trim();
                if command.is_empty() {
                    continue;
                }
                if standard.is_none() && command.to_lowercase().starts_with("script:") {
                    skip(Skip::ScriptExit, 1);
                    continue;
                }
                let Some((to, _)) = exit.to.as_ref().and_then(|t| ours.get(t)) else {
                    match (standard, exit.to.as_deref()) {
                        // An exit to nowhere is Mudlet's way of noting an unexplored one.
                        (Some(d), None | Some("-1") | Some("0")) => stubs.push(d.to_string()),
                        _ => skip(Skip::MissingRoom, 1),
                    }
                    continue;
                };
                let direction = match standard {
                    Some(d) => d.to_string(),
                    None => clean(&command.to_lowercase(), 64, false).trim().to_string(),
                };
                if direction.is_empty() || !keys.insert(direction.clone()) {
                    skip(Skip::DuplicateExit, 1);
                    continue;
                }
                let lookup = |t: &HashMap<String, Value>| {
                    t.get(&exit.name.to_lowercase())
                        .or_else(|| standard.and_then(|d| t.get(d)))
                        .or_else(|| t.get(&direction))
                        .cloned()
                };
                let mut link = MapLink::new(id, to, &direction, true);
                if standard.is_none() {
                    link.command = Some(clean(command, 512, false));
                    summary.special_exits += 1;
                }
                link.door_state = exit.door.or_else(|| door(lookup(&doors).as_ref())).unwrap_or_default();
                link.weight = exit
                    .weight
                    .or_else(|| number(lookup(&weights).as_ref()))
                    .filter(|w| w.is_finite() && *w >= 0.0 && *w <= 1e9)
                    .unwrap_or(0.0);
                link.is_locked = exit.locked.or_else(|| truthy(lookup(&locks).as_ref())).unwrap_or(false);
                if exit.custom_line {
                    skip(Skip::CustomLine, 1);
                }
                summary.doors += usize::from(link.door_state != DoorState::None);
                summary.locked_exits += usize::from(link.is_locked);
                if links.len() < MAX_LINKS {
                    links.push(link);
                }
                summary.exits += 1;
            }
            let mut known: Vec<String> = keys
                .iter()
                .filter(|k| normalize_direction(k).is_some())
                .cloned()
                .collect();
            known.extend(stubs);
            known.sort();
            known.dedup();
            known.truncate(64);
            r.known_exits = known;
            rooms.push(r);
        }
    }
    if summary.exits > MAX_LINKS {
        return Err(ImportError::TooBig {
            rooms: rooms.len(),
            exits: summary.exits,
        });
    }
    summary.rooms = rooms.len();
    summary.areas = rooms.iter().map(MapRoom::area_key).collect::<HashSet<_>>().len();
    // ---- labels ----
    let total_labels: usize = file.areas.iter().map(|a| a.labels.len()).sum();
    let mut labels: Vec<MapLabel> = Vec::new();
    let mut pictures: IndexMap<String, MapImage> = IndexMap::new();
    let mut picture_bytes = 0;
    let mut labels_done = 0;
    for (area, (area_id, area_name)) in file.areas.iter().zip(&area_names) {
        // Which way up the area's labels are: the way that puts more of them over its rooms.
        let bounds = placed
            .values()
            .filter(|p| p.0 == area_name.clone().unwrap_or_default())
            .fold(None, |b: Option<(f64, f64, f64, f64)>, p| {
                let py = y(p.2);
                Some(match b {
                    None => (p.1, p.1, py, py),
                    Some((a, b, c, d)) => (a.min(p.1), b.max(p.1), c.min(py), d.max(py)),
                })
            });
        let parsed: Vec<LabelPlace> = area
            .labels
            .iter()
            .enumerate()
            .filter_map(|(i, v)| {
                let o = v.as_object()?;
                let at = triple(
                    o.get("coordinates")
                        .or_else(|| o.get("position"))
                        .or_else(|| o.get("pos")),
                )
                .or_else(|| {
                    Some((
                        number(o.get("x"))?,
                        number(o.get("y"))?,
                        number(o.get("z")).unwrap_or(0.0),
                    ))
                });
                let size = pair(o.get("size")).or_else(|| Some((number(o.get("width"))?, number(o.get("height"))?)));
                match (at, size) {
                    (Some(at), Some(size)) => Some((i, v, at, size)),
                    _ => None,
                }
            })
            .collect();
        skip(Skip::EmptyLabel, area.labels.len() - parsed.len());
        let over = |flipped: bool| -> usize {
            let Some((x0, x1, y0, y1)) = bounds else { return 0 };
            parsed
                .iter()
                .filter(|(_, _, (x, ly, _), (w, h))| {
                    let top = if flipped { -ly } else { *ly };
                    x + w >= x0 - 2.0 && *x <= x1 + 2.0 && top >= y0 - 2.0 && top - h <= y1 + 2.0
                })
                .count()
        };
        let label_flip = if over(!flip) > over(flip) { !flip } else { flip };
        for (index, value, (x, ly, z), (w, h)) in parsed {
            labels_done += 1;
            if labels_done % 50 == 0 {
                progress(Progress {
                    stage: Stage::Labels,
                    done: labels_done,
                    total: total_labels,
                });
            }
            let o = value.as_object().expect("parsed objects");
            if !(format::coordinate(x) && format::coordinate(ly) && format::coordinate(z))
                || !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0)
            {
                skip(Skip::EmptyLabel, 1);
                continue;
            }
            if labels.len() >= MAX_LABELS {
                skip(Skip::LabelLimit, 1);
                continue;
            }
            let label_id = id_text(o.get("id")).unwrap_or_else(|| format!("#{index}"));
            let id = format!(
                "mudlet:label:{}:{}",
                clean(area_id, 60, false),
                clean(&label_id, 60, false)
            );
            let text = o
                .get("text")
                .and_then(Value::as_str)
                .map(|s| clean(s, 4_000, true))
                .unwrap_or_default();
            let mut label = MapLabel::text(
                &id,
                area_name.as_deref(),
                x,
                if label_flip { -ly } else { ly },
                z,
                text.trim_end(),
            );
            label.width = w.min(10_000.0);
            label.height = h.min(10_000.0);
            label.above_rooms = truthy(o.get("showOnTop").or_else(|| o.get("onTop"))).unwrap_or(false);
            if truthy(o.get("noScaling").or_else(|| o.get("fixedSize"))).unwrap_or(false) {
                skip(Skip::FixedSizeLabel, 1);
            }
            let colors = o.get("colors").and_then(Value::as_array);
            let fg = rgba(
                ["fgColor", "foregroundColor", "foreground", "textColor", "color"]
                    .iter()
                    .find_map(|k| o.get(*k))
                    .or_else(|| colors.and_then(|c| c.first())),
            );
            let bg = rgba(
                ["bgColor", "backgroundColor", "background"]
                    .iter()
                    .find_map(|k| o.get(*k))
                    .or_else(|| colors.and_then(|c| c.get(1))),
            );
            label.color = fg.map(hex);
            label.background = bg.filter(|c| c[3] > 0).map(hex);
            let lines = label.text.lines().count().max(1) as f64;
            label.font_size = number(o.get("fontSize"))
                .or_else(|| {
                    o.get("font")
                        .and_then(|f| number(f.get("size").or_else(|| f.get("pointSize"))))
                })
                .unwrap_or(h * 96.0 * 0.6 / lines)
                .clamp(6.0, 120.0)
                .round();
            if label.text.trim().is_empty() {
                // A picture label.
                let data = match o.get("image").or_else(|| o.get("pixmap")) {
                    Some(Value::String(s)) => images::decode_base64(s, 64 * 1024 * 1024),
                    Some(Value::Array(parts)) => {
                        let joined: String = parts.iter().filter_map(Value::as_str).collect();
                        images::decode_base64(&joined, 64 * 1024 * 1024)
                    }
                    _ => None,
                };
                let Some(data) = data.filter(|d| !d.is_empty()) else {
                    skip(Skip::EmptyLabel, 1);
                    continue;
                };
                let picture = match images::prepare(&data) {
                    Ok(p) => p,
                    Err(ImageError::TooLarge) | Err(ImageError::Unreadable) => {
                        skip(Skip::UnreadablePicture, 1);
                        continue;
                    }
                };
                if !pictures.contains_key(&picture.hash) {
                    if picture_bytes + picture.data.len() > MAX_IMAGES_BYTES {
                        skip(Skip::PictureLimit, 1);
                        continue;
                    }
                    picture_bytes += picture.data.len();
                    pictures.insert(picture.hash.clone(), picture.clone());
                }
                label.image = Some(picture.hash);
                summary.picture_labels += 1;
            } else {
                summary.text_labels += 1;
            }
            labels.push(label);
        }
    }
    summary.skipped = Skip::ALL
        .iter()
        .filter_map(|s| skipped.get(s).map(|n| (*s, *n)))
        .collect();
    let map = MapSnapshot {
        rooms,
        links,
        labels,
        images: pictures.into_values().collect(),
        area_settings,
        ..MapSnapshot::of(Vec::new(), Vec::new())
    };
    format::validate(&map).map_err(|e| ImportError::Invalid(e.0))?;
    Ok(Prepared { map, summary })
}

/// Mudlet's numbered directions (its mapper API's exit numbers): 1 north, 2 northeast, 3
/// northwest, 4 east, 5 west, 6 south, 7 southeast, 8 southwest, 9 up, 10 down, 11 in, 12 out.
fn stub_number(n: i64) -> Option<&'static str> {
    Some(match n {
        1 => "north",
        2 => "northeast",
        3 => "northwest",
        4 => "east",
        5 => "west",
        6 => "south",
        7 => "southeast",
        8 => "southwest",
        9 => "up",
        10 => "down",
        11 => "in",
        12 => "out",
        _ => return None,
    })
}

pub mod xml;

#[cfg(test)]
mod tests;
