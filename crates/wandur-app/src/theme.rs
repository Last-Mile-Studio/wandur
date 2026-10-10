//! Colour themes: the C# client's eleven preset palettes (`UserTheme.cs`), each with its chrome
//! colours and a full sixteen colour terminal palette, applied to egui's visuals and to the
//! terminal. Light and dark presets both work; Hull pairs light chrome with a dark transcript, as
//! in C#. A custom theme names its own chrome colours, and a world's theme replaces the palette
//! while one of its sessions is the active one ([`Theme::resolve`]). The window skins that paint
//! the chassis from these colours live in [`crate::skin`].

use egui::{Color32, CornerRadius, Stroke, Visuals};
use wandur_core::directory::WorldTheme;
use wandur_term::alacritty_terminal::vte::ansi::{Color as TermColor, NamedColor};

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// One C# preset: name, light, shell, panel, terminal, text, muted, accent, border, map
/// background, map grid, and the sixteen terminal colours.
struct Preset {
    name: &'static str,
    light: bool,
    shell: u32,
    panel: u32,
    terminal: u32,
    text: u32,
    muted: u32,
    accent: u32,
    border: u32,
    map_background: u32,
    map_grid: u32,
    ansi: [u32; 16],
}

/// The historic dark terminal palette (the C# `DarkAnsi`).
const DARK_ANSI: [u32; 16] = [
    0x6B6B80, 0xDE6363, 0x0DBC79, 0xE5C07B, 0x61AFEF, 0xC678DD, 0x56B6C2, 0xD4D4D4, 0x8A8A8A, 0xF47B7B, 0x23D18B,
    0xF5F543, 0x82AAFF, 0xD670D6, 0x29B8DB, 0xFFFFFF,
];

