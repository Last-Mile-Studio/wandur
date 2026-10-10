//! The map's data, in the C# client's shape (`MapModels.cs` and the SDK's `RoomObservation`), so
//! a saved map or an exported file reads the same in both clients: JSON property names are the
//! C# ones (PascalCase) and the enums are their numbers.

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Enums written as their number, as System.Text.Json writes them by default.
macro_rules! number_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident = $value:expr),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
        pub enum $name {
            #[default]
            $($(#[$vmeta])* $variant = $value),+
        }

        impl $name {
            pub fn from_number(n: i64) -> Option<Self> {
                match n {
                    $($value => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_i64(*self as i64)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let n = i64::deserialize(d)?;
                Self::from_number(n).ok_or_else(|| serde::de::Error::custom(concat!("not a ", stringify!($name))))
            }
        }
    };
}

number_enum!(
    /// How sure the tracker is of the player's position.
    TrackingState {
        /// Nothing observed yet and no map.
        Waiting = 0,
        /// A server room id says where the player is.
        Confirmed = 1,
        /// Text and movement evidence point at one room.
        Inferred = 2,
        /// Several rooms fit what was seen.
        Ambiguous = 3,
        /// A map exists but the position is not known.
        Unknown = 4,
    }
);

number_enum!(
    /// A door on an exit.
    DoorState {
        None = 0,
        Open = 1,
        Closed = 2,
        Locked = 3,
    }
);

number_enum!(
    /// Where a room observation came from.
    RoomSource {
        Text = 0,
        Gmcp = 1,
        Msdp = 2,
    }
);

fn one() -> f64 {
    1.0
}

/// A room. `Name`, `Description` and `Area` are what the person sees; once a room is edited by
/// hand the server's own words move to the `Observed*` fields, which recognition keeps using.
/// Coordinates are illustrative (north is +Y); topology is in the links.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MapRoom {
    pub id: String,
    pub name: String,
    pub description: String,
    pub area: Option<String>,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub provisional: bool,
    #[serde(default)]
    pub server_id: Option<String>,
    #[serde(default)]
    pub observed_name: Option<String>,
    #[serde(default)]
    pub observed_description: Option<String>,
    #[serde(default)]
    pub observed_area: Option<String>,
    #[serde(default)]
    pub known_exits: Vec<String>,
    #[serde(default)]
    pub environment: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub symbol: Option<String>,
    /// The room classifier's terrain (t13); presentation only, `environment` always wins.
    #[serde(default)]
    pub inferred_environment: Option<String>,
    #[serde(default)]
    pub inferred_confidence: Option<f64>,
    #[serde(default)]
    pub inferred_key: Option<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default = "one")]
    pub weight: f64,
    #[serde(default)]
    pub is_locked: bool,
    #[serde(default)]
    pub is_manually_edited: bool,
    #[serde(default)]
    pub revision: i64,
}

impl MapRoom {
    /// A room with the C# record's defaults (weight 1, no notes, no exits known).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: &str,
        name: &str,
        description: &str,
        area: Option<&str>,
        x: f64,
        y: f64,
        z: f64,
        provisional: bool,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: description.into(),
            area: area.map(str::to_string),
            x,
            y,
            z,
            provisional,
            server_id: None,
            observed_name: None,
            observed_description: None,
            observed_area: None,
            known_exits: Vec::new(),
            environment: None,
            color: None,
            symbol: None,
            inferred_environment: None,
            inferred_confidence: None,
            inferred_key: None,
            notes: String::new(),
            weight: 1.0,
            is_locked: false,
            is_manually_edited: false,
            revision: 0,
        }
    }

    /// The same room with a server id.
    pub fn with_server_id(mut self, id: &str) -> Self {
        self.server_id = Some(id.into());
        self
    }

    /// The area as the map's area list keys it (no area is "").
    pub fn area_key(&self) -> &str {
        self.area.as_deref().unwrap_or("")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MapLinePoint {
    pub x: f64,
    pub y: f64,
}

/// A directed exit: leaving `from_id` by `direction` arrives in `to_id`. A reverse exit is a
/// link of its own, never assumed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MapLink {
    pub from_id: String,
    pub to_id: String,
    pub direction: String,
    /// Seen with server room ids at both ends (else inferred from text).
    pub confirmed: bool,
    /// The movement command; none uses the direction.
    #[serde(default)]
    pub command: Option<String>,
    /// Traversal cost; zero uses the destination room's weight.
    #[serde(default)]
    pub weight: f64,
    #[serde(default)]
    pub is_locked: bool,
    #[serde(default)]
    pub is_manually_edited: bool,
    #[serde(default)]
    pub door_state: DoorState,
    #[serde(default)]
    pub line_points: Vec<MapLinePoint>,
    #[serde(default)]
    pub revision: i64,
}

