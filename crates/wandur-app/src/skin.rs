//! Window skins, chosen independently of the colour theme (the C# `WindowSkinDefinition`,
//! `TitleBarMetrics`, `TitleBarLayout`, `DefaultSkin`, `FleetSkin`, `ArmoredSkinRenderer`):
//!
//! - **Fleet** (the default): a 38 point metal title band with a dark nameplate projecting below
//!   it, a toolbar row whose rim dips around the plate, flat 30 point panel headers, and a 6
//!   point bevelled device edge down the sides and along the bottom.
//! - **Armored**: a 60 point band of machined plates, 24 point side rails, a 30 point foot, a
//!   deeper plate with lamps in its shoulders, the owner's wear texture on the metal, and 30 point
//!   panel headers.
//! - **System**: no drawn frame. One plain toolbar row in the caption area holds the icon, the
//!   title and the controls; the gaps between panels take the panel colour.
//!
//! The sizes are numbers in [`TitleBarMetrics`] and [`WindowSkin`]; the shapes are the painters
//! below; every colour comes from the theme in force ([`Chrome::new`]), so the same skin is right
//! on a light theme, a dark one, a custom theme and a world's theme. A world's theme may repaint
//! surfaces, never the geometry.
//!
//! The pure parts (metrics, title placement, the plate title choice) are tested without a UI.

use egui::epaint::{CornerRadius, RectShape, TessellationOptions, Tessellator};
use egui::epaint::{Mesh, Vertex};
use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, TextureHandle, TextureId, pos2, vec2};
use wandur_core::directory::world_theme::SkinPaint;
use wandur_core::l10n::{S, t};

use crate::theme::{Theme, contrast, is_light, mix, parse_hex};

/// The name the app goes by, on the title plate (`Wandur Mud Client - world`).
pub const APP_NAME: &str = "Wandur Mud Client";
/// The plate's short form when the full name does not fit.
pub const SHORT_NAME: &str = "Wandur";

/// The space the native macOS traffic lights take at the left of the caption area.
pub const MAC_CAPTION_LEFT: f32 = 88.0;
/// The space kept for the caption buttons at the right on Windows and Linux.
pub const CAPTION_BUTTONS_WIDTH: f32 = 144.0;
/// The Skin, palette and full screen buttons beside the plate, at full size.
pub const TITLE_ACTIONS_WIDTH: f32 = 120.0;
/// Armored's side rails and foot.
pub const ARMORED_RAIL_WIDTH: f32 = 24.0;
pub const ARMORED_FOOT_HEIGHT: f32 = 30.0;
/// The plate's icon gap and the System toolbar's row height.
pub const TITLE_LOGO_GAP: f32 = 10.0;
pub const SYSTEM_TOOLBAR_HEIGHT: f32 = 48.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum SkinId {
    #[default]
    Fleet,
    Armored,
    System,
}

impl SkinId {
    pub const ALL: [SkinId; 3] = [SkinId::Fleet, SkinId::Armored, SkinId::System];

    /// A skin by its settings name; anything unknown is Fleet.
    pub fn from_name(name: &str) -> SkinId {
        match wandur_core::settings::normalize_skin(name) {
            "Armored" => SkinId::Armored,
            "System" => SkinId::System,
            _ => SkinId::Fleet,
        }
    }

    /// The settings name.
    pub fn name(self) -> &'static str {
        match self {
            SkinId::Fleet => "Fleet",
            SkinId::Armored => "Armored",
            SkinId::System => "System",
        }
    }

    pub fn label(self) -> &'static str {
        t(match self {
            SkinId::Fleet => S::SkinFleet,
            SkinId::Armored => S::SkinArmored,
            SkinId::System => S::SkinSystem,
        })
    }
}

/// Every size a drawn skin chooses for its title bar (the C# `TitleBarMetrics`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TitleBarMetrics {
    /// The band across the top of the window.
    pub band_height: f32,
    /// Where the plate starts below the top of the band.
    pub plaque_top: f32,
    /// How far the plate projects below the band, into the toolbar's upper ledge.
    pub plaque_drop: f32,
    pub title_font_size: f32,
    pub title_letter_spacing: f32,
    pub logo_size: f32,
    /// The room kept clear on each side of the icon and title inside the plate.
    pub text_inset: f32,
    /// The toolbar's top padding, which clears the plate's projection.
    pub toolbar_top_padding: f32,
    pub toolbar_min_height: f32,
    /// With the toolbar hidden, what the content below keeps clear of the plate.
    pub hidden_toolbar_clearance: f32,
    pub action_button_size: f32,
    /// How large the title buttons are drawn (1 is full size), about their right edge.
    pub action_scale: f32,
    /// The height the buttons centre on; `None` is the middle of the band.
    pub actions_center: Option<f32>,
}

impl TitleBarMetrics {
    /// Fleet: a 38 point band with a plate projecting 10 points below it.
    pub const FLEET: TitleBarMetrics = TitleBarMetrics {
        band_height: 38.0,
        plaque_top: 2.0,
        plaque_drop: 10.0,
        title_font_size: 13.0,
        title_letter_spacing: 1.0,
        logo_size: 24.0,
        text_inset: 44.0 + 40.0,
        toolbar_top_padding: 10.0 + 1.0,
        toolbar_min_height: 10.0 + 1.0 + 36.0 + 5.0,
        hidden_toolbar_clearance: 10.0 + 2.0,
        action_button_size: 30.0,
        action_scale: 0.75,
        actions_center: None,
    };

    /// Armored: a 60 point band with a deeper plate projecting 8 points below it; the buttons
    /// sit on the upper plate, between its top edge at 4 and the joint at band - 18.
    pub const ARMORED: TitleBarMetrics = TitleBarMetrics {
        band_height: 60.0,
        plaque_top: 2.0,
        plaque_drop: 8.0,
        title_font_size: 13.0,
        title_letter_spacing: 1.0,
        logo_size: 24.0,
        text_inset: 100.0,
        toolbar_top_padding: 8.0 + 5.0,
        toolbar_min_height: 8.0 + 5.0 + 36.0 + 5.0,
        hidden_toolbar_clearance: 8.0 + 6.0,
        action_button_size: 30.0,
        action_scale: 0.75,
        actions_center: Some((4.0 + (60.0 - 18.0)) / 2.0),
    };

    pub fn plaque_height(&self) -> f32 {
        self.band_height - self.plaque_top + self.plaque_drop
    }

    pub fn actions_center(&self) -> f32 {
        self.actions_center.unwrap_or(self.band_height / 2.0)
    }

    /// The top of the buttons' full-size box, centred on [`Self::actions_center`].
    pub fn actions_top(&self) -> f32 {
        self.actions_center() - self.action_button_size / 2.0
    }
}

/// A skin's geometry: drawn title bar metrics (none for System), the frame around the content,
/// and the panel header height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowSkin {
    pub id: SkinId,
    pub title_bar: Option<TitleBarMetrics>,
    /// Left, top, right, bottom.
    pub frame: [f32; 4],
    pub dock_header_height: f32,
}

impl WindowSkin {
    pub const FLEET: WindowSkin = WindowSkin {
        id: SkinId::Fleet,
        title_bar: Some(TitleBarMetrics::FLEET),
        frame: [6.0, 0.0, 6.0, 6.0],
        // 30 since the C# `ui/flat-headers` work (38 before).
        dock_header_height: 30.0,
    };
    pub const ARMORED: WindowSkin = WindowSkin {
        id: SkinId::Armored,
        title_bar: Some(TitleBarMetrics::ARMORED),
        frame: [ARMORED_RAIL_WIDTH, 0.0, ARMORED_RAIL_WIDTH, ARMORED_FOOT_HEIGHT],
        dock_header_height: 30.0,
    };
    pub const SYSTEM: WindowSkin = WindowSkin {
        id: SkinId::System,
        title_bar: None,
        frame: [0.0; 4],
        dock_header_height: 30.0,
    };

    pub fn of(id: SkinId) -> WindowSkin {
        match id {
            SkinId::Fleet => Self::FLEET,
            SkinId::Armored => Self::ARMORED,
            SkinId::System => Self::SYSTEM,
        }
    }

    /// A skin by its settings name; unknown names are Fleet.
    pub fn resolve(name: &str) -> WindowSkin {
        Self::of(SkinId::from_name(name))
    }

    /// The skin draws its own title bar.
    pub fn custom_chrome(&self) -> bool {
        self.title_bar.is_some()
    }

    /// The band's height, or zero when the system draws the title bar.
    pub fn title_height(&self) -> f32 {
        self.title_bar.map_or(0.0, |m| m.band_height)
    }
}

/// Where the plate goes, and whether it is too narrow for its shoulders (then the title is shown
/// plain, without a plate).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TitlePlacement {
    pub rect: Rect,
    pub plain: bool,
}

fn safe(value: f32) -> f32 {
    if value.is_finite() { value.max(0.0) } else { 0.0 }
}

/// The width left for the icon and title on a plate between the caption controls, given the
/// plate's own text inset on each side (the C# `TitleBarLayout.TextRoom`).
pub fn text_room(window_width: f32, left: f32, right: f32, text_inset: f32) -> f32 {
    let exclusion = safe(left).max(safe(right)) + 12.0;
    (safe(window_width) - exclusion * 2.0 - text_inset * 2.0).max(0.0)
}