#[rustfmt::skip]
const PRESETS: [Preset; 11] = [
    Preset { name: "Hull", light: true, shell: 0xD1D4D2, panel: 0xDCDEDC, terminal: 0x11171B, text: 0x202629, muted: 0x404B50, accent: 0x115C73, border: 0x7E8788, map_background: 0x11171B, map_grid: 0x26333C, ansi: DARK_ANSI },
    Preset { name: "Ember", light: false, shell: 0x141519, panel: 0x212328, terminal: 0x1A1C21, text: 0xE3E4E8, muted: 0x979BA6, accent: 0xDBBFA0, border: 0x2B2E35, map_background: 0x10191F, map_grid: 0x1D2B34, ansi: DARK_ANSI },
    Preset { name: "Moonlight", light: false, shell: 0x12141C, panel: 0x1C1F2B, terminal: 0x171A24, text: 0xE3E6F1, muted: 0x969CAF, accent: 0xB9B6F2, border: 0x292D3C, map_background: 0x10191F, map_grid: 0x1D2B34, ansi: DARK_ANSI },
    Preset { name: "Forest", light: false, shell: 0x111816, panel: 0x1D2823, terminal: 0x17201C, text: 0xDFE8E1, muted: 0x92A397, accent: 0xA4C8AD, border: 0x2A3730, map_background: 0x10191F, map_grid: 0x1D2B34, ansi: DARK_ANSI },
    Preset { name: "Midnight", light: false, shell: 0x0D1220, panel: 0x161D30, terminal: 0x111728, text: 0xDCE3F2, muted: 0x93A0BE, accent: 0x7FB2FF, border: 0x232C45, map_background: 0x0B1020, map_grid: 0x1E2742,
        ansi: [0x606980, 0xDD5A63, 0x2BD458, 0xD4BA2B, 0x4988DA, 0xCB51DB, 0x2BC3D4, 0xC9CBCF, 0x818692, 0xF68890, 0x54F27E, 0xF2DA54, 0x76ADF5, 0xE77EF5, 0x54E2F2, 0xFFFFFF] },
    Preset { name: "Slate", light: false, shell: 0x16181A, panel: 0x212428, terminal: 0x1B1E21, text: 0xE2E4E7, muted: 0x9AA0A6, accent: 0xB8C0C8, border: 0x2D3136, map_background: 0x14171A, map_grid: 0x272B30,
        ansi: [0x5E6E7D, 0xCF6E6E, 0x40BF6A, 0xBFB540, 0x668BCC, 0xC766CC, 0x40AABF, 0xC9CCCF, 0x848C94, 0xED9898, 0x63E38E, 0xE3D963, 0x8EB0EB, 0xE68EEB, 0x63CEE3, 0xFFFFFF] },
    Preset { name: "Rose", light: false, shell: 0x1A1114, panel: 0x26191E, terminal: 0x1F1418, text: 0xF2E2E6, muted: 0xB39AA2, accent: 0xE58FA5, border: 0x3A2630, map_background: 0x170F12, map_grid: 0x33222A,
        ansi: [0x806067, 0xE0594D, 0x26D971, 0xD9D926, 0x5A81E2, 0xDD3CD7, 0x26ACD9, 0xCFC9CA, 0x928185, 0xF7887E, 0x52F496, 0xF4F452, 0x8CAAF8, 0xF66BF1, 0x52CCF4, 0xFFFFFF] },
    Preset { name: "Paper", light: true, shell: 0xEEEFEF, panel: 0xF6F7F7, terminal: 0xFFFFFF, text: 0x282D35, muted: 0x5F6674, accent: 0x9A7443, border: 0xDEE1E6, map_background: 0xF4F5F6, map_grid: 0xDCE0E6,
        ansi: [0x13161B, 0xA12121, 0x1B833E, 0x7B7319, 0x2150A1, 0x9A21A1, 0x1D798C, 0x43464C, 0x5D636F, 0x9A0404, 0x02591F, 0x504902, 0x043FA4, 0x85048B, 0x035464, 0x1F2228] },
    Preset { name: "Parchment", light: true, shell: 0xF3EADA, panel: 0xF9F2E6, terminal: 0xFBF5EA, text: 0x3A3128, muted: 0x6B5E4C, accent: 0x8A5E2B, border: 0xE0D3BD, map_background: 0xF6EFE1, map_grid: 0xDFD2BC,
        ansi: [0x1B1813, 0xA5261D, 0x15793D, 0x716F14, 0x1D46A5, 0xA51DA2, 0x19748F, 0x4C4843, 0x6F685D, 0x900D04, 0x024F21, 0x464502, 0x0434A4, 0x81037F, 0x034D63, 0x28241F] },
    Preset { name: "Daylight", light: true, shell: 0xF2F4F7, panel: 0xFAFBFC, terminal: 0xFFFFFF, text: 0x1F2933, muted: 0x5A6672, accent: 0x1F6FEB, border: 0xDCE1E8, map_background: 0xF7F9FB, map_grid: 0xD8DEE6,
        ansi: [0x13171B, 0xA31F28, 0x188134, 0x817118, 0x1F58A3, 0x931FA3, 0x197A85, 0x43474C, 0x5D656F, 0x9A040E, 0x025518, 0x5A4C02, 0x04479F, 0x840495, 0x02555E, 0x1F2328] },
    Preset { name: "Linen", light: true, shell: 0xEDEAE4, panel: 0xF5F3EE, terminal: 0xF8F6F2, text: 0x2F2E2B, muted: 0x62605A, accent: 0x2F6E62, border: 0xDFDBD2, map_background: 0xF2EFE9, map_grid: 0xDAD5CB,
        ansi: [0x1B1913, 0x9D2525, 0x1D7C3D, 0x746C1B, 0x25519D, 0x97259D, 0x1F7384, 0x4C4A43, 0x6F6B5D, 0x920808, 0x04521E, 0x4E4804, 0x0840A0, 0x7D0783, 0x054D5C, 0x28261F] },
];

/// WCAG relative luminance.
pub fn luminance(c: Color32) -> f32 {
    let lin = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.039_28 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// True when black reads better than white on this background (the C# threshold).
pub fn is_light(c: Color32) -> bool {
    luminance(c) > 0.1791
}

/// WCAG contrast ratio of two colours.
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Mix two colours: `t` = 0 gives `a`, 1 gives `b`.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// Colours of one theme.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    /// The chrome is light (egui's light visuals underneath).
    pub light: bool,
    pub shell: Color32,
    pub panel: Color32,
    pub terminal: Color32,
    /// Interface text.
    pub text: Color32,
    /// Default text on the terminal (light on Hull's dark transcript).
    pub terminal_text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    /// The second accent: primary buttons and the transcript's selection (a world may set it apart).
    pub accent_secondary: Color32,
    pub border: Color32,
    /// The title band and dock header colour a custom theme names (the panel colour otherwise).
    pub chrome: Color32,
    /// The palette is the one Fleet was drawn from (Hull, no world, no custom theme): the skin
    /// paints its reference metal.
    pub reference: bool,
    /// Connected, online, beginner friendly.
    pub ok: Color32,
    pub warn: Color32,
    pub error: Color32,
    /// Background of selected terminal cells.
    pub selection: Color32,
    /// Behind a world card's picture while it loads, and its initials.
    pub plate: Color32,
    pub map_background: Color32,
    pub map_grid: Color32,
    pub ansi: [Color32; 16],
}