impl MapLink {
    pub fn new(from: &str, to: &str, direction: &str, confirmed: bool) -> Self {
        Self {
            from_id: from.into(),
            to_id: to.into(),
            direction: direction.into(),
            confirmed,
            command: None,
            weight: 0.0,
            is_locked: false,
            is_manually_edited: false,
            door_state: DoorState::None,
            line_points: Vec::new(),
            revision: 0,
        }
    }

    /// Whether a route may not pass: locked, or a closed or locked door.
    pub fn blocked(&self) -> bool {
        self.is_locked || matches!(self.door_state, DoorState::Closed | DoorState::Locked)
    }
}

/// Per-area display settings (grid mode), saved with the map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MapAreaSettings {
    pub area: String,
    #[serde(default)]
    pub grid_mode: bool,
    #[serde(default)]
    pub revision: i64,
}

impl MapAreaSettings {
    pub fn new(area: &str, grid_mode: bool) -> Self {
        Self {
            area: area.into(),
            grid_mode,
            revision: 0,
        }
    }
}

/// A room id that now means another (a merge or an identity upgrade).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MapRoomAlias {
    pub source_id: String,
    pub target_id: String,
    pub revision: i64,
}

/// A tombstone: a deleted room an older save must not bring back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MapRoomDeletion {
    pub id: String,
    pub revision: i64,
}

/// A tombstone for an exit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MapLinkDeletion {
    pub from_id: String,
    pub direction: String,
    pub revision: i64,
}

/// A label drawn on the map (text, or a picture): a note, an area's name, a drawn landmark.
/// Its position is the top-left corner in map coordinates (north is +Y, so it extends east
/// and south), its size in room cells. A label is either text (`image` is `None`) or a
/// picture (`image` is the SHA-256 of a [`MapImage`] the map holds).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MapLabel {
    pub id: String,
    #[serde(default)]
    pub area: Option<String>,
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub z: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub text: String,
    /// The text's size in points at zoom 1 (it scales with the map).
    #[serde(default = "label_font_size")]
    pub font_size: f64,
    /// The text colour (`#RRGGBB`); none uses the map's text colour.
    #[serde(default)]
    pub color: Option<String>,
    /// The fill behind the text (`#RRGGBB`); none is transparent.
    #[serde(default)]
    pub background: Option<String>,
    /// The picture's SHA-256 (lowercase hex), for a picture label.
    #[serde(default)]
    pub image: Option<String>,
    /// The whole label's opacity, 0.05 to 1.
    #[serde(default = "one")]
    pub opacity: f64,
    /// Drawn over the rooms (else under them).
    #[serde(default)]
    pub above_rooms: bool,
    #[serde(default)]
    pub is_manually_edited: bool,
    #[serde(default)]
    pub revision: i64,
}

fn label_font_size() -> f64 {
    14.0
}

impl MapLabel {
    /// A text label with the defaults (14 point text, opaque, under the rooms).
    pub fn text(id: &str, area: Option<&str>, x: f64, y: f64, z: f64, text: &str) -> Self {
        Self {
            id: id.into(),
            area: area.map(str::to_string),
            x,
            y,
            z,
            width: 3.0,
            height: 1.0,
            text: text.into(),
            font_size: label_font_size(),
            color: None,
            background: None,
            image: None,
            opacity: 1.0,
            above_rooms: false,
            is_manually_edited: false,
            revision: 0,
        }
    }

    /// The area as the map's area list keys it (no area is "").
    pub fn area_key(&self) -> &str {
        self.area.as_deref().unwrap_or("")
    }
}

/// A tombstone for a label.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MapLabelDeletion {
    pub id: String,
    pub revision: i64,
}

/// A picture a label shows: PNG or JPEG bytes, keyed by their SHA-256 (lowercase hex), so the
/// same picture is stored once. The bytes are shared, so copies of a map (undo steps, saves)
/// cost nothing. In a map file the bytes are base64.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MapImage {
    pub hash: String,
    #[serde(
        serialize_with = "super::images::serialize_bytes",
        deserialize_with = "super::images::deserialize_bytes"
    )]
    pub data: std::sync::Arc<[u8]>,
}

/// A whole map: what is saved, exported, merged and kept for undo.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MapSnapshot {
    pub rooms: Vec<MapRoom>,
    pub links: Vec<MapLink>,
    pub candidate_room_ids: Vec<String>,
    #[serde(default)]
    pub current_room_id: Option<String>,
    #[serde(default)]
    pub state: TrackingState,
    #[serde(default)]
    pub source: RoomSource,
    #[serde(default)]
    pub observation_count: i32,
    #[serde(default)]
    pub room_aliases: Vec<MapRoomAlias>,
    #[serde(default)]
    pub area_settings: Vec<MapAreaSettings>,
    #[serde(default)]
    pub deleted_rooms: Vec<MapRoomDeletion>,
    #[serde(default)]
    pub deleted_links: Vec<MapLinkDeletion>,
    /// Labels; absent from files of maps without them (the C# client's shape).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<MapLabel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deleted_labels: Vec<MapLabelDeletion>,
    /// The pictures the labels show (only those a label uses).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<MapImage>,
}

