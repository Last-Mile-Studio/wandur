//! A world's own appearance, as the directory lists it (the C# `WorldTheme`): a light or dark
//! palette of nine colours, an optional metallic surface, and an optional skin that repaints the
//! client's chassis (title band, toolbar, panel headers, footer, ground, the nameplate and the
//! device edge). Geometry is always the client's: a skin's paint is read, its shapes are not.
//!
//! A theme that does not validate is dropped (`None`), never an error: an unsupported theme must
//! not invalidate the listing that carries it. Theme art (tiled chrome images, bezel frames) is
//! validated and kept, but this client does not fetch or draw it yet.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::settings::is_color;

/// The nine palette colours every theme names.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldThemeColors {
    pub shell: String,
    pub panel: String,
    pub terminal: String,
    pub text: String,
    pub muted: String,
    pub accent: String,
    pub accent_secondary: String,
    pub border: String,
    pub terminal_text: String,
}

impl WorldThemeColors {
    pub fn values(&self) -> [&str; 9] {
        [
            &self.shell,
            &self.panel,
            &self.terminal,
            &self.text,
            &self.muted,
            &self.accent,
            &self.accent_secondary,
            &self.border,
            &self.terminal_text,
        ]
    }
}

/// One painted surface of the chassis: a vertical gradient from `from` to `to`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkinSurface {
    pub from: String,
    pub to: String,
}

/// The surfaces a world may repaint. Absent ones keep the client's.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkinSurfaces {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_bar: Option<SkinSurface>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub toolbar: Option<SkinSurface>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub panel_header: Option<SkinSurface>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub panel_body: Option<SkinSurface>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footer: Option<SkinSurface>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ground: Option<SkinSurface>,
}

/// Paint for the nameplate and the bracket behind it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlaquePaint {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wing_fill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wing_edge: Option<String>,
}

/// Paint for the device edge down the sides and along the bottom.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EdgePaint {
    pub color: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
}

/// What a world's skin may change: paint only.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkinPaint {
    pub surfaces: SkinSurfaces,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plaque: Option<PlaquePaint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge: Option<EdgePaint>,
}

impl SkinPaint {
    fn is_empty(&self) -> bool {
        self == &SkinPaint::default()
    }
}

/// A world's theme. Build it with [`WorldTheme::parse`], which applies the C# validation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldTheme {
    pub version: u32,
    pub id: String,
    pub name: String,
    /// `dark` or `light`: the chrome's lightness.
    pub variant: String,
    pub corner_radius: f64,
    pub colors: WorldThemeColors,
    /// `standard` or `metallic`.
    #[serde(default = "standard")]
    pub surface: String,
    /// Tiled chrome art the theme names (kept, not drawn by this client yet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chrome_image: Option<String>,
    /// A bezel frame the theme names (kept, not drawn by this client yet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bezel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skin: Option<SkinPaint>,
}

fn standard() -> String {
    "standard".into()
}

impl WorldTheme {
    /// Whether the chrome is light.
    pub fn is_light(&self) -> bool {
        self.variant == "light"
    }

    /// The C# `IsValid`.
    pub fn is_valid(&self) -> bool {
        let id_ok = (1..=80).contains(&self.id.len())
            && self
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
        let name_ok = (1..=100).contains(&self.name.encode_utf16().count())
            && !self.name.trim().is_empty()
            && !self.name.chars().any(char::is_control);
        self.version == 1
            && id_ok
            && name_ok
            && matches!(self.variant.as_str(), "dark" | "light")
            && self.corner_radius.is_finite()
            && (0.0..=16.0).contains(&self.corner_radius)
            && self.colors.values().iter().all(|c| is_color(c))
    }

    /// Read a theme from its directory JSON; `None` when it is missing a field or does not validate.
    pub fn parse(value: &Value) -> Option<WorldTheme> {
        let root = value.as_object()?;
        let colors_value = root.get("colors")?.as_object()?;
        let color = |key: &str| colors_value.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        let theme = WorldTheme {
            version: u32::try_from(root.get("version")?.as_i64()?).ok()?,
            id: root.get("id")?.as_str()?.to_string(),
            name: root.get("name")?.as_str()?.to_string(),
            variant: root.get("variant")?.as_str()?.to_string(),
            corner_radius: root.get("corner_radius")?.as_f64()?,
            colors: WorldThemeColors {
                shell: color("shell"),
                panel: color("panel"),
                terminal: color("terminal"),
                text: color("text"),
                muted: color("muted"),
                accent: color("accent"),
                accent_secondary: color("accent_secondary"),
                border: color("border"),
                terminal_text: color("terminal_text"),
            },
            surface: if root.get("surface").and_then(Value::as_str) == Some("metallic") {
                "metallic".into()
            } else {
                standard()
            },
            chrome_image: root.get("images").and_then(|i| i.get("chrome")).and_then(read_image),
            bezel: read_bezel(root.get("frame")),
            skin: root.get("skin").and_then(read_skin),
        };
        theme.is_valid().then_some(theme)
    }
}