impl Default for Theme {
    fn default() -> Self {
        Self::preset(wandur_core::settings::DEFAULT_THEME)
    }
}

impl Theme {
    /// The preset names, in the C# client's order.
    pub fn names() -> impl Iterator<Item = &'static str> {
        PRESETS.iter().map(|p| p.name)
    }

    /// A preset by name; an unknown name gives Hull, as in C#.
    pub fn preset(name: &str) -> Self {
        let p = PRESETS
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .unwrap_or(&PRESETS[0]);
        let terminal = hex(p.terminal);
        let text = hex(p.text);
        // The transcript's text: the interface text unless the transcript's lightness differs from
        // the chrome's (Hull), as the C# `TerminalTextFor` does.
        let terminal_text = if is_light(terminal) == p.light {
            text
        } else if p.light {
            hex(0xCBD6E2)
        } else {
            hex(0x1F2933)
        };
        let panel = hex(p.panel);
        let panel_light = is_light(panel);
        let accent = hex(p.accent);
        // The map follows the chrome: a light theme whose preset map is dark (Hull, which C#
        // draws with a dark map) gets a light map made from its panel and border colours.
        let (map_background, map_grid) = if p.light && !is_light(hex(p.map_background)) {
            (mix(panel, Color32::WHITE, 0.45), mix(panel, hex(p.border), 0.45))
        } else {
            (hex(p.map_background), hex(p.map_grid))
        };
        Self {
            name: p.name,
            light: p.light,
            shell: hex(p.shell),
            panel,
            terminal,
            text,
            terminal_text,
            muted: hex(p.muted),
            accent,
            accent_secondary: accent,
            border: hex(p.border),
            chrome: panel,
            reference: p.name == "Hull",
            ok: if panel_light { hex(0x1E8A4E) } else { hex(0x3DDC8C) },
            warn: if panel_light { hex(0x9A6700) } else { hex(0xE5C07B) },
            error: if panel_light { hex(0xB42318) } else { hex(0xF47B7B) },
            selection: mix(terminal, accent, 0.35),
            plate: mix(panel, hex(p.border), 0.5),
            map_background,
            map_grid,
            ansi: p.ansi.map(hex),
        }
    }

    /// The theme the settings choose: a preset, or a custom scheme (its preset with its own
    /// terminal colours), then the transcript text and background overrides. Every colour here
    /// is read when a frame is painted, so a change applies to the transcript as it stands.
    pub fn from_settings(settings: &wandur_core::settings::Settings) -> Self {
        Self::resolve(settings, None)
    }

    /// The theme on screen: a world's theme while one of its sessions is the active one (when
    /// the settings allow world themes), else the chosen preset or custom theme; then the
    /// transcript text and background overrides (the C# `ThemeService.Apply`).
    pub fn resolve(settings: &wandur_core::settings::Settings, world: Option<&WorldTheme>) -> Self {
        let world = world.filter(|w| settings.use_world_themes && w.is_valid());
        let mut theme = match (world, settings.custom_theme()) {
            (Some(world), custom) => {
                let mut theme = Self::from_world(world, &settings.theme);
                if let Some(custom) = custom {
                    // A custom theme keeps the terminal colours its editor shows, over a world too.
                    theme.ansi = custom_ansi(custom);
                }
                theme
            }
            (None, Some(custom)) => Self::from_custom(custom),
            (None, None) => Self::preset(&settings.theme),
        };
        if let Some(color) = settings.foreground.as_deref().and_then(parse_hex) {
            theme.terminal_text = color;
        }
        if let Some(color) = settings.background.as_deref().and_then(parse_hex) {
            theme.terminal = color;
            theme.selection = mix(color, theme.accent, 0.35);
        }
        theme
    }

    /// A custom theme: its own chrome colours (or its base preset's, for a theme saved before it
    /// had any) and its terminal colours.
    pub fn from_custom(custom: &wandur_core::settings::CustomTheme) -> Self {
        let mut theme = Self::preset(&custom.base);
        theme.ansi = custom_ansi(custom);
        theme.reference = false;
        if custom.colors.is_empty() {
            return theme;
        }
        let color = |key: &str, fallback: Color32| custom.color(key).and_then(parse_hex).unwrap_or(fallback);
        theme.light = custom.is_light;
        theme.shell = color("Shell", theme.shell);
        theme.panel = color("Panel", theme.panel);
        theme.terminal = color("Terminal", theme.terminal);
        theme.text = color("Text", theme.text);
        theme.muted = color("Muted", theme.muted);
        theme.accent = color("Accent", theme.accent);
        theme.accent_secondary = color("AccentSecondary", theme.accent);
        theme.border = color("Border", theme.border);
        theme.terminal_text = color("TerminalText", theme.terminal_text);
        theme.chrome = color("Chrome", theme.panel);
        let (preset, _) = preset_colors(&custom.base);
        let untouched_map = custom.color("MapBackground") == preset.get("MapBackground").map(String::as_str)
            && custom.color("MapGrid") == preset.get("MapGrid").map(String::as_str);
        if !untouched_map {
            // A map colour the person chose is used as it is; an untouched copy keeps the
            // preset's map, which follows the chrome's lightness (the M4 ruling).
            theme.map_background = color("MapBackground", theme.map_background);
            theme.map_grid = color("MapGrid", theme.map_grid);
        }
        theme.derive();
        theme
    }

    /// A world's theme. Its nine colours replace the palette; the terminal colours are the
    /// chosen preset's when they suit the world's transcript, else a palette made for that
    /// lightness (the C# `PaletteForBackground`).
    pub fn from_world(world: &WorldTheme, preset: &str) -> Self {
        let mut theme = Self::preset(preset);
        let c = &world.colors;
        let color = |value: &str, fallback: Color32| parse_hex(value).unwrap_or(fallback);
        let preset_terminal_light = is_light(theme.terminal);
        theme.light = world.is_light();
        theme.reference = false;
        theme.shell = color(&c.shell, theme.shell);
        theme.panel = color(&c.panel, theme.panel);
        theme.terminal = color(&c.terminal, theme.terminal);
        theme.text = color(&c.text, theme.text);
        theme.muted = color(&c.muted, theme.muted);
        theme.accent = color(&c.accent, theme.accent);
        theme.accent_secondary = color(&c.accent_secondary, theme.accent);
        theme.border = color(&c.border, theme.border);
        theme.terminal_text = color(&c.terminal_text, theme.terminal_text);
        theme.chrome = theme.panel;
        if is_light(theme.terminal) != preset_terminal_light {
            theme.ansi = if is_light(theme.terminal) {
                Self::preset("Linen").ansi
            } else {
                Self::preset("Ember").ansi
            };
        }
        // The map takes the world's transcript and border colours (C#), following the chrome's
        // lightness as the presets do (the M4 ruling).
        theme.map_background = theme.terminal;
        theme.map_grid = theme.border;
        if theme.light && !is_light(theme.map_background) {
            theme.map_background = mix(theme.panel, Color32::WHITE, 0.45);
            theme.map_grid = mix(theme.panel, theme.border, 0.45);
        }
        theme.derive();
        theme
    }

    /// The colours made from the others, after a custom or world palette replaced them.
    fn derive(&mut self) {
        let panel_light = is_light(self.panel);
        self.ok = if panel_light { hex(0x1E8A4E) } else { hex(0x3DDC8C) };
        self.warn = if panel_light { hex(0x9A6700) } else { hex(0xE5C07B) };
        self.error = if panel_light { hex(0xB42318) } else { hex(0xF47B7B) };
        self.selection = mix(self.terminal, self.accent_secondary, 0.35);
        self.plate = mix(self.panel, self.border, 0.5);
    }

    pub fn ember() -> Self {
        Self::preset("Ember")
    }

    /// A palette entry: 0 to 15 from the theme, the rest from the xterm cube and grey ramp.
    pub fn indexed(&self, index: u8) -> Color32 {
        if index < 16 {
            self.ansi[index as usize]
        } else {
            let (r, g, b) = xterm_256(index);
            Color32::from_rgb(r, g, b)
        }
    }

    /// Foreground colour of a cell. Bold turns the eight basic colours bright, as MUDs expect.
    pub fn term_fg(&self, color: TermColor, bold: bool) -> Color32 {
        match color {
            TermColor::Spec(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
            TermColor::Indexed(i) => self.indexed(i),
            TermColor::Named(named) => {
                let n = named as usize;
                if n < 8 {
                    self.ansi[if bold { n + 8 } else { n }]
                } else if n < 16 {
                    self.ansi[n]
                } else {
                    match named {
                        NamedColor::Background => self.terminal,
                        NamedColor::DimForeground => self.terminal_text.gamma_multiply(0.6),
                        NamedColor::BrightForeground => {
                            if is_light(self.terminal) {
                                Color32::BLACK
                            } else {
                                Color32::WHITE
                            }
                        }
                        NamedColor::DimBlack
                        | NamedColor::DimRed
                        | NamedColor::DimGreen
                        | NamedColor::DimYellow
                        | NamedColor::DimBlue
                        | NamedColor::DimMagenta
                        | NamedColor::DimCyan
                        | NamedColor::DimWhite => {
                            let base = named as usize - NamedColor::DimBlack as usize;
                            self.ansi[base].gamma_multiply(0.6)
                        }
                        _ => self.terminal_text,
                    }
                }
            }
        }
    }

    /// Background colour of a cell; the default background is transparent (the panel shows).
    pub fn term_bg(&self, color: TermColor) -> Color32 {
        match color {
            TermColor::Named(NamedColor::Background) => Color32::TRANSPARENT,
            other => self.term_fg(other, false),
        }
    }

    pub fn is_default_bg(&self, color: TermColor) -> bool {
        matches!(color, TermColor::Named(NamedColor::Background))
    }

    /// A status dot, painted (the bundled fonts have no U+25CF glyph, so a text dot shows as a box).
    pub fn dot(ui: &mut egui::Ui, color: Color32) {
        let size = ui.spacing().interact_size.y * 0.5;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), size * 0.4, color);
    }

    /// A hollow status dot (not connected).
    pub fn ring(ui: &mut egui::Ui, color: Color32) {
        let size = ui.spacing().interact_size.y * 0.5;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        ui.painter()
            .circle_stroke(rect.center(), size * 0.35, Stroke::new(1.2, color));
    }

    /// The egui theme these visuals belong to.
    pub fn egui_theme(&self) -> egui::Theme {
        if self.light {
            egui::Theme::Light
        } else {
            egui::Theme::Dark
        }
    }

    /// Apply to a context: the visuals for the theme's lightness, and that lightness forced (the
    /// system setting would otherwise pick the other set of visuals).
    pub fn apply(&self, ctx: &egui::Context) {
        ctx.set_theme(self.egui_theme());
        ctx.set_visuals_of(self.egui_theme(), self.visuals());
    }

    /// egui visuals for the workspace chrome.
    pub fn visuals(&self) -> Visuals {
        let mut v = if self.light { Visuals::light() } else { Visuals::dark() };
        let (hover, press) = if self.light { (0.10, 0.18) } else { (0.08, 0.16) };
        let ink = if self.light { Color32::BLACK } else { Color32::WHITE };
        let control = mix(self.panel, ink, if self.light { 0.06 } else { 0.07 });
        v.override_text_color = Some(self.text);
        v.panel_fill = self.panel;
        v.window_fill = self.panel;
        // Text fields and scroll wells sit on the far side from the text.
        let well = if self.light {
            mix(self.panel, Color32::WHITE, 0.6)
        } else {
            mix(self.shell, Color32::BLACK, 0.15)
        };
        v.extreme_bg_color = well;
        v.text_edit_bg_color = Some(well);
        v.faint_bg_color = self.shell;
        v.code_bg_color = self.shell;
        v.hyperlink_color = self.accent;
        v.weak_text_color = Some(self.muted);
        v.selection.bg_fill = mix(self.panel, self.accent, 0.35);
        v.selection.stroke = Stroke::new(1.0, if self.light { self.text } else { self.accent });
        v.window_stroke = Stroke::new(1.0, self.border);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        v.widgets.noninteractive.bg_fill = self.panel;
        v.widgets.noninteractive.weak_bg_fill = self.panel;
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, self.text);
        v.widgets.inactive.bg_fill = control;
        v.widgets.inactive.weak_bg_fill = control;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, self.border);
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, self.text);
        v.widgets.hovered.bg_fill = mix(control, ink, hover);
        v.widgets.hovered.weak_bg_fill = mix(control, ink, hover);
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, self.accent);
        v.widgets.hovered.fg_stroke = Stroke::new(1.5, self.text);
        v.widgets.active.bg_fill = mix(control, ink, press);
        v.widgets.active.weak_bg_fill = mix(self.panel, self.accent, 0.3);
        v.widgets.active.fg_stroke = Stroke::new(1.5, self.text);
        v.widgets.open.weak_bg_fill = mix(control, ink, hover);
        let radius = CornerRadius::same(4);
        v.widgets.noninteractive.corner_radius = radius;
        v.widgets.inactive.corner_radius = radius;
        v.widgets.hovered.corner_radius = radius;
        v.widgets.active.corner_radius = radius;
        v.widgets.open.corner_radius = radius;
        v.window_corner_radius = CornerRadius::same(6);
        // A steady caret: egui's blink costs about ten frames a second while idle (measured in
        // Milestone 2), the largest part of idle CPU.
        v.text_cursor.blink = false;
        v
    }

    /// The sixteen colours for coloured text on the panel (the Channels panel): the theme's own
    /// when the panel and the transcript have the same lightness, else the C# fallback for that
    /// lightness (Linen's palette on light, Ember's on dark), as `PaletteForBackground` does.
    pub fn panel_ansi(&self) -> [Color32; 16] {
        if is_light(self.panel) == is_light(self.terminal) {
            self.ansi
        } else if is_light(self.panel) {
            Theme::preset("Linen").ansi
        } else {
            Theme::preset("Ember").ansi
        }
    }

    /// Behind a selected row.
    pub fn selection_fill(&self) -> Color32 {
        mix(self.panel, self.accent, 0.22)
    }

    /// Behind a hovered row.
    pub fn hover_fill(&self) -> Color32 {
        mix(self.panel, self.text, 0.06)
    }

    /// Behind an open menu (a little lighter than the panels on light themes, as the C# menus).
    pub fn menu_fill(&self) -> Color32 {
        if self.light {
            mix(self.panel, Color32::WHITE, 0.45)
        } else {
            mix(self.panel, self.text, 0.04)
        }
    }

    /// Text of a disabled item.
    pub fn disabled_text(&self) -> Color32 {
        mix(self.text, self.panel, 0.55)
    }

    /// Behind a modal dialog: the window dimmed.
    pub fn dim(&self) -> Color32 {
        Color32::from_black_alpha(if self.light { 90 } else { 120 })
    }

    /// Text on an accent-filled button (kept readable).
    pub fn on_accent(&self) -> Color32 {
        if is_light(self.accent) {
            Color32::BLACK
        } else {
            Color32::WHITE
        }
    }

    /// The face of a primary button (Send, Save): the second accent, as C# fills them from it.
    pub fn primary(&self) -> Color32 {
        self.accent_secondary
    }

    /// Text on a primary button (kept readable).
    pub fn on_primary(&self) -> Color32 {
        if is_light(self.accent_secondary) {
            Color32::BLACK
        } else {
            Color32::WHITE
        }
    }
}

