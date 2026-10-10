//! Room data from the server (the SDK's `RoomProtocolDecoder`): GMCP `Room.Info` and MSDP
//! `ROOM_*` variables (or a `ROOM` table), decoded conservatively. A placeholder id (`-1`, `0`,
//! `unknown`...) is no id; LOTJ's `O` and `C` exit flags are states, not room ids; coordinates
//! must be finite; an MSDP payload must carry the id and the name together, so a new id is never
//! combined with an older name.

use serde_json::Value;

use super::model::{RoomObservation, RoomSource};
use crate::protocol::GmcpMessage;
use crate::protocol::msdp::{self, MsdpValue};

/// Largest payload decoded (the C# limit).
const MAX_PAYLOAD: usize = 16_384;

fn valid_coordinate(value: f64) -> bool {
    value.is_finite() && value.abs() <= 1e9
}

/// A `Room.Info` message as an observation, or `None` when it is something else, has no name
/// or does not parse.
pub fn from_gmcp(message: &GmcpMessage) -> Option<RoomObservation> {
    if message.raw.len() > MAX_PAYLOAD || !message.is("Room.Info") {
        return None;
    }
    // A number out of double range fails serde_json's parse, where .NET reads it (and the
    // coordinate check then drops it): read such a body again with those numbers as null.
    let lenient;
    let data = match &message.data {
        Some(data) => data,
        None => {
            let raw = String::from_utf8_lossy(&message.raw);
            let body = raw.trim().split_once(char::is_whitespace)?.1;
            lenient = serde_json::from_str::<Value>(&null_huge_numbers(body)).ok()?;
            &lenient
        }
    };
    let root = data.as_object()?;
    let name = json_text(root, "name")?;
    if name.trim().is_empty() {
        return None;
    }
    let mut room = RoomObservation {
        source: RoomSource::Gmcp,
        ..RoomObservation::default()
    };
    // LOTJ reports a vnum and a planet; its O/C exit values are states, not destination ids.
    let state_exits = root.contains_key("vnum") && json_text(root, "planet").is_some();
    match root.get("exits") {
        Some(Value::Object(exits)) => {
            room.exits_provided = true;
            for (direction, value) in exits.iter().take(64) {
                let state = state_exits && matches!(value.as_str(), Some("O" | "C"));
                add_exit(&mut room, direction, if state { None } else { json_id(value) });
            }
        }
        Some(Value::Array(exits)) => {
            room.exits_provided = true;
            for direction in exits.iter().take(64).filter_map(Value::as_str) {
                add_exit(&mut room, direction, None);
            }
        }
        _ => {}
    }
    room.server_id = ["num", "id", "vnum"]
        .iter()
        .find_map(|key| root.get(*key).and_then(json_id));
    room.name = name.trim().to_string();
    room.description = json_text(root, "desc")
        .or_else(|| json_text(root, "description"))
        .unwrap_or_default()
        .to_string();
    room.area = ["area", "zone", "planet"]
        .iter()
        .find_map(|key| json_text(root, key))
        .map(str::to_string);
    room.environment = json_text(root, "environment")
        .or_else(|| json_text(root, "terrain"))
        .map(str::to_string);
    room.symbol = json_text(root, "symbol").map(str::to_string);
    room.x = json_coordinate(root, "x");
    room.y = json_coordinate(root, "y");
    room.z = json_coordinate(root, "z");
    Some(room)
}