/// Theme art is relative to the directory and nowhere else (the C# `IsThemeUrl`).
pub fn is_theme_url(url: &str) -> bool {
    (1..=2048).contains(&url.len())
        && !url.chars().any(char::is_control)
        && !url.contains('\\')
        && !url.starts_with('/')
        && !url.contains("//")
        && !url.contains(':')
        && !url.split('/').any(|p| p == "." || p == "..")
}

fn read_image(value: &Value) -> Option<String> {
    let url = value.get("url")?.as_str()?;
    let opacity = value.get("opacity").and_then(Value::as_f64).unwrap_or(0.12);
    (is_theme_url(url) && opacity.is_finite() && (0.0..=0.35).contains(&opacity)).then(|| url.to_string())
}

fn read_bezel(frame: Option<&Value>) -> Option<String> {
    let frame = frame?;
    if frame.get("kind")?.as_str()? != "bezel" {
        return None;
    }
    let url = frame.get("assets")?.get("border")?.get("url")?.as_str()?;
    is_theme_url(url).then(|| url.to_string())
}

fn hex_at(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|c| is_color(c))
        .map(str::to_string)
}

fn read_skin(value: &Value) -> Option<SkinPaint> {
    if !value.is_object() {
        return None;
    }
    let slot = |name: &str| -> Option<SkinSurface> {
        let s = value.get("surfaces")?.get(name)?;
        Some(SkinSurface {
            from: hex_at(s, "from")?,
            to: hex_at(s, "to")?,
        })
    };
    let plaque = value
        .get("layout")
        .and_then(|l| l.get("title_bar"))
        .and_then(|b| b.get("plaque"))
        .map(|p| PlaquePaint {
            fill: hex_at(p, "fill"),
            edge: hex_at(p, "edge"),
            accent: hex_at(p, "accent"),
            wing_fill: p.get("wings").and_then(|w| hex_at(w, "fill")),
            wing_edge: p.get("wings").and_then(|w| hex_at(w, "edge")),
        })
        .filter(|p| p != &PlaquePaint::default());
    let edge = value.get("edge").and_then(|e| {
        Some(EdgePaint {
            color: hex_at(e, "color")?,
            outline: hex_at(e, "outline"),
            accent: hex_at(e, "accent"),
        })
    });
    let paint = SkinPaint {
        surfaces: SkinSurfaces {
            title_bar: slot("title_bar"),
            toolbar: slot("toolbar"),
            panel_header: slot("panel_header"),
            panel_body: slot("panel_body"),
            footer: slot("footer"),
            ground: slot("ground"),
        },
        plaque,
        edge,
    };
    (!paint.is_empty()).then_some(paint)
}