/// The plate, centred on the window and clear of the wider caption area on both sides; too
/// narrow for its shoulders and a short title, it is a plain title (`TitleBarLayout.Calculate`).
pub fn place_title(
    metrics: &TitleBarMetrics,
    window_width: f32,
    left: f32,
    right: f32,
    measured: f32,
) -> TitlePlacement {
    let w = safe(window_width);
    let exclusion = safe(left).max(safe(right)) + 12.0;
    let available = (w - exclusion * 2.0).max(0.0);
    // Whole, even widths keep the plate centred on the pixel grid.
    let width = ((safe(measured) / 2.0).ceil() * 2.0 + metrics.text_inset * 2.0).min(available);
    TitlePlacement {
        rect: Rect::from_min_size(
            pos2((w - width) / 2.0, metrics.plaque_top),
            vec2(width, metrics.plaque_height()),
        ),
        plain: width < metrics.text_inset * 2.0 + 48.0,
    }
}

/// The plate's two labels: the app name, then " - world" once a session is open.
pub fn plate_labels(world: Option<&str>) -> (String, String) {
    match world.map(str::trim).filter(|w| !w.is_empty()) {
        Some(world) => (format!("{APP_NAME} - {world}"), format!("{SHORT_NAME} - {world}")),
        None => (APP_NAME.into(), SHORT_NAME.into()),
    }
}

/// The plate's title, engraved in capitals: the full label when it fits `room`, else the short
/// one (a world name that still does not fit is trimmed when it is drawn).
pub fn plate_title(full: &str, short: &str, room: f32, measure: impl Fn(&str) -> f32) -> String {
    let full = full.to_uppercase();
    if measure(&full) <= room {
        full
    } else {
        short.to_uppercase()
    }
}

/// The caption area kept clear at the left and right of the band on this platform: the native
/// traffic lights on macOS, the drawn caption buttons elsewhere, and the title buttons.
pub fn caption_exclusion(mac: bool, metrics: &TitleBarMetrics) -> (f32, f32) {
    let left = if mac { MAC_CAPTION_LEFT } else { 0.0 };
    (
        left,
        actions_right_inset(mac) + TITLE_ACTIONS_WIDTH * metrics.action_scale,
    )
}

/// Where the title buttons end, from the window's right edge.
pub fn actions_right_inset(mac: bool) -> f32 {
    if mac { 12.0 } else { CAPTION_BUTTONS_WIDTH + 12.0 }
}

// ---------------------------------------------------------------------------------------------
// Colours

/// A vertical gradient: stops from 0 (top) to 1 (bottom).
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient(pub Vec<(f32, Color32)>);

impl Gradient {
    pub fn flat(color: Color32) -> Gradient {
        Gradient(vec![(0.0, color), (1.0, color)])
    }

    fn two(from: Color32, to: Color32) -> Gradient {
        Gradient(vec![(0.0, from), (1.0, to)])
    }

    /// A surface with a raised bevel: a light first stop and a dark last one (the C#
    /// `ThemeSkinSurfaces.Build` for `bevel: raised`).
    fn raised(from: Color32, to: Color32, strength: f32) -> Gradient {
        const EDGE: f32 = 0.035;
        Gradient(vec![
            (0.0, shift(from, 0.55 * strength)),
            (EDGE, from),
            (1.0 - EDGE, to),
            (1.0, shift(to, -0.30 * strength)),
        ])
    }

    fn of(stops: &[(u32, f32)]) -> Gradient {
        Gradient(stops.iter().map(|&(c, o)| (o, rgb(c))).collect())
    }

    /// The colour at the middle, which is what text on the surface reads against.
    pub fn middle(&self) -> Color32 {
        self.at(0.5)
    }

    pub fn at(&self, y: f32) -> Color32 {
        let stops = &self.0;
        let Some(first) = stops.first() else {
            return Color32::TRANSPARENT;
        };
        if y <= first.0 {
            return first.1;
        }
        for pair in stops.windows(2) {
            let ((a, ca), (b, cb)) = (pair[0], pair[1]);
            if y <= b {
                let t = if b > a { (y - a) / (b - a) } else { 1.0 };
                return mix(ca, cb, t);
            }
        }
        stops.last().map_or(Color32::TRANSPARENT, |s| s.1)
    }
}

const fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Toward white (positive) or black (negative).
fn shift(c: Color32, amount: f32) -> Color32 {
    if amount >= 0.0 {
        mix(c, Color32::WHITE, amount)
    } else {
        mix(c, Color32::BLACK, -amount)
    }
}

/// The colours a skin paints, made once per theme or skin change.
#[derive(Clone, Debug, PartialEq)]
pub struct Chrome {
    pub skin: WindowSkin,
    /// The title band.
    pub band: Gradient,
    pub toolbar: Gradient,
    pub header: Gradient,
    pub footer: Gradient,
    /// The chassis the panels rest on (the gaps between them).
    pub ground: Color32,
    /// The plate: its face, edge, lamps, and the bracket behind it.
    pub plaque: Gradient,
    pub plaque_edge: Color32,
    pub plaque_accent: Color32,
    pub wing: Gradient,
    pub wing_edge: Color32,
    /// The device edge: its metal, outline and accent light.
    pub edge: Color32,
    pub edge_outline: Color32,
    pub edge_accent: Color32,
    pub rim_edge: Color32,
    pub rim_highlight: Color32,
    pub rim_shadow: Color32,
    /// The engraved title on the plate, and toolbar and panel header glyphs.
    pub title_text: Color32,
    pub icon: Color32,
    pub header_text: Color32,
}

impl Chrome {
    /// The skin's colours for a theme, with a world skin's paint over them (the C#
    /// `ThemeService.Apply` with `DefaultSkin`, `FleetSkin` and `ThemeSkinSurfaces`).
    pub fn new(theme: &Theme, id: SkinId, world: Option<&SkinPaint>) -> Chrome {
        let skin = WindowSkin::of(id);
        let panel = theme.panel;
        let text = theme.text;
        let dark = !is_light(panel);
        let toward = |a: f32| mix(panel, text, a);
        let away = if is_light(text) { Color32::BLACK } else { Color32::WHITE };
        let lit = |a: f32| mix(panel, away, a);
        let outline = mix(panel, Color32::BLACK, if dark { 0.55 } else { 0.58 });
        let reference = theme.reference && world.is_none();
        let plate = mix(rgb(0x20292D), panel, 0.10);

        // DefaultSkin.For, or FleetSkin.Create and FleetSkin.Apply for the reference palette.
        let (mut band, mut toolbar, mut header, mut footer, mut ground) = if reference {
            (
                Gradient::of(&[
                    (0xFAFCFA, 0.0),
                    (0xE8EAE8, 0.06),
                    (0xD5D8D6, 0.36),
                    (0xBEC3C1, 0.90),
                    (0xA3AAA7, 0.97),
                    (0x616B69, 1.0),
                ]),
                Gradient::of(&[
                    (0xF4F5F3, 0.0),
                    (0xDFE1DF, 0.05),
                    (0xD5D8D6, 0.38),
                    (0xC3C7C5, 0.95),
                    (0x6D7775, 1.0),
                ]),
                Gradient::of(&[(0xE5E8E5, 0.0), (0xD8DCDA, 0.18), (0xC7CECA, 1.0)]),
                Gradient::two(rgb(0xDDDED8), rgb(0xBFC2BD)),
                rgb(0x424743),
            )
        } else {
            (
                Gradient::raised(if dark { toward(0.10) } else { lit(0.12) }, lit(0.04), 0.18),
                Gradient::raised(toward(0.04), if dark { lit(0.06) } else { toward(0.08) }, 0.15),
                Gradient::raised(toward(0.07), toward(0.02), 0.15),
                Gradient::raised(panel, lit(0.04), 0.12),
                outline,
            )
        };
        let mut plaque_fill = if reference { rgb(0x20292D) } else { plate };
        let mut plaque_edge = if reference { rgb(0x424743) } else { rgb(0x10191D) };
        let mut plaque_accent = if reference {
            rgb(0x8DDEE5)
        } else {
            readable(theme.accent, plate)
        };
        let mut wing_fill = if reference { rgb(0xD1D3D1) } else { lit(0.10) };
        let mut wing_edge = if reference { rgb(0x454C50) } else { outline };
        let mut edge = if reference { rgb(0xD0D0CA) } else { panel };
        let mut edge_outline = if reference { rgb(0x424743) } else { outline };
        let mut edge_accent = if reference {
            rgb(0x8DDEE5)
        } else {
            readable(theme.accent, panel)
        };

        // A world's paint over the client's (DefaultSkin.Merge): surfaces, the plate, the edge.
        if let Some(paint) = world {
            let surface = |s: &Option<wandur_core::directory::world_theme::SkinSurface>| {
                s.as_ref()
                    .and_then(|s| Some(Gradient::two(parse_hex(&s.from)?, parse_hex(&s.to)?)))
            };
            let s = &paint.surfaces;
            band = surface(&s.title_bar).unwrap_or(band);
            toolbar = surface(&s.toolbar).unwrap_or(toolbar);
            header = surface(&s.panel_header).unwrap_or(header);
            footer = surface(&s.footer).unwrap_or(footer);
            ground = surface(&s.ground).map_or(ground, |g| g.middle());
            if let Some(p) = &paint.plaque {
                let pick = |v: &Option<String>, d: Color32| v.as_deref().and_then(parse_hex).unwrap_or(d);
                plaque_fill = pick(&p.fill, plaque_fill);
                plaque_edge = pick(&p.edge, plaque_edge);
                plaque_accent = pick(&p.accent, plaque_accent);
                wing_fill = pick(&p.wing_fill, wing_fill);
                wing_edge = pick(&p.wing_edge, wing_edge);
            }
            if let Some(e) = &paint.edge {
                edge = parse_hex(&e.color).unwrap_or(edge);
                edge_outline = e.outline.as_deref().and_then(parse_hex).unwrap_or(edge_outline);
                edge_accent = e.accent.as_deref().and_then(parse_hex).unwrap_or(edge_accent);
            }
        }

        // FleetSkin.SynchronizeMaterials.
        let light = is_light(panel);
        let rim_highlight = mix(panel, Color32::WHITE, if light { 0.45 } else { 0.15 });
        let rim_shadow = mix(panel, Color32::BLACK, 0.72);
        let plaque = if reference {
            Gradient::of(&[(0x48575D, 0.0), (0x2E3A40, 0.12), (0x253138, 0.92), (0x172229, 1.0)])
        } else {
            shaded(plaque_fill, 0.10, -0.12)
        };
        let wing = shaded(wing_fill, 0.2, -0.12);
        if id == SkinId::Armored {
            // Painted armour has broad, quiet faces; depth belongs to its bevels and joints.
            let metal = if reference { rgb(0xD9DDE0) } else { band.middle() };
            band = matte(metal);
            toolbar = matte(if reference { rgb(0xD1D6DA) } else { toolbar.middle() });
            footer = toolbar.clone();
        }
        if id == SkinId::Fleet {
            // Flat headers (C# `ui/flat-headers`): one colour, the middle of the shading the
            // header had, or a custom theme's chrome colour; the band, plate and toolbar keep
            // their metal.
            let face = if theme.chrome != theme.panel {
                theme.chrome
            } else {
                header.middle()
            };
            header = Gradient::flat(face);
        }
        if id == SkinId::System {
            // Native chrome owns the outside: flat surfaces in the theme's chrome colour, and the
            // gaps between panels take the panels' own colour.
            let chrome = theme.chrome;
            band = Gradient::flat(chrome);
            toolbar = Gradient::flat(chrome);
            header = Gradient::flat(chrome);
            footer = Gradient::flat(theme.shell);
            if world.is_none_or(|w| w.surfaces.ground.is_none()) {
                ground = panel;
            }
        }
        let header_text = text;
        Chrome {
            skin,
            band,
            toolbar,
            header,
            footer,
            ground,
            title_text: mix(plaque.middle(), Color32::WHITE, 0.82),
            plaque,
            plaque_edge,
            plaque_accent,
            wing,
            wing_edge,
            edge,
            edge_outline,
            edge_accent,
            rim_edge: edge_outline,
            rim_highlight,
            rim_shadow,
            icon: text,
            header_text,
        }
    }