/// An MSDP payload as an observation, when it names the room's id and name together.
pub fn from_msdp(payload: &[u8]) -> Option<RoomObservation> {
    let table = msdp::parse(payload)?;
    let nested = table.iter().find_map(|(k, v)| match v {
        MsdpValue::Table(inner) if k == "ROOM" => Some(inner),
        _ => None,
    });
    let fields: &[(String, MsdpValue)] = nested.map_or(table.as_slice(), Vec::as_slice);
    let get = |key: &str| fields.iter().find(|(k, _)| k == key).map(|(_, v)| v);
    let text = |key: &str| get(key).and_then(MsdpValue::as_text);
    let key = |plain: &'static str, prefixed: &'static str| if nested.is_some() { plain } else { prefixed };
    let id = get(key("VNUM", "ROOM_VNUM"))?;
    let name = text(key("NAME", "ROOM_NAME"))?;
    if name.trim().is_empty() {
        return None;
    }
    let mut room = RoomObservation {
        source: RoomSource::Msdp,
        server_id: id.as_text().and_then(normalize_id),
        name: name.trim().to_string(),
        ..RoomObservation::default()
    };
    match get(key("EXITS", "ROOM_EXITS")) {
        Some(MsdpValue::Table(exits)) => {
            room.exits_provided = true;
            for (direction, value) in exits.iter().take(64) {
                add_exit(&mut room, direction, value.as_text().and_then(normalize_id));
            }
        }
        Some(MsdpValue::Array(exits)) => {
            room.exits_provided = true;
            for direction in exits.iter().take(64).filter_map(MsdpValue::as_text) {
                add_exit(&mut room, direction, None);
            }
        }
        _ => {}
    }
    room.area = text(key("AREA", "AREA_NAME")).map(str::to_string);
    room.environment = text(key("ENVIRONMENT", "ROOM_ENVIRONMENT"))
        .or_else(|| text(key("TERRAIN", "ROOM_TERRAIN")))
        .map(str::to_string);
    room.symbol = text(key("SYMBOL", "ROOM_SYMBOL")).map(str::to_string);
    let coordinates = match get(key("COORDINATES", "ROOM_COORDINATES")) {
        Some(MsdpValue::Table(c)) => Some(c),
        _ => None,
    };
    let axis = |plain: &'static str, prefixed: &'static str| -> Option<f64> {
        match coordinates {
            Some(c) => c
                .iter()
                .find(|(k, _)| k == plain)
                .and_then(|(_, v)| v.as_text())
                .and_then(number_text),
            None => text(key(plain, prefixed)).and_then(number_text),
        }
    };
    room.x = axis("X", "ROOM_X");
    room.y = axis("Y", "ROOM_Y");
    room.z = axis("Z", "ROOM_Z");
    Some(room)
}

/// `body` with every number literal that is not a finite double replaced by `null`.
fn null_huge_numbers(body: &str) -> String {
    let bytes = body.as_bytes();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' {
            let start = i;
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                i += if bytes[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(bytes.len());
            out.push_str(&body[start..i]);
        } else if c == b'-' || c.is_ascii_digit() {
            let start = i;
            while i < bytes.len() && matches!(bytes[i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
                i += 1;
            }
            let token = &body[start..i];
            if token.parse::<f64>().is_ok_and(f64::is_finite) {
                out.push_str(token);
            } else {
                out.push_str("null");
            }
        } else {
            let len = body[i..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&body[i..i + len]);
            i += len;
        }
    }
    out
}

fn json_text<'a>(root: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    root.get(key).and_then(Value::as_str)
}

fn json_id(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => normalize_id(s),
        Value::Number(n) => n.as_i64().filter(|n| *n > 0).map(|n| n.to_string()),
        _ => None,
    }
}

fn json_coordinate(root: &serde_json::Map<String, Value>, axis: &str) -> Option<f64> {
    // Each observation is decoded on its own: a missing axis never borrows another room's.
    for key in ["coord", "coords", "coordinates"] {
        if let Some(Value::Object(c)) = root.get(key)
            && let Some(component) = c.get(axis)
        {
            return json_number(component);
        }
    }
    root.get(axis).and_then(json_number)
}

fn json_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64().filter(|v| valid_coordinate(*v)),
        Value::String(s) => number_text(s),
        _ => None,
    }
}