/// Each preset's code editor colours (the C# presets' EditorBackground and EditorText).
const EDITOR: [(u32, u32); 11] = [
    (0xF4F5F6, 0x202622),
    (0x161B22, 0xE6EDF3),
    (0x161B22, 0xE6EDF3),
    (0x161B22, 0xE6EDF3),
    (0x101628, 0xDCE3F2),
    (0x1A1D20, 0xE2E4E7),
    (0x1E1317, 0xF2E2E6),
    (0xFFFFFF, 0x24292F),
    (0xFBF5EA, 0x3A3128),
    (0xFFFFFF, 0x1F2933),
    (0xF8F6F2, 0x2F2E2B),
];

/// A preset's colours as a custom theme names them (the C# `UserTheme.FromPreset`), keyed by
/// [`wandur_core::settings::COLOR_KEYS`], and whether its chrome is light.
pub fn preset_colors(name: &str) -> (std::collections::BTreeMap<String, String>, bool) {
    let index = PRESETS
        .iter()
        .position(|p| p.name.eq_ignore_ascii_case(name))
        .unwrap_or(0);
    let p = &PRESETS[index];
    let terminal_text = to_hex(Theme::preset(p.name).terminal_text);
    let h = |c: u32| to_hex(hex(c));
    let (editor_background, editor_text) = EDITOR[index];
    let pairs = [
        ("Shell", h(p.shell)),
        ("Panel", h(p.panel)),
        ("Terminal", h(p.terminal)),
        ("Text", h(p.text)),
        ("Muted", h(p.muted)),
        ("Accent", h(p.accent)),
        ("AccentSecondary", h(p.accent)),
        ("Border", h(p.border)),
        ("TerminalText", terminal_text),
        ("Chrome", h(p.panel)),
        ("Selection", h(p.border)),
        ("Button", h(p.panel)),
        ("ButtonText", h(p.text)),
        ("PrimaryText", "#242424".to_string()),
        ("MapBackground", h(p.map_background)),
        ("MapGrid", h(p.map_grid)),
        ("EditorBackground", h(editor_background)),
        ("EditorText", h(editor_text)),
    ];
    (pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect(), p.light)
}