/// A theme field that is dropped, not refused, when it does not parse or validate (for serde).
pub fn lenient<'de, D: Deserializer<'de>>(d: D) -> Result<Option<WorldTheme>, D::Error> {
    let value = Option::<Value>::deserialize(d)?;
    Ok(value.as_ref().and_then(|v| {
        // A theme saved by this client (its own field names) or one as the directory lists it.
        serde_json::from_value::<WorldTheme>(v.clone())
            .ok()
            .filter(WorldTheme::is_valid)
            .or_else(|| WorldTheme::parse(v))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/world-theme")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn the_fixture_themes_parse() {
        let theme = WorldTheme::parse(&fixture("world-theme.json")).unwrap();
        assert_eq!(theme.id, "lotj-navy-cyan-gold");
        assert_eq!(theme.name, "Legends of the Jedi");
        assert!(!theme.is_light());
        assert_eq!(theme.colors.accent_secondary, "#F4CD72");
        assert_eq!(theme.surface, "standard");
        assert!(theme.skin.is_none());

        let metallic = WorldTheme::parse(&fixture("world-theme-metallic.json")).unwrap();
        assert_eq!(metallic.surface, "metallic");
        assert_eq!(
            metallic.chrome_image.as_deref(),
            Some("theme-assets/brushed-gunmetal-v1.png")
        );

        let bezel = WorldTheme::parse(&fixture("world-theme-lotj-bezel.json")).unwrap();
        assert!(bezel.is_light());
        assert_eq!(bezel.bezel.as_deref(), Some("themes/imperial-bezel/border.png"));

        let industrial = WorldTheme::parse(&fixture("world-theme-industrial-skin.json")).unwrap();
        assert_eq!(industrial.colors.panel, "#D1D3D1");

        // The contract's skin is geometry and art only: the client keeps its own geometry, so
        // nothing of it is read, and the palette still applies.
        let contract = WorldTheme::parse(&fixture("world-theme-skin-contract.json")).unwrap();
        assert!(contract.skin.is_none());
        assert!(WorldTheme::parse(&fixture("world-theme-icesus.json")).is_some());
    }

    #[test]
    fn a_skin_repaints_surfaces_and_the_plaque_only() {
        let mut theme = fixture("world-theme.json");
        theme["skin"] = serde_json::json!({
            "version": 1,
            "surfaces": {
                "title_bar": {"from": "#112233", "to": "#223344", "bevel": "raised"},
                "toolbar": {"from": "nope", "to": "#000000"}
            },
            "layout": {"title_bar": {"height": 90, "plaque": {"shape": "round", "fill": "#0A0B0C", "wings": {"fill": "#ABCDEF"}}}},
            "edge": {"color": "#101010", "thickness": 40}
        });
        let skin = WorldTheme::parse(&theme).unwrap().skin.unwrap();
        assert_eq!(skin.surfaces.title_bar.as_ref().unwrap().to, "#223344");
        assert!(
            skin.surfaces.toolbar.is_none(),
            "a surface with a bad colour is left out"
        );
        let plaque = skin.plaque.unwrap();
        assert_eq!(plaque.fill.as_deref(), Some("#0A0B0C"));
        assert_eq!(plaque.wing_fill.as_deref(), Some("#ABCDEF"));
        assert_eq!(skin.edge.unwrap().color, "#101010");
    }

    #[test]
    fn invalid_themes_are_dropped_not_refused() {
        let good = fixture("world-theme.json");
        let mut bad = good.clone();
        bad["colors"]["accent"] = "teal".into();
        assert!(WorldTheme::parse(&bad).is_none());
        for (key, value) in [
            ("version", Value::from(2)),
            ("id", Value::from("")),
            ("id", Value::from("has space")),
            ("name", Value::from("  ")),
            ("variant", Value::from("sepia")),
            ("corner_radius", Value::from(17)),
        ] {
            let mut bad = good.clone();
            bad[key] = value;
            assert!(WorldTheme::parse(&bad).is_none(), "{key}");
        }
        let mut missing = good.clone();
        missing["colors"].as_object_mut().unwrap().remove("muted");
        assert!(WorldTheme::parse(&missing).is_none());
        // Art pointing at another host is ignored, the palette still applies.
        let mut art = good.clone();
        art["images"] = serde_json::json!({"chrome": {"url": "https://example.org/x.png"}});
        let theme = WorldTheme::parse(&art).unwrap();
        assert!(theme.chrome_image.is_none());
        assert!(!is_theme_url("../x.png") && !is_theme_url("/x.png") && is_theme_url("a/b.png"));
    }

    #[test]
    fn a_saved_theme_round_trips() {
        #[derive(Deserialize)]
        struct Holder {
            #[serde(deserialize_with = "lenient", default)]
            theme: Option<WorldTheme>,
        }
        let theme = WorldTheme::parse(&fixture("world-theme-metallic.json")).unwrap();
        let saved = serde_json::json!({ "theme": serde_json::to_value(&theme).unwrap() });
        let back: Holder = serde_json::from_value(saved).unwrap();
        assert_eq!(back.theme, Some(theme));
        let listed: Holder =
            serde_json::from_value(serde_json::json!({ "theme": fixture("world-theme.json") })).unwrap();
        assert_eq!(listed.theme.unwrap().id, "lotj-navy-cyan-gold");
        let broken: Holder = serde_json::from_value(serde_json::json!({ "theme": {"id": 3} })).unwrap();
        assert!(broken.theme.is_none());
    }
}