fn number_text(text: &str) -> Option<f64> {
    let text = text.trim();
    let lower = text.to_ascii_lowercase();
    // .NET's invariant parse knows no "inf"; both forms fail the finite check anyway.
    if lower.contains("inf") || lower.contains("nan") {
        return None;
    }
    text.parse::<f64>().ok().filter(|v| valid_coordinate(*v))
}

/// A server room id, or `None` for placeholders and numbers that are not whole and positive.
pub fn normalize_id(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.encode_utf16().count() > 128 || value.chars().any(char::is_control) {
        return None;
    }
    if matches!(
        value.to_lowercase().as_str(),
        "unknown"
            | "null"
            | "none"
            | "nil"
            | "undefined"
            | "false"
            | "true"
            | "nan"
            | "inf"
            | "infinity"
            | "-infinity"
            | "?"
            | "-1"
            | "0"
    ) {
        return None;
    }
    match decimal_integer(value) {
        Number::Opaque => Some(value.to_string()),
        Number::PositiveWhole(digits) => Some(digits),
        Number::Other => None,
    }
}

enum Number {
    /// Not a number: the text is an opaque id.
    Opaque,
    PositiveWhole(String),
    Other,
}

/// Read `value` as .NET's `decimal.TryParse` with `NumberStyles.Float` would: sign, digits, a
/// point and an exponent. Whole positive values come back as plain digits ("1e3" is "1000").
fn decimal_integer(value: &str) -> Number {
    let bytes = value.as_bytes();
    let mut i = 0;
    let mut negative = false;
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        negative = bytes[0] == b'-';
        i = 1;
    }
    let mut digits = String::new();
    let mut fraction = 0i64;
    let mut seen_digit = false;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        digits.push(bytes[i] as char);
        seen_digit = true;
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            digits.push(bytes[i] as char);
            fraction += 1;
            seen_digit = true;
            i += 1;
        }
    }
    if !seen_digit {
        return Number::Opaque;
    }
    let mut exponent = 0i64;
    if i < bytes.len() && matches!(bytes[i], b'e' | b'E') {
        i += 1;
        let mut sign = 1;
        if i < bytes.len() && matches!(bytes[i], b'+' | b'-') {
            sign = if bytes[i] == b'-' { -1 } else { 1 };
            i += 1;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            exponent = (exponent * 10 + i64::from(bytes[i] - b'0')).min(10_000);
            i += 1;
        }
        if i == start {
            return Number::Opaque;
        }
        exponent *= sign;
    }
    if i != bytes.len() {
        return Number::Opaque;
    }
    let scale = exponent - fraction;
    let mut digits = digits.trim_start_matches('0').to_string();
    if digits.is_empty() {
        return Number::Other;
    }
    if scale >= 0 {
        if digits.len() as i64 + scale > 29 {
            // Beyond decimal's range: .NET's parse fails and the text is kept as an opaque id.
            return Number::Opaque;
        }
        digits.extend(std::iter::repeat_n('0', scale as usize));
    } else {
        let drop = (-scale) as usize;
        if drop >= digits.len() || !digits[digits.len() - drop..].bytes().all(|b| b == b'0') {
            return Number::Other;
        }
        digits.truncate(digits.len() - drop);
    }
    if negative {
        Number::Other
    } else {
        Number::PositiveWhole(digits)
    }
}

fn add_exit(room: &mut RoomObservation, direction: &str, id: Option<String>) {
    let lower = direction.trim().to_lowercase();
    let direction = match lower.as_str() {
        "n" => "north",
        "s" => "south",
        "e" => "east",
        "w" => "west",
        "ne" => "northeast",
        "nw" => "northwest",
        "se" => "southeast",
        "sw" => "southwest",
        "u" => "up",
        "d" => "down",
        other => other,
    };
    if !direction.is_empty() && direction.encode_utf16().count() <= 64 && !direction.chars().any(char::is_control) {
        room.exits.insert(direction.to_string(), id);
    }
}