    /// Text that has to read on a surface: the theme's text, or the plate's engraving.
    pub fn text_pairs(&self, theme: &Theme) -> Vec<(&'static str, Color32, Color32)> {
        let mut pairs = vec![
            ("toolbar", self.icon, self.toolbar.middle()),
            ("panel header", self.header_text, self.header.middle()),
            ("status bar", theme.text, self.footer.middle()),
        ];
        if self.skin.custom_chrome() {
            pairs.push(("title plate", self.title_text, self.plaque.middle()));
            pairs.push(("title band buttons", self.icon, self.band.middle()));
        }
        pairs
    }
}

/// The accent lifted until it reads on the plate (the C# `DefaultSkin.Readable`).
fn readable(accent: Color32, plate: Color32) -> Color32 {
    let mut lifted = accent;
    for _ in 0..12 {
        if contrast(lifted, plate) >= 3.5 {
            break;
        }
        lifted = mix(lifted, Color32::WHITE, 0.18);
    }
    lifted
}

/// A flat colour given a top-lit face (the C# `ThemePlaque.Shaded`).
fn shaded(c: Color32, lift: f32, drop: f32) -> Gradient {
    Gradient(vec![(0.0, shift(c, lift)), (0.18, c), (1.0, shift(c, drop))])
}

/// Armored's quiet metal faces (the C# `FleetSkin.Matte`).
fn matte(c: Color32) -> Gradient {
    Gradient(vec![
        (0.0, mix(c, Color32::WHITE, 0.045)),
        (0.55, c),
        (1.0, mix(c, Color32::BLACK, 0.025)),
    ])
}

// ---------------------------------------------------------------------------------------------
// Painting helpers

/// Shapes collected for baking: the skin's painters draw here, the app bakes the result into
/// meshes once ([`bake`]) and reuses them every frame until the window, the plate or the
/// colours change.
#[derive(Default)]
pub struct Canvas {
    pub shapes: Vec<Shape>,
}

impl Canvas {
    pub fn add(&mut self, shape: impl Into<Shape>) {
        self.shapes.push(shape.into());
    }

    pub fn line_segment(&mut self, points: [Pos2; 2], stroke: Stroke) {
        self.add(Shape::line_segment(points, stroke));
    }

    pub fn rect_filled(&mut self, rect: Rect, radius: impl Into<CornerRadius>, color: Color32) {
        self.add(Shape::rect_filled(rect, radius, color));
    }

    pub fn rect_stroke(&mut self, rect: Rect, radius: impl Into<CornerRadius>, stroke: Stroke, kind: StrokeKind) {
        self.add(Shape::rect_stroke(rect, radius, stroke, kind));
    }

    pub fn rect(
        &mut self,
        rect: Rect,
        radius: impl Into<CornerRadius>,
        fill: Color32,
        stroke: Stroke,
        kind: StrokeKind,
    ) {
        self.add(RectShape::new(rect, radius, fill, stroke, kind));
    }

    pub fn circle_filled(&mut self, center: Pos2, radius: f32, color: Color32) {
        self.add(Shape::circle_filled(center, radius, color));
    }
}

/// Tessellate a canvas into as few meshes as its textures allow (the untextured shapes become
/// one mesh; each wear pass keeps its own), ready to be added to a painter frame after frame.
pub fn bake(ctx: &egui::Context, canvas: Canvas) -> Vec<Shape> {
    let options: TessellationOptions = ctx.tessellation_options(|o| *o);
    let size = ctx.fonts(|f| f.font_image_size());
    let mut tessellator = Tessellator::new(ctx.pixels_per_point(), options, size, Vec::new());
    let mut out = Vec::new();
    let mut current = egui::epaint::Mesh::default();
    for shape in canvas.shapes {
        match shape {
            Shape::Mesh(mesh) if mesh.texture_id != TextureId::default() => {
                if !current.is_empty() {
                    out.push(Shape::mesh(std::mem::take(&mut current)));
                }
                out.push(Shape::Mesh(mesh));
            }
            other => tessellator.tessellate_shape(other, &mut current),
        }
    }
    if !current.is_empty() {
        out.push(Shape::mesh(current));
    }
    out
}

/// One baked layer of the chrome and what it was made for.
#[derive(Default)]
pub struct Baked {
    key: Option<u64>,
    shapes: Vec<Shape>,
}

impl Baked {
    /// The shapes for `key`, built and baked again only when the key changes.
    pub fn get(&mut self, ctx: &egui::Context, key: u64, build: impl FnOnce(&mut Canvas)) -> &[Shape] {
        if self.key != Some(key) {
            let mut canvas = Canvas::default();
            build(&mut canvas);
            self.shapes = bake(ctx, canvas);
            self.key = Some(key);
        }
        &self.shapes
    }
}

/// A key for [`Baked`]: rectangles and numbers by their bits, plus the pixel density.
pub fn bake_key(ctx: &egui::Context, numbers: &[f32], extra: u64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for n in numbers {
        n.to_bits().hash(&mut h);
    }
    ctx.pixels_per_point().to_bits().hash(&mut h);
    extra.hash(&mut h);
    h.finish()
}

/// Paint a gradient straight onto a painter (surfaces drawn once per frame, not baked).
pub fn gradient_on(painter: &Painter, rect: Rect, gradient: &Gradient) {
    let mut canvas = Canvas::default();
    paint_gradient(&mut canvas, rect, gradient);
    painter.extend(canvas.shapes);
}

/// Fill `rect` with a vertical gradient.
pub fn paint_gradient(painter: &mut Canvas, rect: Rect, gradient: &Gradient) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    let stops = &gradient.0;
    if stops.len() < 2 || stops.iter().all(|s| s.1 == stops[0].1) {
        painter.rect_filled(rect, 0.0, stops.first().map_or(Color32::TRANSPARENT, |s| s.1));
        return;
    }
    let mut mesh = Mesh::default();
    for (i, &(offset, color)) in stops.iter().enumerate() {
        let y = rect.top() + rect.height() * offset.clamp(0.0, 1.0);
        mesh.colored_vertex(pos2(rect.left(), y), color);
        mesh.colored_vertex(pos2(rect.right(), y), color);
        if i > 0 {
            let base = (i as u32 - 1) * 2;
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base + 1, base + 3, base + 2);
        }
    }
    painter.add(Shape::mesh(mesh));
}