impl MapSnapshot {
    /// A map of these rooms and links, nobody standing anywhere.
    pub fn of(rooms: Vec<MapRoom>, links: Vec<MapLink>) -> Self {
        Self {
            rooms,
            links,
            state: TrackingState::Unknown,
            ..Self::default()
        }
    }

    pub fn room(&self, id: &str) -> Option<&MapRoom> {
        self.rooms.iter().find(|r| r.id == id)
    }
}

/// What the server (or a room block in the text) said about the room the player is in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoomObservation {
    /// The server's room id, if it sent one that is not a placeholder.
    pub server_id: Option<String>,
    pub name: String,
    pub description: String,
    /// Exits by normalized direction, with the destination's server id when known.
    pub exits: IndexMap<String, Option<String>>,
    pub area: Option<String>,
    pub source: RoomSource,
    /// The server sent an exits field (even an empty one).
    pub exits_provided: bool,
    pub environment: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub z: Option<f64>,
    pub symbol: Option<String>,
}

impl RoomObservation {
    /// An observation with exits given as (direction, destination) pairs.
    pub fn new(server_id: Option<&str>, name: &str, description: &str, exits: &[(&str, Option<&str>)]) -> Self {
        Self {
            server_id: server_id.map(str::to_string),
            name: name.into(),
            description: description.into(),
            exits: exits
                .iter()
                .map(|(d, to)| ((*d).to_string(), to.map(str::to_string)))
                .collect(),
            ..Self::default()
        }
    }

    pub fn with_source(mut self, source: RoomSource) -> Self {
        self.source = source;
        self
    }

    pub fn with_area(mut self, area: &str) -> Self {
        self.area = Some(area.into());
        self
    }

    /// The characters it carries (a bound on queued observations).
    pub fn weight(&self) -> usize {
        self.name.len()
            + self.description.len()
            + self.server_id.as_ref().map_or(0, String::len)
            + self.area.as_ref().map_or(0, String::len)
            + self.environment.as_ref().map_or(0, String::len)
            + self
                .exits
                .iter()
                .map(|(k, v)| k.len() + v.as_ref().map_or(0, String::len))
                .sum::<usize>()
    }
}

/// `n`, `North`... to `north`; `None` for anything that is not a compass point, up, down, in
/// or out.
pub fn normalize_direction(value: &str) -> Option<&'static str> {
    Some(match value.trim().to_lowercase().as_str() {
        "n" | "north" => "north",
        "s" | "south" => "south",
        "e" | "east" => "east",
        "w" | "west" => "west",
        "ne" | "northeast" => "northeast",
        "nw" | "northwest" => "northwest",
        "se" | "southeast" => "southeast",
        "sw" | "southwest" => "southwest",
        "u" | "up" => "up",
        "d" | "down" => "down",
        "in" => "in",
        "out" => "out",
        _ => return None,
    })
}

/// The way back.
pub fn opposite(direction: &str) -> Option<&'static str> {
    Some(match direction {
        "north" => "south",
        "south" => "north",
        "east" => "west",
        "west" => "east",
        "northeast" => "southwest",
        "southwest" => "northeast",
        "northwest" => "southeast",
        "southeast" => "northwest",
        "up" => "down",
        "down" => "up",
        _ => return None,
    })
}

/// One step in map coordinates (north is +Y), as the C# tracker places new rooms. Anything
/// that is not a compass point, up or down steps east.
pub fn step(direction: Option<&str>) -> (f64, f64, f64) {
    match direction {
        Some("north") => (0.0, 1.0, 0.0),
        Some("south") => (0.0, -1.0, 0.0),
        Some("east") => (1.0, 0.0, 0.0),
        Some("west") => (-1.0, 0.0, 0.0),
        Some("northeast") => (1.0, 1.0, 0.0),
        Some("northwest") => (-1.0, 1.0, 0.0),
        Some("southeast") => (1.0, -1.0, 0.0),
        Some("southwest") => (-1.0, -1.0, 0.0),
        Some("up") => (0.0, 0.0, 1.0),
        Some("down") => (0.0, 0.0, -1.0),
        _ => (1.0, 0.0, 0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_uses_the_csharp_names_and_numbers() {
        let room = MapRoom::new("a", "Hall", "", Some("Keep"), 1.0, 2.0, 0.0, false);
        let json = serde_json::to_string(&room).unwrap();
        assert!(
            json.contains("\"Id\":\"a\"") && json.contains("\"Weight\":1.0"),
            "{json}"
        );
        let link = MapLink {
            door_state: DoorState::Locked,
            ..MapLink::new("a", "b", "north", true)
        };
        let json = serde_json::to_string(&link).unwrap();
        assert!(
            json.contains("\"DoorState\":3") && json.contains("\"FromId\":\"a\""),
            "{json}"
        );
        assert!(serde_json::from_str::<DoorState>("9").is_err());
    }
}
