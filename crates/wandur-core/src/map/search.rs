//! Room search (the C# `RoomSearch`): rooms whose observed name or description holds every term.
//! A double-quoted run is one term. Case does not matter. A manual label alone is never matched:
//! once a room is edited by hand, the server's own words (the `Observed*` fields) are searched.

use super::model::MapRoom;

/// The server's words for a room: its name and description until it is edited by hand, then
/// the observed ones (none for a room added by hand).
pub fn observed_text(room: &MapRoom) -> (Option<&str>, Option<&str>) {
    if room.is_manually_edited {
        (room.observed_name.as_deref(), room.observed_description.as_deref())
    } else {
        (Some(&room.name), Some(&room.description))
    }
}

/// Split a query: whitespace separates terms, a double-quoted run is one term.
pub fn parse_terms(query: &str) -> Vec<String> {
    let chars: Vec<char> = query.chars().collect();
    let mut terms = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        if chars[i] == '"' {
            let end = chars[i + 1..].iter().position(|c| *c == '"').map(|p| p + i + 1);
            let content: String = chars[i + 1..end.unwrap_or(chars.len())].iter().collect();
            let content = content.trim();
            if !content.is_empty() {
                terms.push(content.to_string());
            }
            i = end.map_or(chars.len(), |e| e + 1);
        } else {
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '"' {
                i += 1;
            }
            terms.push(chars[start..i].iter().collect());
        }
    }
    terms
}

/// Whether `room` matches every term (already lower-cased).
pub fn matches_terms(room: &MapRoom, terms: &[String]) -> bool {
    if terms.is_empty() {
        return false;
    }
    let (name, description) = observed_text(room);
    let (name, description) = (name.unwrap_or(""), description.unwrap_or(""));
    if name.is_empty() && description.is_empty() {
        return false;
    }
    let text = format!("{name}\n{description}").to_lowercase();
    terms.iter().all(|t| text.contains(t.as_str()))
}

pub fn matches(room: &MapRoom, query: &str) -> bool {
    matches_terms(room, &lower_terms(query))
}

fn lower_terms(query: &str) -> Vec<String> {
    parse_terms(query).into_iter().map(|t| t.to_lowercase()).collect()
}

/// The rooms that match, in their order.
pub fn search<'a>(rooms: impl IntoIterator<Item = &'a MapRoom>, query: &str) -> Vec<&'a MapRoom> {
    let terms = lower_terms(query);
    if terms.is_empty() {
        return Vec::new();
    }
    rooms.into_iter().filter(|r| matches_terms(r, &terms)).collect()
}