/// Triangulate a simple polygon (convex or not) by ear clipping. Returns index triples.
pub fn triangulate(points: &[Pos2]) -> Vec<[usize; 3]> {
    let n = points.len();
    if n < 3 {
        return Vec::new();
    }
    let area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    let ccw = area > 0.0;
    let cross = |a: Pos2, b: Pos2, c: Pos2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let convex = |a: Pos2, b: Pos2, c: Pos2| {
        let z = cross(a, b, c);
        if ccw { z > 0.0 } else { z < 0.0 }
    };
    let inside = |p: Pos2, a: Pos2, b: Pos2, c: Pos2| {
        let (d1, d2, d3) = (cross(a, b, p), cross(b, c, p), cross(c, a, p));
        let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        !(neg && pos)
    };
    let mut remaining: Vec<usize> = (0..n).collect();
    let mut triangles = Vec::with_capacity(n - 2);
    let mut guard = 0;
    while remaining.len() > 3 && guard < n * n {
        guard += 1;
        let m = remaining.len();
        let mut clipped = false;
        for i in 0..m {
            let (ia, ib, ic) = (remaining[(i + m - 1) % m], remaining[i], remaining[(i + 1) % m]);
            let (a, b, c) = (points[ia], points[ib], points[ic]);
            if !convex(a, b, c) {
                continue;
            }
            let ear = remaining
                .iter()
                .all(|&j| j == ia || j == ib || j == ic || !inside(points[j], a, b, c));
            if ear {
                triangles.push([ia, ib, ic]);
                remaining.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            // Degenerate input (collinear points): fan out what is left.
            break;
        }
    }
    for i in 1..remaining.len().saturating_sub(1) {
        triangles.push([remaining[0], remaining[i], remaining[i + 1]]);
    }
    triangles
}

/// Fill a polygon with a vertical gradient over its bounds.
pub fn fill_polygon(painter: &mut Canvas, points: &[Pos2], fill: &Gradient) {
    let bounds = Rect::from_points(points);
    let mut mesh = Mesh::default();
    for p in points {
        let y = if bounds.height() > 0.0 {
            (p.y - bounds.top()) / bounds.height()
        } else {
            0.0
        };
        mesh.colored_vertex(*p, fill.at(y));
    }
    for [a, b, c] in triangulate(points) {
        mesh.add_triangle(a as u32, b as u32, c as u32);
    }
    painter.add(Shape::mesh(mesh));
}

fn polygon(painter: &mut Canvas, points: &[Pos2], fill: &Gradient, stroke: Stroke) {
    fill_polygon(painter, points, fill);
    if stroke.width > 0.0 {
        painter.add(Shape::closed_line(points.to_vec(), stroke));
    }
}

fn line(painter: &mut Canvas, points: &[Pos2], stroke: Stroke) {
    painter.add(Shape::line(points.to_vec(), stroke));
}

fn alpha(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(a.clamp(0.0, 1.0))
}

fn chamfer(r: Rect, cap: f32) -> Vec<Pos2> {
    vec![
        pos2(r.left() + cap, r.top()),
        pos2(r.right() - cap, r.top()),
        pos2(r.right(), r.top() + cap),
        pos2(r.right(), r.bottom() - cap),
        pos2(r.right() - cap, r.bottom()),
        pos2(r.left() + cap, r.bottom()),
        pos2(r.left(), r.bottom() - cap),
        pos2(r.left(), r.top() + cap),
    ]
}

fn mirror(points: &[Pos2], axis_sum: f32) -> Vec<Pos2> {
    points.iter().rev().map(|p| pos2(axis_sum - p.x, p.y)).collect()
}

fn translate(points: &[Pos2], by: egui::Vec2) -> Vec<Pos2> {
    points.iter().map(|p| *p + by).collect()
}

// ---------------------------------------------------------------------------------------------
// Fleet

/// Fleet's band: metal across the top with rails meeting the plate's shoulders.
pub fn paint_fleet_band(painter: &mut Canvas, window: Rect, title: Rect, chrome: &Chrome, mac: bool) {
    let band = chrome.skin.title_height();
    let w = window.width();
    let o = window.min.to_vec2();
    let rect = Rect::from_min_size(window.min, vec2(w, band));
    paint_gradient(painter, rect, &chrome.band);
    if w <= 12.0 {
        return;
    }
    let dark = Stroke::new(1.0, chrome.rim_edge);
    let lip = Stroke::new(1.0, chrome.rim_highlight);
    if !mac {
        // Without native traffic lights, the band is a capped plate of its own.
        let cap = Rect::from_min_size(pos2(5.5, 2.5) + o, vec2(w - 11.0, band - 3.0));
        painter.rect_stroke(cap, 4.0, dark, StrokeKind::Middle);
        painter.rect_stroke(cap.shrink(1.0), 3.0, lip, StrokeKind::Middle);
    }
    let rail_y = band - 0.5;
    let left_end = (title.left() - o.x + 6.0).clamp(6.0, w - 6.0);
    let right_start = (title.right() - o.x - 6.0).clamp(left_end, w - 6.0);
    for (from, to) in [(6.0, left_end), (right_start, w - 6.0)] {
        painter.line_segment([pos2(from, rail_y) + o, pos2(to, rail_y) + o], dark);
        painter.line_segment([pos2(from, rail_y + 1.0) + o, pos2(to, rail_y + 1.0) + o], lip);
    }
}

/// Fleet's device edge: a bevelled metal band down both sides and along the bottom.
pub fn paint_fleet_edges(painter: &mut Canvas, window: Rect, top: f32, chrome: &Chrome) {
    let t = chrome.skin.frame[0];
    let (w, h) = (window.width(), window.height());
    if t <= 0.0 || w < t * 2.0 || h <= top + t {
        return;
    }
    let o = window.min.to_vec2();
    let r = |x: f32, y: f32, rw: f32, rh: f32| Rect::from_min_size(pos2(x, y) + o, vec2(rw, rh));
    let mut fill = |rect: Rect, c: Color32| painter.rect_filled(rect, 0.0, c);
    for rect in [r(0.0, top, t, h - top), r(w - t, top, t, h - top), r(0.0, h - t, w, t)] {
        fill(rect, chrome.edge);
    }
    let dark = chrome.edge_outline;
    let lip = chrome.rim_highlight;
    fill(r(0.0, top, 1.0, h - top), dark);
    fill(r(w - 1.0, top, 1.0, h - top), dark);
    fill(r(0.0, h - 1.0, w, 1.0), dark);
    fill(r(1.0, top, 1.0, h - top - 1.0), lip);
    fill(r(w - 2.0, top, 1.0, h - top - 1.0), lip);
    fill(r(1.0, h - 2.0, w - 2.0, 1.0), lip);
    fill(r(t - 1.0, top, 1.0, h - top - t), dark);
    fill(r(w - t, top, 1.0, h - top - t), dark);
    fill(r(t - 1.0, h - t, w - t * 2.0 + 2.0, 1.0), dark);
    if t >= 4.0 {
        fill(r(2.0, top, 1.5, h - top - t), chrome.edge_accent);
        fill(r(w - 3.5, top, 1.5, h - top - t), chrome.edge_accent);
    }
}

/// The plate inside a Fleet plaque, where the icon and title sit.
pub fn fleet_plate(rect: Rect) -> Rect {
    let inset = 7.0f32.min((rect.height() * 0.12).max(0.0));
    Rect::from_min_size(
        rect.min + vec2(44.0, inset),
        vec2((rect.width() - 88.0).max(0.0), (rect.height() - inset * 2.0).max(0.0)),
    )
}

/// Fleet's nameplate: a light bracket with cut shoulders and a recessed dark plate with two
/// accent lamps (the C# `ThemePlaque.DrawFleet`).
pub fn paint_fleet_plaque(painter: &mut Canvas, rect: Rect, chrome: &Chrome) {
    let (w, h) = (rect.width(), rect.height());
    if w < 168.0 || h < 24.0 {
        return;
    }
    let o = rect.min.to_vec2();
    let outline: Vec<Pos2> = [
        (28.0, 2.0),
        (w - 28.0, 2.0),
        (w - 0.5, h * 0.60),
        (w - 12.0, h - 2.0),
        (12.0, h - 2.0),
        (0.5, h * 0.60),
    ]
    .iter()
    .map(|&(x, y)| pos2(x, y) + o)
    .collect();
    polygon(
        painter,
        &translate(&outline, vec2(0.0, 2.0)),
        &Gradient::flat(chrome.rim_shadow),
        Stroke::new(1.0, rgb(0x303B3D)),
    );
    polygon(painter, &outline, &chrome.wing, Stroke::new(1.0, chrome.wing_edge));
    let lip = Stroke::new(1.0, chrome.rim_highlight);
    let p = |x: f32, y: f32| pos2(x, y) + o;
    painter.line_segment([p(28.0, 3.0), p(w - 28.0, 3.0)], lip);
    painter.line_segment([p(28.0, 3.0), p(1.5, h * 0.60)], lip);
    painter.line_segment([p(w - 28.0, 3.0), p(w - 1.5, h * 0.60)], lip);
    painter.line_segment(
        [p(12.0, h - 3.0), p(w - 12.0, h - 3.0)],
        Stroke::new(1.0, chrome.rim_shadow),
    );
    let plate = fleet_plate(rect);
    paint_gradient(painter, plate, &chrome.plaque);
    painter.rect_stroke(plate, 2.0, lip, StrokeKind::Middle);
    painter.rect_stroke(
        plate.shrink(1.0),
        2.0,
        Stroke::new(1.0, chrome.plaque_edge),
        StrokeKind::Middle,
    );
    painter.line_segment(
        [plate.left_top() + vec2(2.0, 2.0), plate.right_top() + vec2(-2.0, 2.0)],
        Stroke::new(1.0, rgb(0x101B20)),
    );
    painter.line_segment(
        [
            plate.left_bottom() + vec2(2.0, -1.0),
            plate.right_bottom() + vec2(-2.0, -1.0),
        ],
        Stroke::new(1.0, rgb(0x8A9698)),
    );
    let lamp_inset = 6.0f32.min(plate.height() * 0.18);
    for x in [plate.left() + 10.0, plate.right() - 13.0] {
        let lamp = Rect::from_min_size(
            pos2(x, plate.top() + lamp_inset),
            vec2(3.0, plate.height() - lamp_inset * 2.0),
        );
        painter.rect_filled(lamp.expand(2.0), 3.0, alpha(chrome.plaque_accent, 0.16));
        painter.rect_filled(lamp, 1.5, chrome.plaque_accent);
    }
}

/// Fleet's toolbar row: its rim dips around the plate's projection, which reads as the receiving
/// half of the title joint (the C# `FleetToolbarSurface`). `title` is in window coordinates.
pub fn paint_fleet_toolbar(painter: &mut Canvas, rect: Rect, title: Rect, chrome: &Chrome) {
    if rect.width() < 2.0 || rect.height() < 2.0 {
        return;
    }
    let (w, h) = (rect.width(), rect.height());
    let o = rect.min.to_vec2();
    let title = title.translate(-o);
    let top = 4.0f32.min(h / 3.0);
    let notch = (title.bottom() + 5.0).clamp(top, top.max(h - 2.0));
    let left = (title.left() - 12.0).clamp(0.0, w);
    let right = (title.right() + 12.0).clamp(left, w);
    let rim: Vec<Pos2> = if title.width() >= 176.0 && title.bottom() > 0.0 && right - left >= 42.0 {
        vec![
            pos2(0.0, top),
            pos2(left, top),
            pos2(left + 21.0, notch),
            pos2(right - 21.0, notch),
            pos2(right, top),
            pos2(w, top),
        ]
    } else {
        vec![pos2(0.0, top), pos2(w, top)]
    };
    painter.rect_filled(rect, 0.0, chrome.rim_shadow);
    let mut face = rim.clone();
    face.push(pos2(w, h));
    face.push(pos2(0.0, h));
    let face = translate(&face, o);
    // The gradient runs over the whole row, not the face's bounds.
    let mut mesh = Mesh::default();
    for p in &face {
        mesh.colored_vertex(*p, chrome.toolbar.at((p.y - rect.top()) / h));
    }
    for [a, b, c] in triangulate(&face) {
        mesh.add_triangle(a as u32, b as u32, c as u32);
    }
    painter.add(Shape::mesh(mesh));
    let rim = translate(&rim, o);
    line(
        painter,
        &rim,
        Stroke::new(5.0, Color32::from_rgba_unmultiplied(0x3F, 0x4B, 0x4D, 0x45)),
    );
    line(painter, &rim, Stroke::new(1.0, chrome.rim_edge));
    line(
        painter,
        &translate(&rim, vec2(0.0, 1.5)),
        Stroke::new(1.0, chrome.rim_highlight),
    );
}

// ---------------------------------------------------------------------------------------------
// Armored

const SHADE: Color32 = Color32::from_rgba_premultiplied(0x01, 0x05, 0x06, 0x48);
const RECESS: Color32 = Color32::from_rgba_premultiplied(0x0A, 0x11, 0x17, 0xE0);
const REFLECTION: Color32 = Color32::from_rgba_premultiplied(0x65, 0x65, 0x65, 0x65);

/// The owner's wear texture for this context, tiled at 384 points over Armored's plates (loaded
/// on first use).
pub fn wear_texture(ctx: &egui::Context) -> Option<TextureHandle> {
    let id = egui::Id::new("armored-wear-texture");
    if let Some(handle) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return Some(handle);
    }
    let bytes: &[u8] = include_bytes!("../assets/skins/armored-wear-384.png");
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
    let rgba = image.to_rgba8();
    // Only the alpha is used: white with the mask's alpha, tinted per pass.
    let pixels: Vec<Color32> = rgba.pixels().map(|p| Color32::from_white_alpha(p.0[3])).collect();
    let color = egui::ColorImage::new([rgba.width() as usize, rgba.height() as usize], pixels);
    let options = egui::TextureOptions {
        wrap_mode: egui::TextureWrapMode::Repeat,
        ..egui::TextureOptions::LINEAR
    };
    let handle = ctx.load_texture("armored-wear", color, options);
    ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
    Some(handle)
}