/// A preset's name in the UI language (the C# `UserTheme.DisplayName`); other names unchanged.
pub fn preset_label(name: &str) -> &str {
    use wandur_core::l10n::{S, t};
    let key = match name {
        "Hull" => S::ThemeHull,
        "Ember" => S::ThemeEmber,
        "Moonlight" => S::ThemeMoonlight,
        "Forest" => S::ThemeForest,
        "Paper" => S::ThemePaper,
        "Midnight" => S::ThemeMidnight,
        "Slate" => S::ThemeSlate,
        "Rose" => S::ThemeRose,
        "Parchment" => S::ThemeParchment,
        "Daylight" => S::ThemeDaylight,
        "Linen" => S::ThemeLinen,
        _ => return name,
    };
    t(key)
}

/// `#RRGGBB` as a colour.
pub fn parse_hex(value: &str) -> Option<Color32> {
    wandur_core::settings::parse_color(value.trim()).map(|(r, g, b)| Color32::from_rgb(r, g, b))
}

/// `#RRGGBB` for a colour.
pub fn to_hex(color: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b())
}

/// A custom scheme's sixteen terminal colours: its own, else the shipped defaults (as C#, where a
/// custom scheme keeps only the entries that differ from `AnsiPalette.Defaults`).
pub fn custom_ansi(custom: &wandur_core::settings::CustomTheme) -> [Color32; 16] {
    std::array::from_fn(|i| {
        custom
            .ansi_colors
            .get(&(i as u8))
            .and_then(|c| parse_hex(c))
            .or_else(|| parse_hex(wandur_core::settings::ANSI_DEFAULTS[i]))
            .unwrap_or(Color32::GRAY)
    })
}

