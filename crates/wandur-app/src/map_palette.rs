//! Terrain colours and symbols for map rooms (the C# `MapEnvironmentPalette`): a room's colour
//! comes from its terrain (server or manual, else the classifier's guess), and its own colour
//! and symbol win over the palette's.

use egui::Color32;
use wandur_core::l10n::{S, t, tf};
use wandur_core::map::MapRoom;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainStyle {
    pub key: &'static str,
    pub label: S,
    pub color: u32,
    pub symbol: &'static str,
}

/// The C# palette, in its order.
pub const STYLES: [TerrainStyle; 14] = [
    style("unknown", S::MapTerrainUnknown, 0x77858E, ""),
    style("indoor", S::MapTerrainIndoor, 0xD9CBB0, ""),
    style("city", S::MapTerrainCity, 0xAC8990, ""),
    style("road", S::MapTerrainRoad, 0xAC851E, ""),
    style("grassland", S::MapTerrainGrassland, 0xA0C536, ""),
    style("forest", S::MapTerrainForest, 0x2E803F, ""),
    style("desert", S::MapTerrainDesert, 0xE8CD79, ""),
    style("mountain", S::MapTerrainMountain, 0x96988A, "△"),
    style("cave", S::MapTerrainCave, 0x726D87, ""),
    style("water", S::MapTerrainWater, 0x16A7D5, ""),
    style("underwater", S::MapTerrainUnderwater, 0x326AB5, ""),
    style("swamp", S::MapTerrainSwamp, 0x649888, ""),
    style("snow", S::MapTerrainSnow, 0xC8E1E7, ""),
    style("spacecraft", S::MapTerrainSpacecraft, 0x657A88, ""),
];

const fn style(key: &'static str, label: S, color: u32, symbol: &'static str) -> TerrainStyle {
    TerrainStyle {
        key,
        label,
        color,
        symbol,
    }
}

/// A terrain name as the palette knows it (`urban` is `city`, `woods` is `forest`...).
pub fn normalize(environment: Option<&str>) -> String {
    let Some(value) = environment else {
        return "unknown".into();
    };
    let value = value.trim().to_lowercase();
    match value.as_str() {
        "inside" | "indoors" | "building" => "indoor",
        "town" | "settlement" | "urban" => "city",
        "path" | "street" | "trail" => "road",
        "field" | "fields" | "plains" | "plain" | "grass" => "grassland",
        "woods" | "woodland" | "jungle" => "forest",
        "sand" | "beach" => "desert",
        "mountains" | "hills" | "hill" => "mountain",
        "underground" | "cavern" => "cave",
        "river" | "ocean" | "sea" | "lake" | "water_swim" | "water_noswim" => "water",
        "marsh" | "wetland" => "swamp",
        "ice" | "tundra" => "snow",
        "spaceship" | "space station" | "starship" => "spacecraft",
        _ => return value,
    }
    .into()
}

/// Whether the room's terrain is the classifier's guess (it has none of its own).
pub fn uses_inference(room: &MapRoom) -> bool {
    room.environment.as_deref().is_none_or(|e| e.trim().is_empty())
        && room
            .inferred_environment
            .as_deref()
            .is_some_and(|e| !e.trim().is_empty())
}

fn displayed(room: &MapRoom) -> Option<&str> {
    if uses_inference(room) {
        room.inferred_environment.as_deref()
    } else {
        room.environment.as_deref()
    }
}

/// The room's fill colour and symbol.
pub fn resolve(room: &MapRoom) -> (Color32, std::borrow::Cow<'_, str>) {
    let style = style_of(displayed(room));
    let color = room
        .color
        .as_deref()
        .and_then(crate::theme::parse_hex)
        .unwrap_or_else(|| hex(style.color));
    let symbol = match &room.symbol {
        Some(s) => std::borrow::Cow::Borrowed(s.as_str()),
        None => std::borrow::Cow::Borrowed(style.symbol),
    };
    (color, symbol)
}

/// The palette entry for a terrain, without allocating for the usual names.
fn style_of(environment: Option<&str>) -> &'static TerrainStyle {
    let Some(value) = environment.map(str::trim) else {
        return &STYLES[0];
    };
    if let Some(style) = STYLES.iter().find(|s| s.key.eq_ignore_ascii_case(value)) {
        return style;
    }
    let key = normalize(Some(value));
    STYLES.iter().find(|s| s.key == key).unwrap_or(&STYLES[0])
}

/// The terrain's name for people, or the world's own word for one the palette does not know.
pub fn label_for(environment: Option<&str>) -> String {
    let key = normalize(environment);
    match STYLES.iter().find(|s| s.key == key) {
        Some(style) => t(style.label).into(),
        None => environment.map_or_else(|| t(S::MapTerrainUnknown).to_string(), str::to_string),
    }
}

/// Tooltip text: the terrain, marked when the classifier guessed it.
pub fn describe(room: &MapRoom) -> String {
    let label = label_for(displayed(room));
    if uses_inference(room) {
        let percent = (room.inferred_confidence.unwrap_or(0.0) * 100.0).round();
        tf(S::MapTerrainInferred, &[&label, &percent])
    } else {
        label
    }
}

fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_names_map_to_the_palette_and_room_colours_win() {
        let mut room = MapRoom::new("a", "Hall", "", None, 0.0, 0.0, 0.0, false);
        room.environment = Some("Urban".into());
        assert_eq!(resolve(&room).0, hex(0xAC8990));
        room.color = Some("#112233".into());
        assert_eq!(resolve(&room).0, hex(0x112233));
        room.environment = Some("hills".into());
        assert_eq!(resolve(&room).1, "△");
        room.environment = Some("lava".into());
        assert_eq!(label_for(room.environment.as_deref()), "lava");
        room.environment = None;
        room.inferred_environment = Some("forest".into());
        room.inferred_confidence = Some(0.87);
        assert!(describe(&room).contains("87"), "{}", describe(&room));
    }

    fn room(id: &str, environment: Option<&str>, inferred: Option<&str>) -> MapRoom {
        let mut r = MapRoom::new(id, id, "", None, 0.0, 0.0, 0.0, false);
        r.environment = environment.map(str::to_string);
        r.inferred_environment = inferred.map(str::to_string);
        r.inferred_confidence = inferred.map(|_| 0.87);
        r
    }

    /// The C# `RoomInferenceRenderingTests`: server terrain wins, inference fills the gap,
    /// `urban` is a city, the room's own colour wins over both; only inferred terrain is marked.
    #[test]
    fn server_terrain_wins_and_inference_fills_the_gap() {
        assert_eq!(resolve(&room("a", Some("cave"), Some("forest"))).0, hex(0x726D87));
        assert_eq!(resolve(&room("b", None, Some("forest"))).0, hex(0x2E803F));
        assert_eq!(resolve(&room("c", None, Some("urban"))).0, hex(0xAC8990));
        assert_eq!(resolve(&room("d", None, None)).0, hex(0x77858E));
        let mut own = room("e", None, Some("forest"));
        own.color = Some("#123456".into());
        assert_eq!(resolve(&own).0, hex(0x123456));
        assert!(!describe(&room("a", Some("cave"), Some("forest"))).contains("inferred"));
        assert_eq!(describe(&room("b", None, Some("forest"))), "Forest · inferred 87%");
        assert!(uses_inference(&room("b", None, Some("forest"))));
        assert!(!uses_inference(&room("a", Some("cave"), Some("forest"))));
    }
}