/// The wear, clipped to a plate's outline: a light lip offset by half a point, then the dark
/// scratch (the C# `ArmoredWear.Draw`).
fn wear(painter: &mut Canvas, outline: &[Pos2], wear: Option<&TextureHandle>) {
    let Some(texture) = wear else { return };
    let triangles = triangulate(outline);
    for (offset, tint) in [
        (vec2(0.5, 0.75), Color32::from_white_alpha(71)),
        (vec2(0.0, 0.0), Color32::from_black_alpha(82)),
    ] {
        let mut mesh = Mesh::with_texture(texture.id());
        for p in outline {
            let uv = (*p - offset) / 384.0;
            mesh.vertices.push(Vertex {
                pos: *p,
                uv: pos2(uv.x, uv.y),
                color: tint,
            });
        }
        for [a, b, c] in &triangles {
            mesh.add_triangle(*a as u32, *b as u32, *c as u32);
        }
        painter.add(Shape::mesh(mesh));
    }
}

fn fastener(painter: &mut Canvas, p: Pos2, edge: Color32, highlight: Color32) {
    painter.circle_filled(p, 1.3, edge);
    painter.line_segment([p + vec2(-0.7, 0.6), p + vec2(0.7, 0.6)], Stroke::new(0.6, highlight));
}

/// A chamfered plate with its bevels (the C# `Plate`).
fn plate(
    painter: &mut Canvas,
    r: Rect,
    cap: f32,
    metal: &Gradient,
    edge: Color32,
    highlight: Color32,
    w: Option<&TextureHandle>,
) {
    let shape = chamfer(r, cap);
    polygon(painter, &shape, metal, Stroke::new(1.0, edge));
    wear(painter, &shape, w);
    if r.width() < 5.0 || r.height() < 5.0 {
        return;
    }
    let bevel = 3.0f32.min(r.width().min(r.height()) / 4.0);
    fill_polygon(
        painter,
        &[
            r.left_top(),
            r.right_top(),
            r.right_top() + vec2(-bevel, bevel),
            r.left_top() + vec2(bevel, bevel),
        ],
        &Gradient::flat(alpha(highlight, 0.5)),
    );
    fill_polygon(
        painter,
        &[
            r.left_bottom(),
            r.left_bottom() + vec2(bevel, -bevel),
            r.right_bottom() + vec2(-bevel, -bevel),
            r.right_bottom(),
        ],
        &Gradient::flat(SHADE),
    );
    painter.add(Shape::closed_line(
        chamfer(r.shrink(1.2), (cap - 1.0).max(1.0)),
        Stroke::new(0.8, highlight),
    ));
    painter.line_segment(
        [
            pos2(r.left() + cap, r.top() + 2.0),
            pos2(r.right() - cap, r.top() + 2.0),
        ],
        Stroke::new(1.2, highlight),
    );
    painter.line_segment(
        [
            pos2(r.left() + cap, r.bottom() - 2.0),
            pos2(r.right() - cap, r.bottom() - 2.0),
        ],
        Stroke::new(2.0, SHADE),
    );
}

/// A plate cut to any outline, lit from the upper left whichever way it faces.
fn sculpted(
    painter: &mut Canvas,
    outline: &[Pos2],
    depth: f32,
    metal: &Gradient,
    edge: Color32,
    highlight: Color32,
    w: Option<&TextureHandle>,
) {
    polygon(painter, outline, metal, Stroke::new(1.0, edge));
    wear(painter, outline, w);
    // Each edge gets an inward strip, light where it faces up and left, dark otherwise. The
    // direction of "inward" depends on the outline's winding.
    let area: f32 = (0..outline.len())
        .map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    let sign = if area > 0.0 { 1.0 } else { -1.0 };
    for i in 0..outline.len() {
        let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
        let d = b - a;
        let length = d.length();
        if length < 0.01 {
            continue;
        }
        let inward = vec2(-d.y, d.x) / length * depth * sign;
        let light = (-0.6 * d.y + 0.8 * d.x) / length * sign;
        let color = if light > 0.0 {
            alpha(highlight, light.abs() * 0.75)
        } else {
            alpha(RECESS, light.abs() * 0.48)
        };
        fill_polygon(painter, &[a, b, b + inward, a + inward], &Gradient::flat(color));
    }
}