/// The RGB value of a 256 colour palette entry above 15 (the 6x6x6 cube and the grey ramp).
/// Entries 0 to 15 belong to the theme.
pub fn xterm_256(index: u8) -> (u8, u8, u8) {
    match index {
        0..=15 => (0, 0, 0),
        16..=231 => {
            let n = index - 16;
            let channel = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (channel(n / 36), channel(n / 6 % 6), channel(n % 6))
        }
        232..=255 => {
            let c = 8 + (index - 232) * 10;
            (c, c, c)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_lookup() {
        let t = Theme::ember();
        assert_eq!(
            t.term_fg(TermColor::Named(NamedColor::Foreground), false),
            t.terminal_text
        );
        assert_eq!(
            t.term_bg(TermColor::Named(NamedColor::Background)),
            Color32::TRANSPARENT
        );
        assert_eq!(t.term_fg(TermColor::Named(NamedColor::Red), false), t.ansi[1]);
        assert_eq!(t.term_fg(TermColor::Named(NamedColor::Red), true), t.ansi[9]);
        assert_eq!(t.term_fg(TermColor::Indexed(1), true), t.ansi[1]);
        assert_eq!(t.term_fg(TermColor::Indexed(196), false), Color32::from_rgb(255, 0, 0));
        assert_eq!(t.indexed(232), Color32::from_rgb(8, 8, 8));
    }

    #[test]
    fn cube_and_greys_match_xterm() {
        assert_eq!(xterm_256(16), (0, 0, 0));
        assert_eq!(xterm_256(196), (255, 0, 0));
        assert_eq!(xterm_256(231), (255, 255, 255));
        assert_eq!(xterm_256(232), (8, 8, 8));
        assert_eq!(xterm_256(255), (238, 238, 238));
    }

    #[test]
    fn the_map_follows_the_theme_lightness() {
        for name in Theme::names() {
            let t = Theme::preset(name);
            assert_eq!(is_light(t.map_background), t.light, "{name}");
            assert_ne!(t.map_background, t.map_grid, "{name}: the grid shows");
        }
    }

    #[test]
    fn every_preset_is_ported_with_its_lightness() {
        let names: Vec<_> = Theme::names().collect();
        assert_eq!(names.len(), 11);
        assert_eq!(names[0], "Hull");
        for name in names {
            let t = Theme::preset(name);
            assert_eq!(t.name, name);
            assert_eq!(is_light(t.panel), t.light, "{name}: chrome lightness matches the flag");
            // Interface text and transcript text are readable on their backgrounds.
            assert!(contrast(t.text, t.panel) >= 4.5, "{name}: text on panel");
            assert!(contrast(t.terminal_text, t.terminal) >= 4.5, "{name}: transcript text");
            assert!(contrast(t.muted, t.panel) >= 3.0, "{name}: muted on panel");
            // The terminal palette suits the transcript's lightness: white is the lightest
            // colour on a dark transcript, black-ish is the darkest on a light one.
            let white = t.ansi[15];
            if is_light(t.terminal) {
                assert!(
                    luminance(white) < 0.1,
                    "{name}: bright white is dark ink on light paper"
                );
            } else {
                assert_eq!(white, Color32::WHITE, "{name}");
            }
        }
        assert_eq!(Theme::preset("nonsense").name, "Hull");
    }

    #[test]
    fn hull_is_light_chrome_around_a_dark_transcript() {
        let hull = Theme::preset("Hull");
        assert!(hull.light && !is_light(hull.terminal));
        assert_eq!(hull.terminal_text, hex(0xCBD6E2));
        let paper = Theme::preset("paper");
        assert!(paper.light && is_light(paper.terminal));
        assert_eq!(paper.terminal_text, paper.text);
        // Coloured channel text on Hull's light panel uses a palette made for light backgrounds.
        assert_eq!(hull.panel_ansi(), Theme::preset("Linen").ansi);
        assert_eq!(paper.panel_ansi(), paper.ansi);
    }

    #[test]
    fn applying_sets_the_matching_egui_theme() {
        let ctx = egui::Context::default();
        let paper = Theme::preset("Paper");
        paper.apply(&ctx);
        assert_eq!(ctx.theme(), egui::Theme::Light);
        assert_eq!(ctx.style_of(egui::Theme::Light).visuals.panel_fill, paper.panel);
        assert!(!ctx.global_style().visuals.dark_mode);
        let ember = Theme::ember();
        ember.apply(&ctx);
        assert_eq!(ctx.theme(), egui::Theme::Dark);
        assert_eq!(ctx.global_style().visuals.panel_fill, ember.panel);
        assert_eq!(ctx.global_style().visuals.override_text_color, Some(ember.text));
    }

    #[test]
    fn custom_schemes_and_overrides_come_from_the_settings() {
        use wandur_core::settings::{ANSI_DEFAULTS, CustomTheme, Settings};
        let mut custom = CustomTheme {
            name: "Hull copy 1".into(),
            base: "Hull".into(),
            ..CustomTheme::default()
        };
        custom.ansi_colors.insert(1, "#112233".into());
        let settings = Settings {
            theme: custom.id.clone(),
            custom_themes: vec![custom],
            foreground: Some("#A0B0C0".into()),
            background: Some("#010203".into()),
            ..Settings::default()
        };
        let theme = Theme::from_settings(&settings);
        assert_eq!(theme.name, "Hull");
        assert_eq!(theme.ansi[1], hex(0x112233));
        assert_eq!(theme.ansi[2], parse_hex(ANSI_DEFAULTS[2]).unwrap());
        assert_eq!(theme.terminal_text, hex(0xA0B0C0));
        assert_eq!(theme.terminal, hex(0x010203));
        assert_eq!(to_hex(theme.terminal), "#010203");
        // Without overrides it is the preset.
        assert_eq!(Theme::from_settings(&Settings::default()), Theme::preset("Hull"));
    }
}