/// Armored's band, side rails and foot (the C# `ArmoredSkinRenderer.DrawFrame`). `title` is
/// the plate in window coordinates; `exclusion` the caption areas at the left and right.
pub fn paint_armored_frame(
    painter: &mut Canvas,
    window: Rect,
    title: Rect,
    exclusion: (f32, f32),
    chrome: &Chrome,
    w_tex: Option<&TextureHandle>,
) {
    let (w, h) = (window.width(), window.height());
    let band = chrome.skin.title_height();
    let rail = ARMORED_RAIL_WIDTH;
    let foot = ARMORED_FOOT_HEIGHT;
    if w < 32.0 || h < band + foot + 54.0 {
        return;
    }
    let o = window.min.to_vec2();
    let p = |x: f32, y: f32| pos2(x, y) + o;
    let r = |x: f32, y: f32, rw: f32, rh: f32| Rect::from_min_size(p(x, y), vec2(rw, rh));
    let metal = &chrome.band;
    let edge = chrome.rim_edge;
    let highlight = chrome.rim_highlight;
    let accent = chrome.edge_accent;
    let title = title.translate(-o);
    let band_rect = r(0.5, 0.5, w - 1.0, band - 1.0);
    paint_gradient(painter, band_rect, metal);
    painter.rect_stroke(band_rect, 3.0, Stroke::new(1.0, edge), StrokeKind::Middle);
    let left_end = (title.left() + 8.0).clamp(6.0, w - 6.0);
    let right_start = (title.right() - 8.0).clamp(left_end, w - 6.0);
    for panel in [
        Rect::from_min_size(pos2(3.0, 4.0), vec2((left_end - 3.0).max(0.0), band - 8.0)),
        Rect::from_min_size(
            pos2(right_start, 4.0),
            vec2((w - right_start - 3.0).max(0.0), band - 8.0),
        ),
    ] {
        if panel.width() < 24.0 {
            continue;
        }
        let left = panel.left() < w / 2.0;
        let mut wing = vec![
            pos2(panel.left() + 7.0, panel.top()),
            pos2(panel.right() - 26.0, panel.top()),
            pos2(panel.right() - 16.0, panel.top() + 11.0),
            pos2(panel.right(), panel.top() + 11.0),
            pos2(panel.right(), panel.bottom() - 11.0),
            pos2(panel.right() - 12.0, panel.bottom()),
            pos2(panel.left() + 7.0, panel.bottom()),
            pos2(panel.left(), panel.bottom() - 7.0),
            pos2(panel.left(), panel.top() + 7.0),
        ];
        if !left {
            wing = mirror(&wing, panel.left() + panel.right());
        }
        sculpted(painter, &translate(&wing, o), 8.0, metal, edge, highlight, w_tex);
        let y = panel.bottom() - 14.0;
        let mut joint = vec![
            pos2(panel.left() + 4.0, y),
            pos2(panel.right() - 48.0, y),
            pos2(panel.right() - 40.0, y + 5.0),
            pos2(panel.right() - 5.0, y + 5.0),
        ];
        if !left {
            joint = mirror(&joint, panel.left() + panel.right());
        }
        let joint = translate(&joint, o);
        line(painter, &joint, Stroke::new(2.5, RECESS));
        line(painter, &translate(&joint, vec2(0.0, 2.0)), Stroke::new(1.0, highlight));
        if panel.width() > 240.0 {
            let seam_x = if left {
                panel.right() - 100.0
            } else {
                panel.left() + 100.0
            };
            if seam_x - 12.0 >= exclusion.0 + 4.0 && seam_x + 12.0 <= w - exclusion.1 - 4.0 {
                let dir = if left { 1.0 } else { -1.0 };
                let seam = translate(
                    &[
                        pos2(seam_x, panel.top() + 9.0),
                        pos2(seam_x, y - 18.0),
                        pos2(seam_x + dir * 9.0, y - 10.0),
                        pos2(seam_x + dir * 9.0, y),
                    ],
                    o,
                );
                line(painter, &seam, Stroke::new(2.0, RECESS));
                line(painter, &translate(&seam, vec2(1.5, 0.0)), Stroke::new(0.8, highlight));
                fastener(painter, p(seam_x - dir * 6.0, y - 6.0), edge, highlight);
            }
        }
        fastener(painter, p(panel.left() + 7.0, panel.bottom() - 5.0), edge, highlight);
        fastener(painter, p(panel.right() - 7.0, panel.bottom() - 5.0), edge, highlight);
    }

    for x in [0.0, w - rail] {
        let side = r(x, band - 3.0, rail, h - band - foot + 6.0);
        painter.rect_filled(side, 0.0, RECESS);
        plate(
            painter,
            r(
                if x == 0.0 { 1.0 } else { w - rail + 6.0 },
                band - 3.0,
                rail - 7.0,
                side.height(),
            ),
            4.0,
            metal,
            edge,
            highlight,
            w_tex,
        );
        let lip_x = if x == 0.0 { rail - 3.0 } else { w - rail + 3.0 };
        painter.line_segment([p(lip_x, band), p(lip_x, h - foot)], Stroke::new(4.0, edge));
        let hx = lip_x + if x == 0.0 { -2.0 } else { 2.0 };
        painter.line_segment([p(hx, band), p(hx, h - foot)], Stroke::new(1.0, highlight));
        for fraction in [0.0, 0.34, 0.70] {
            let y = band + 9.0 + fraction * (h - band - foot - 54.0).max(0.0);
            let px = if x == 0.0 { 7.0 } else { w - 21.0 };
            plate(painter, r(px, y, 14.0, 42.0), 2.0, metal, edge, highlight, w_tex);
            fastener(painter, p(px + 7.0, y + 6.0), edge, highlight);
            fastener(painter, p(px + 7.0, y + 36.0), edge, highlight);
            painter.line_segment([p(px + 5.0, y + 15.0), p(px + 5.0, y + 27.0)], Stroke::new(2.0, SHADE));
            painter.line_segment(
                [p(px + 7.0, y + 15.0), p(px + 7.0, y + 27.0)],
                Stroke::new(0.8, highlight),
            );
        }
    }

    let bottom = r(0.5, h - foot, w - 1.0, foot - 0.5);
    painter.rect_filled(bottom, 0.0, RECESS);
    plate(
        painter,
        bottom.shrink2(vec2(1.0, 3.0)),
        6.0,
        metal,
        edge,
        highlight,
        w_tex,
    );
    painter.line_segment(
        [p(rail, h - foot + 2.0), p(w - rail, h - foot + 2.0)],
        Stroke::new(3.0, SHADE),
    );
    painter.line_segment(
        [p(rail, h - foot + 3.0), p(w - rail, h - foot + 3.0)],
        Stroke::new(1.0, highlight),
    );
    let end_width = 138.0f32.min((w - 24.0) / 3.0);
    for x in [5.0, w - end_width - 5.0] {
        let end = r(x, h - foot + 3.0, end_width, foot - 6.0);
        let mut corner = vec![
            pos2(2.0, h - 76.0),
            pos2(21.0, h - 76.0),
            pos2(21.0, h - 31.0),
            pos2(24.0, h - 30.0),
            pos2(28.0, h - 27.0),
            pos2(end_width + 1.0, h - 27.0),
            pos2(end_width + 8.0, h - 20.0),
            pos2(end_width + 8.0, h - 7.0),
            pos2(end_width + 2.0, h - 1.0),
            pos2(9.0, h - 1.0),
            pos2(1.0, h - 9.0),
            pos2(1.0, h - 68.0),
        ];
        if x != 5.0 {
            corner = mirror(&corner, w);
        }
        sculpted(painter, &translate(&corner, o), 4.0, metal, edge, highlight, w_tex);
        fastener(
            painter,
            p(if x == 5.0 { 14.0 } else { w - 14.0 }, h - 65.0),
            edge,
            highlight,
        );
        let vent = Rect::from_min_size(end.min + vec2(18.0, 5.0), vec2((end_width - 36.0).max(0.0), 14.0));
        if vent.width() < 4.0 {
            continue;
        }
        polygon(
            painter,
            &chamfer(vent, 2.0),
            &Gradient::flat(RECESS),
            Stroke::new(1.0, edge),
        );
        for i in 0..9 {
            let rib = vent.left() + 4.0 + i as f32 * (vent.width() - 8.0) / 9.0;
            painter.line_segment(
                [pos2(rib, vent.top() + 2.0), pos2(rib, vent.bottom() - 2.0)],
                Stroke::new(3.0, edge),
            );
            painter.line_segment(
                [pos2(rib - 1.0, vent.top() + 2.0), pos2(rib - 1.0, vent.bottom() - 2.0)],
                Stroke::new(0.8, highlight),
            );
        }
        fastener(painter, pos2(end.left() + 7.0, end.center().y), edge, highlight);
        fastener(painter, pos2(end.right() - 7.0, end.center().y), edge, highlight);
    }
    for x in [end_width + 34.0, w / 2.0 - 86.0, w / 2.0 + 86.0, w - end_width - 34.0] {
        painter.line_segment(
            [p(x - 5.0, h - foot + 4.0), p(x + 5.0, h - 4.5)],
            Stroke::new(1.0, edge),
        );
        painter.line_segment(
            [p(x - 3.0, h - foot + 4.0), p(x + 7.0, h - 4.5)],
            Stroke::new(0.8, highlight),
        );
    }
    let latch = r(w / 2.0 - 68.0, h - foot + 4.0, 136.0, foot - 8.0);
    plate(painter, latch, 5.0, metal, edge, highlight, w_tex);
    fastener(painter, pos2(latch.left() + 10.0, latch.center().y), edge, highlight);
    fastener(painter, pos2(latch.right() - 10.0, latch.center().y), edge, highlight);
    for i in -1..=1 {
        let lamp = Rect::from_center_size(
            pos2(latch.center().x + i as f32 * 10.0, latch.center().y),
            vec2(4.0, 4.0),
        );
        painter.rect(
            lamp.expand(1.5),
            0.0,
            RECESS,
            Stroke::new(0.5, edge),
            StrokeKind::Middle,
        );
        painter.rect_filled(lamp, 0.0, accent);
    }
    for x in [2.0, w - 5.0] {
        let light = r(x, band + 4.0, 3.0, (h - band - foot - 4.0).max(0.0));
        painter.rect(
            light.expand(1.0),
            1.0,
            RECESS,
            Stroke::new(0.6, edge),
            StrokeKind::Middle,
        );
        painter.rect_filled(light.expand(2.0), 2.0, alpha(accent, 0.22));
        painter.rect_filled(light, 1.0, accent);
        painter.rect_filled(
            Rect::from_min_size(light.min, vec2(0.7, light.height())),
            0.0,
            alpha(highlight, 0.65),
        );
    }
}

/// Armored's plate: shoulders with lamps on a dark spine around a recessed receiver (the C#
/// `ArmoredSkinRenderer.DrawPlaque`).
pub fn paint_armored_plaque(painter: &mut Canvas, rect: Rect, chrome: &Chrome, w_tex: Option<&TextureHandle>) {
    let (w, h) = (rect.width(), rect.height());
    if w < 208.0 || h < 40.0 {
        return;
    }
    let o = rect.min.to_vec2();
    let p = |x: f32, y: f32| pos2(x, y) + o;
    let metal = &chrome.band;
    let edge = chrome.rim_edge;
    let accent = chrome.plaque_accent;
    let outline: Vec<Pos2> = [
        (0.5, 25.0),
        (10.0, 25.0),
        (27.0, 2.0),
        (52.0, 2.0),
        (59.0, 7.0),
        (w - 59.0, 7.0),
        (w - 52.0, 2.0),
        (w - 27.0, 2.0),
        (w - 10.0, 25.0),
        (w - 0.5, 25.0),
        (w - 0.5, h - 12.0),
        (w - 12.0, h - 2.0),
        (12.0, h - 2.0),
        (0.5, h - 12.0),
    ]
    .iter()
    .map(|&(x, y)| p(x, y))
    .collect();
    polygon(
        painter,
        &translate(&outline, vec2(0.0, 3.0)),
        &Gradient::flat(RECESS),
        Stroke::new(5.0, SHADE),
    );
    polygon(painter, &outline, metal, Stroke::new(1.0, edge));
    wear(painter, &outline, w_tex);
    painter.line_segment([p(60.0, 8.0), p(w - 60.0, 8.0)], Stroke::new(1.0, REFLECTION));
    painter.line_segment([p(10.0, h - 3.0), p(w - 10.0, h - 3.0)], Stroke::new(1.0, REFLECTION));
    let receiver = Rect::from_min_size(p(70.0, 10.0), vec2(w - 140.0, h - 20.0));
    polygon(
        painter,
        &chamfer(receiver, 7.0),
        &Gradient::flat(RECESS),
        Stroke::new(1.5, edge),
    );
    painter.add(Shape::closed_line(
        chamfer(receiver.shrink(2.0), 6.0),
        Stroke::new(0.8, REFLECTION),
    ));
    painter.add(Shape::closed_line(
        chamfer(receiver.shrink(4.0), 5.0),
        Stroke::new(3.0, SHADE),
    ));
    let well = receiver.shrink(8.0);
    let mut fill = chamfer(well, 5.0);
    fill_polygon(painter, &fill, &chrome.plaque);
    fill.push(fill[0]);
    line(painter, &fill, Stroke::new(1.0, edge));
    painter.line_segment(
        [
            pos2(well.left() + 5.0, well.top() + 2.0),
            pos2(well.right() - 5.0, well.top() + 2.0),
        ],
        Stroke::new(3.0, SHADE),
    );
    painter.line_segment(
        [
            pos2(well.left() + 5.0, well.bottom() - 1.0),
            pos2(well.right() - 5.0, well.bottom() - 1.0),
        ],
        Stroke::new(0.8, REFLECTION),
    );
    for left in [true, false] {
        let socket = Rect::from_min_size(p(if left { 43.0 } else { w - 67.0 }, 14.0), vec2(24.0, h - 28.0));
        polygon(
            painter,
            &chamfer(socket, 4.0),
            &Gradient::flat(RECESS),
            Stroke::new(1.0, edge),
        );
        painter.add(Shape::closed_line(
            chamfer(socket.shrink(2.0), 3.0),
            Stroke::new(2.0, SHADE),
        ));
        let inset = 7.0f32.min(socket.height() / 4.0);
        let light = Rect::from_min_size(
            pos2(socket.center().x - 2.0, socket.top() + inset),
            vec2(4.0, socket.height() - inset * 2.0),
        );
        painter.rect_filled(light.expand(4.0), 3.0, alpha(accent, 0.12));
        painter.rect_filled(light.expand(2.0), 2.0, alpha(accent, 0.28));
        painter.rect_filled(light, 1.5, accent);
        let mut shoulder = vec![
            pos2(26.0, 3.0),
            pos2(44.0, 3.0),
            pos2(50.0, 10.0),
            pos2(50.0, h * 0.28),
            pos2(43.0, h * 0.36),
            pos2(43.0, h * 0.66),
            pos2(49.0, h * 0.74),
            pos2(49.0, h - 8.0),
            pos2(18.0, h - 8.0),
            pos2(9.0, h - 17.0),
            pos2(9.0, 27.0),
        ];
        let mut heel = vec![
            pos2(9.0, h - 30.0),
            pos2(25.0, h - 30.0),
            pos2(32.0, h - 23.0),
            pos2(49.0, h - 23.0),
            pos2(49.0, h - 7.0),
            pos2(18.0, h - 7.0),
            pos2(9.0, h - 16.0),
        ];
        if !left {
            shoulder = mirror(&shoulder, w);
            heel = mirror(&heel, w);
        }
        sculpted(
            painter,
            &translate(&shoulder, o),
            4.0,
            metal,
            edge,
            Color32::WHITE,
            w_tex,
        );
        sculpted(painter, &translate(&heel, o), 2.5, metal, edge, Color32::WHITE, w_tex);
        for y in [12.0, h - 15.0] {
            fastener(painter, p(if left { 32.0 } else { w - 32.0 }, y), edge, REFLECTION);
        }
        for y in [7.0, h - 14.0] {
            plate(
                painter,
                Rect::from_min_size(p(if left { 57.0 } else { w - 81.0 }, y), vec2(24.0, 7.0)),
                2.0,
                metal,
                edge,
                REFLECTION,
                w_tex,
            );
        }
    }
    for x in [receiver.left() + 9.0, receiver.right() - 9.0] {
        for y in [receiver.top() + 4.0, receiver.bottom() - 4.0] {
            fastener(painter, pos2(x, y), edge, REFLECTION);
        }
    }
}

/// The text well inside Armored's plate, where the icon and title sit.
pub fn armored_well(rect: Rect) -> Rect {
    Rect::from_min_size(
        rect.min + vec2(78.0, 18.0),
        vec2((rect.width() - 156.0).max(0.0), (rect.height() - 36.0).max(0.0)),
    )
}

/// Armored's toolbar row: a matte face whose seam steps down around the plate.
pub fn paint_armored_toolbar(painter: &mut Canvas, rect: Rect, title: Rect, chrome: &Chrome) {
    if rect.width() < 2.0 || rect.height() < 2.0 {
        return;
    }
    let (w, h) = (rect.width(), rect.height());
    let o = rect.min.to_vec2();
    let title = title.translate(-o);
    let left = (title.left() - 5.0).clamp(0.0, w);
    let right = (title.right() + 5.0).clamp(left, w);
    let bottom = (title.bottom() + 4.0).clamp(3.0, 3.0f32.max(h - 1.0));
    let cap = 6.0f32.min((right - left) / 2.0);
    let rim: Vec<Pos2> = if title.width() >= 208.0 && title.bottom() > 0.0 {
        vec![
            pos2(0.0, 2.0),
            pos2(left, 2.0),
            pos2(left, bottom - cap),
            pos2(left + cap, bottom),
            pos2(right - cap, bottom),
            pos2(right, bottom - cap),
            pos2(right, 2.0),
            pos2(w, 2.0),
        ]
    } else {
        vec![pos2(0.0, 2.0), pos2(w, 2.0)]
    };
    painter.rect_filled(rect, 0.0, chrome.rim_edge);
    let mut face = rim.clone();
    face.push(pos2(w, h));
    face.push(pos2(0.0, h));
    fill_polygon(painter, &translate(&face, o), &chrome.toolbar);
    let seam = translate(&rim, o);
    line(painter, &seam, Stroke::new(7.0, SHADE));
    line(painter, &seam, Stroke::new(1.0, chrome.rim_edge));
    line(
        painter,
        &translate(&seam, vec2(0.0, 2.0)),
        Stroke::new(1.0, chrome.rim_highlight),
    );
    painter.line_segment(
        [
            pos2(rect.left(), rect.bottom() - 1.0),
            pos2(rect.right(), rect.bottom() - 1.0),
        ],
        Stroke::new(1.0, chrome.rim_edge),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// TitleBarMetricsTests.OnlyTheCustomChromeSkinsHaveTitleBarMetrics.
    #[test]
    fn only_the_drawn_skins_have_title_bar_metrics() {
        assert_eq!(WindowSkin::resolve("Fleet").title_bar, Some(TitleBarMetrics::FLEET));
        assert_eq!(WindowSkin::resolve("Armored").title_bar, Some(TitleBarMetrics::ARMORED));
        let system = WindowSkin::resolve("System");
        assert!(system.title_bar.is_none() && !system.custom_chrome());
        assert_eq!(system.title_height(), 0.0);
        for metrics in [TitleBarMetrics::FLEET, TitleBarMetrics::ARMORED] {
            assert_eq!(
                metrics.plaque_height(),
                metrics.band_height - metrics.plaque_top + metrics.plaque_drop
            );
            assert!(metrics.plaque_drop > 0.0, "the plate projects below the band");
            assert!(
                metrics.toolbar_top_padding > metrics.plaque_drop,
                "the toolbar clears the plate"
            );
            assert!(
                metrics.hidden_toolbar_clearance > metrics.plaque_drop,
                "content clears the plate without a toolbar"
            );
            let top = metrics.actions_top();
            assert!((0.0..=metrics.band_height - metrics.action_button_size).contains(&top));
        }
        assert_eq!(TitleBarMetrics::FLEET.actions_center(), 19.0);
        assert_eq!(TitleBarMetrics::ARMORED.actions_center(), 23.0);
    }

    #[test]
    fn dock_headers_and_frames_follow_the_skin() {
        assert_eq!(
            SkinId::ALL.map(|id| WindowSkin::of(id).dock_header_height),
            [30.0, 30.0, 30.0]
        );
        // Fleet and System headers are flat (one colour); Armored keeps its shading.
        let theme = crate::theme::Theme::preset("Hull");
        for id in [SkinId::Fleet, SkinId::System] {
            let header = Chrome::new(&theme, id, None).header;
            assert!(header.0.iter().all(|(_, c)| *c == header.middle()), "{id:?}");
        }
        assert_eq!(Chrome::new(&theme, SkinId::System, None).header.middle(), theme.chrome);
        assert_eq!(WindowSkin::FLEET.frame, [6.0, 0.0, 6.0, 6.0]);
        assert_eq!(WindowSkin::ARMORED.frame, [24.0, 0.0, 24.0, 30.0]);
        assert_eq!(WindowSkin::SYSTEM.frame, [0.0; 4]);
        assert_eq!(WindowSkin::ARMORED.title_height(), 60.0);
    }

    #[test]
    fn unknown_skins_are_fleet() {
        for (name, id) in [
            ("Fleet", SkinId::Fleet),
            ("Armored", SkinId::Armored),
            ("System", SkinId::System),
            ("", SkinId::Fleet),
            ("Steampunk", SkinId::Fleet),
            ("system", SkinId::Fleet),
        ] {
            assert_eq!(SkinId::from_name(name), id, "{name}");
        }
        for id in SkinId::ALL {
            assert_eq!(SkinId::from_name(id.name()), id);
        }
    }

    /// The C# TitleBarLayout cases: centred on the window, clear of the wider caption area on
    /// both sides, even widths, and a plain title when the plate cannot fit its shoulders.
    #[test]
    fn the_plate_is_centred_clear_of_the_caption_and_falls_back_to_plain() {
        let m = TitleBarMetrics::FLEET;
        let place = place_title(&m, 1600.0, 88.0, 102.0, 301.0);
        // 302 (even) plus the inset on each side.
        assert_eq!(place.rect.width(), 302.0 + 168.0);
        assert_eq!(place.rect.center().x, 800.0);
        assert_eq!(place.rect.top(), m.plaque_top);
        assert_eq!(place.rect.height(), m.plaque_height());
        assert!(!place.plain);
        // A long title is held clear of the wider exclusion (102 + 12 each side).
        let wide = place_title(&m, 1000.0, 88.0, 102.0, 2000.0);
        assert_eq!(wide.rect.width(), 1000.0 - 114.0 * 2.0);
        assert_eq!(wide.rect.left(), 114.0);
        // Too narrow for the shoulders and a short title: plain.
        let narrow = place_title(&m, 420.0, 88.0, 102.0, 300.0);
        assert!(narrow.plain);
        assert!(narrow.rect.width() < m.text_inset * 2.0 + 48.0);
        // Garbage in is not a panic or a negative size.
        let odd = place_title(&m, f32::NAN, -5.0, f32::INFINITY, -1.0);
        assert!(odd.rect.width() >= 0.0 && odd.plain);
        assert_eq!(text_room(1600.0, 88.0, 102.0, m.text_inset), 1600.0 - 228.0 - 168.0);
        assert_eq!(text_room(100.0, 88.0, 102.0, m.text_inset), 0.0);
        let armored = place_title(&TitleBarMetrics::ARMORED, 1600.0, 88.0, 102.0, 300.0);
        assert_eq!(armored.rect.width(), 300.0 + 200.0);
        assert_eq!(armored.rect.height(), 66.0);
    }

    /// The plate's title rule: the full app name (with the world) when it fits, "Wandur" when not.
    #[test]
    fn the_plate_shows_the_full_name_when_it_fits_and_wandur_when_not() {
        let measure = |s: &str| s.chars().count() as f32 * 9.0;
        let (full, short) = plate_labels(None);
        assert_eq!((full.as_str(), short.as_str()), ("Wandur Mud Client", "Wandur"));
        assert_eq!(plate_title(&full, &short, 400.0, measure), "WANDUR MUD CLIENT");
        assert_eq!(plate_title(&full, &short, 100.0, measure), "WANDUR");
        let (full, short) = plate_labels(Some("The Lantern Road"));
        assert_eq!(full, "Wandur Mud Client - The Lantern Road");
        assert_eq!(short, "Wandur - The Lantern Road");
        let fits = measure(&full.to_uppercase());
        assert_eq!(
            plate_title(&full, &short, fits, measure),
            "WANDUR MUD CLIENT - THE LANTERN ROAD"
        );
        assert_eq!(
            plate_title(&full, &short, fits - 1.0, measure),
            "WANDUR - THE LANTERN ROAD"
        );
        assert_eq!(plate_labels(Some("  ")).0, APP_NAME);
        // In a real window: the room the layout leaves at 1300 and at 900 points.
        let m = TitleBarMetrics::FLEET;
        let (left, right) = caption_exclusion(true, &m);
        let room = |width: f32| text_room(width, left, right, m.text_inset) - m.logo_size - TITLE_LOGO_GAP;
        assert!(room(1300.0) > measure("WANDUR MUD CLIENT - THE LANTERN ROAD"));
        assert_eq!(
            plate_title(&full, &short, room(700.0), measure),
            "WANDUR - THE LANTERN ROAD"
        );
    }

    #[test]
    fn caption_areas_differ_by_platform() {
        let m = TitleBarMetrics::FLEET;
        assert_eq!(caption_exclusion(true, &m), (88.0, 12.0 + 90.0));
        assert_eq!(caption_exclusion(false, &m), (0.0, 156.0 + 90.0));
    }

    #[test]
    fn ear_clipping_covers_a_concave_outline() {
        // A notched toolbar face: area 100 x 20 minus the notch triangle-ish dip.
        let face = [
            pos2(0.0, 0.0),
            pos2(30.0, 0.0),
            pos2(40.0, 10.0),
            pos2(60.0, 10.0),
            pos2(70.0, 0.0),
            pos2(100.0, 0.0),
            pos2(100.0, 20.0),
            pos2(0.0, 20.0),
        ];
        let tris = triangulate(&face);
        assert_eq!(tris.len(), face.len() - 2);
        let area: f32 = tris
            .iter()
            .map(|[a, b, c]| {
                let (a, b, c) = (face[*a], face[*b], face[*c]);
                ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs() / 2.0
            })
            .sum();
        assert!((area - (2000.0 - 300.0)).abs() < 0.01, "{area}");
    }

    #[test]
    fn gradients_interpolate_between_stops() {
        let g = Gradient::two(Color32::BLACK, Color32::WHITE);
        assert_eq!(g.at(0.0), Color32::BLACK);
        assert_eq!(g.at(1.0), Color32::WHITE);
        assert_eq!(g.middle(), Color32::from_rgb(128, 128, 128));
        assert_eq!(Gradient::flat(Color32::RED).at(0.3), Color32::RED);
    }

    #[test]
    fn the_reference_palette_is_fleets_own_metal() {
        let hull = Chrome::new(&Theme::preset("Hull"), SkinId::Fleet, None);
        assert_eq!(hull.ground, rgb(0x424743));
        assert_eq!(hull.plaque_accent, rgb(0x8DDEE5));
        assert_eq!(hull.band.0.len(), 6);
        let slate = Chrome::new(&Theme::preset("Slate"), SkinId::Fleet, None);
        assert_ne!(slate.ground, hull.ground);
        // System has no drawn frame: the gaps take the panel colour.
        let system = Chrome::new(&Theme::preset("Slate"), SkinId::System, None);
        assert_eq!(system.ground, Theme::preset("Slate").panel);
        let armored = Chrome::new(&Theme::preset("Hull"), SkinId::Armored, None);
        assert_eq!(armored.band.at(0.55), rgb(0xD9DDE0));
    }

    /// ThemeContrastTests.EveryPresetKeepsItsChromeLegible, for what the skins paint: every
    /// glyph on a skin surface (toolbar, panel headers, status bar, the title plate and band)
    /// reads at 4.5:1 in every preset and skin, in a Create a copy of every preset, and in the
    /// fixture world themes.
    #[test]
    fn every_preset_keeps_its_chrome_legible_in_every_skin() {
        const FLOOR: f32 = 4.5;
        let mut themes: Vec<(String, Theme)> = Theme::names().map(|n| (n.to_string(), Theme::preset(n))).collect();
        for name in Theme::names() {
            let (colors, is_light) = crate::theme::preset_colors(name);
            let copy = wandur_core::settings::CustomTheme {
                name: format!("{name} copy 1"),
                base: name.into(),
                colors,
                is_light,
                ..Default::default()
            };
            assert!(copy.is_valid(), "{name}");
            themes.push((copy.name.clone(), Theme::from_custom(&copy)));
        }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../wandur-core/tests/fixtures/world-theme");
        for file in [
            "world-theme.json",
            "world-theme-icesus.json",
            "world-theme-industrial-skin.json",
            "world-theme-metallic.json",
        ] {
            let value: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(dir.join(file)).unwrap()).unwrap();
            let world = wandur_core::directory::WorldTheme::parse(&value).unwrap();
            themes.push((file.into(), Theme::from_world(&world, "Hull")));
        }
        let mut failures = Vec::new();
        for (name, theme) in &themes {
            for id in SkinId::ALL {
                let chrome = Chrome::new(theme, id, None);
                for (surface, ink, under) in chrome.text_pairs(theme) {
                    let ratio = contrast(ink, under);
                    if ratio < FLOOR {
                        failures.push(format!("{name} {} {surface}: {ratio:.2}", id.name()));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "unreadable chrome:\n{}", failures.join("\n"));
    }

    #[test]
    fn a_world_skin_repaints_surfaces_but_not_geometry() {
        use wandur_core::directory::world_theme::{PlaquePaint, SkinSurface, SkinSurfaces};
        let paint = SkinPaint {
            surfaces: SkinSurfaces {
                title_bar: Some(SkinSurface {
                    from: "#112233".into(),
                    to: "#445566".into(),
                }),
                ground: Some(SkinSurface {
                    from: "#010101".into(),
                    to: "#010101".into(),
                }),
                ..Default::default()
            },
            plaque: Some(PlaquePaint {
                accent: Some("#FF0000".into()),
                ..Default::default()
            }),
            edge: None,
        };
        let theme = Theme::preset("Slate");
        let chrome = Chrome::new(&theme, SkinId::Fleet, Some(&paint));
        assert_eq!(chrome.band.at(0.0), rgb(0x112233));
        assert_eq!(chrome.ground, rgb(0x010101));
        assert_eq!(chrome.plaque_accent, rgb(0xFF0000));
        assert_eq!(chrome.skin, WindowSkin::FLEET);
        let system = Chrome::new(&theme, SkinId::System, Some(&paint));
        assert_eq!(system.ground, rgb(0x010101), "a world that paints its ground keeps it");
    }
}
