//! Basic settings and saved worlds, kept as one versioned JSON file in the data directory.
//!
//! The data directory is `--data-dir` when given (the C# client's flag), else the platform's
//! application data directory plus `Wandur-Rust` (so the prototype never touches the C# client's
//! `Wandur` data). Writes go to a temporary file that is then renamed over the old one, so a crash
//! never leaves half a file. A file that cannot be read is moved aside and defaults are used.
//! Saving happens on a background thread ([`Saver`]); several saves in quick succession are
//! coalesced into one write.

use crate::l10n::{S, t, tf};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::charset::Charset;
use crate::endpoint::Endpoint;

pub const SETTINGS_FILE: &str = "settings.json";
pub const SETTINGS_VERSION: u32 = 1;
/// Directory name under the platform data directory.
pub const APP_DIR: &str = "Wandur-Rust";

/// Allowed range of the terminal font size, in points (the C# rule: 11 to 28).
pub const FONT_SIZES: std::ops::RangeInclusive<f32> = 11.0..=28.0;
/// Allowed range of the output frame cap (0 means no cap).
pub const MAX_OUTPUT_FPS: u32 = 240;
/// Scrollback rows; the hard maximum is the terminal's own.
pub const MIN_SCROLLBACK: usize = 100;

/// A world the person saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedWorld {
    /// The stable world id (32 hex digits) that maps, scripts and usage are keyed by. It stays
    /// when the address is edited; empty in files from before it existed, until
    /// [`Settings::assign_world_ids`] gives one.
    pub world_id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub charset: Charset,
    pub auto_reconnect: bool,
    /// The directory listing this world was added from, for its artwork; empty when typed in.
    pub listing_id: String,
    /// When the world was last connected to (seconds since 1970), for the recent order.
    pub last_connected: Option<u64>,
    /// How many times the world was connected to.
    pub connections: u32,
    /// The login name (character or account), sent by auto-login and shown as the session's
    /// character until the world names one. Empty when none is saved.
    pub username: String,
    /// The reference of the password saved in the system vault (a random UUID; see
    /// [`crate::login::vault::key`]), or `None`. The password itself is never in this file.
    pub password_id: Option<String>,
    /// Log in automatically on connect (needs a username and a saved password).
    pub auto_login: bool,
    /// The username prompt, a case-insensitive regular expression over the whole prompt line.
    pub username_prompt: String,
    /// The password prompt; it also makes input private when it matches.
    pub password_prompt: String,
    /// The protocol mapping from the directory listing the world was added from (vitals, the
    /// opponent card, the reported character). Only used at its exact endpoint; an invalid one
    /// in the file is dropped.
    #[serde(
        deserialize_with = "crate::protocol::mapping::lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub protocol_mapping: Option<crate::protocol::mapping::WorldMapping>,
    /// The world's codebase, from its directory listing ("SMAUG 1.4a"): it picks the shipped
    /// channel rule family. Empty when unknown (then the server's MSSP CODEBASE is used).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub codebase: String,
    /// Channel rules taught for this world (Mark as channel, the world editor's Channels
    /// section), in the shape a channel pack carries. They run before the family's.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub channel_rules: Vec<crate::channels::ChannelRule>,
    /// The world's own appearance, from its directory listing (applied while one of its sessions
    /// is the active one and Settings > Appearance allows world themes). An invalid one is dropped.
    #[serde(
        deserialize_with = "crate::directory::world_theme::lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub theme: Option<crate::directory::WorldTheme>,
    /// The world's own command style (the world editor's Connection section); `None` follows
    /// the global one, or MUSH-safe on a MUSH or MUX.
    #[serde(
        deserialize_with = "crate::command_line::lenient_override",
        skip_serializing_if = "Option::is_none"
    )]
    pub command_style: Option<crate::command_line::CommandStyle>,
}

impl Default for SavedWorld {
    fn default() -> Self {
        Self {
            world_id: String::new(),
            name: String::new(),
            host: String::new(),
            port: 4000,
            tls: false,
            charset: Charset::Utf8,
            auto_reconnect: false,
            listing_id: String::new(),
            last_connected: None,
            connections: 0,
            username: String::new(),
            password_id: None,
            auto_login: false,
            username_prompt: crate::login::DEFAULT_USERNAME_PROMPT.into(),
            password_prompt: crate::login::DEFAULT_PASSWORD_PROMPT.into(),
            protocol_mapping: None,
            codebase: String::new(),
            channel_rules: Vec::new(),
            theme: None,
            command_style: None,
        }
    }
}

impl SavedWorld {
    pub fn from_endpoint(name: impl Into<String>, endpoint: &Endpoint) -> Self {
        Self {
            name: name.into(),
            host: endpoint.host.clone(),
            port: endpoint.port,
            tls: endpoint.tls,
            ..Self::default()
        }
    }

    pub fn endpoint(&self) -> Endpoint {
        Endpoint {
            host: self.host.clone(),
            port: self.port,
            tls: self.tls,
        }
    }

    /// Whether this world is at `endpoint` (host names compare without case and a trailing dot).
    pub fn is_at(&self, endpoint: &Endpoint) -> bool {
        same_host(&self.host, &endpoint.host) && self.port == endpoint.port && self.tls == endpoint.tls
    }

    /// Note a connection now.
    pub fn note_connected(&mut self, now_secs: u64) {
        self.last_connected = Some(now_secs);
        self.connections = self.connections.saturating_add(1);
    }

    /// Check what a person typed in the world form; the message says what to fix.
    pub fn validate(&self) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err(t(S::GiveTheWorldAName).into());
        }
        if name.chars().count() > 100 || name.chars().any(char::is_control) {
            return Err(t(S::WorldNameTooLong).into());
        }
        let host = self.host.trim();
        if host.is_empty() || host.contains(char::is_whitespace) || host.contains('/') {
            return Err(t(S::EnterAHostnameOrIPAddressWithoutAURL).into());
        }
        if self.port == 0 {
            return Err(t(S::PortMustBeBetween1And65535).into());
        }
        if self.username.chars().count() > 256 || self.username.chars().any(char::is_control) {
            return Err(t(S::UsernameMustBeASingleLineOfAtMost).into());
        }
        if self.auto_login && (self.username.trim().is_empty() || self.password_id.is_none()) {
            return Err(t(S::AutoLoginNeedsAUsernameAndASavedPassword).into());
        }
        if crate::login::sequence::compile(&self.username_prompt).is_err()
            || crate::login::sequence::compile(&self.password_prompt).is_err()
        {
            return Err(t(S::InvalidLoginPromptPatternUseASimpleRegularExpression).into());
        }
        if self.codebase.chars().count() > 100 || self.codebase.chars().any(char::is_control) {
            return Err(t(S::InvalidWorldProfile).into());
        }
        crate::channels::rules::validate(&self.channel_rules)
    }
}

/// Host names compare without case and without a trailing dot.
pub fn same_host(a: &str, b: &str) -> bool {
    a.trim_end_matches('.').eq_ignore_ascii_case(b.trim_end_matches('.'))
}

/// Saved world indices, most recently connected first; worlds never connected keep their saved
/// order after them.
pub fn recent_order(worlds: &[SavedWorld]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..worlds.len()).collect();
    // Stable: equal keys (never connected) keep the saved order.
    order.sort_by_key(|&i| std::cmp::Reverse(worlds[i].last_connected.unwrap_or(0)));
    order
}

/// Seconds since 1970 now.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    /// Scrollback rows per session.
    pub scrollback: usize,
    pub font_size: f32,
    /// Most output frames a second (0: every display frame).
    pub output_fps: u32,
    /// Show sent commands in the transcript.
    pub local_echo: bool,
    /// Reconnect automatically when a connection drops (new sessions; saved worlds have their own).
    pub auto_reconnect: bool,
    /// Treat an unterminated line that stays quiet this long as a prompt (milliseconds).
    pub prompt_quiet_ms: u64,
    pub worlds: Vec<SavedWorld>,
    /// Addresses connected to recently, newest first.
    pub recent: Vec<String>,
    /// The colour theme, by preset name (the C# client's presets; Hull is its default) or by a
    /// custom theme's id.
    pub theme: String,
    /// The window skin: `Fleet` (the default), `Armored` or `System`. Chosen independently of the
    /// colour theme; an unknown name is Fleet.
    pub skin: String,
    /// Let a world's own theme apply while one of its sessions is the active one.
    pub use_world_themes: bool,
    /// The world directory's address. Empty means the public wandur.net directory; the
    /// `WANDUR_DIRECTORY_URL` environment variable overrides both (as in the C# client).
    pub directory_url: String,
    /// Show worlds the directory marks as adult content.
    pub show_adult: bool,
    /// The UI language as a culture code (`en`, `es`, `fr`, `de`, `pt-BR`); empty follows the
    /// system language.
    pub language: String,
    /// Show the Channels panel (Settings > General); off closes it.
    pub show_channels: bool,
    /// Offer greyed completions in the command line (Settings > Input).
    pub composer_suggestions: bool,
    /// Let scripts written in Lua run (Settings > Input, a prototype, off by default);
    /// JavaScript is unaffected.
    pub enable_lua_scripts: bool,
    /// How the command line spells its own commands (Settings > Input): `/` or `#` before them,
    /// `;` or `;;` between them. A saved world may choose its own.
    #[serde(deserialize_with = "crate::command_line::lenient_style")]
    pub command_style: crate::command_line::CommandStyle,
    /// The "Looks like TinTin++ / zMUD style" tip was answered (Use # or Keep /): it never shows
    /// again.
    pub command_style_tip_answered: bool,
    /// Let MUD text blink when the server asks for it (Settings > Terminal); off shows it steady.
    pub allow_blinking_text: bool,
    /// Wrap long MUD lines at word boundaries instead of at the last column (Settings >
    /// Terminal). Display only: the server is told the real width, and copy gives the line as sent.
    pub wrap_words: bool,
    /// Indent the continuation rows of a wrapped line (Settings > Terminal).
    pub wrap_indent: bool,
    /// Keep the transcript at most `text_width_columns` wide, centred in a wider pane (Settings >
    /// Terminal). The server is told the narrower width.
    pub limit_text_width: bool,
    /// The widest transcript in columns when `limit_text_width` is on ([`TEXT_WIDTH_COLUMNS`]).
    pub text_width_columns: u32,
    /// The share of the output area the live view takes while the transcript is scrolled back,
    /// 0.1 to 0.6; 0 turns the split off.
    pub scroll_tail_share: f32,
    /// Transcript text colour (`#RRGGBB`) instead of the scheme's, or `None`.
    pub foreground: Option<String>,
    /// Transcript background (`#RRGGBB`) instead of the scheme's, or `None`.
    pub background: Option<String>,
    /// Colour schemes the person made from a preset (Settings > MUD colors > Create a copy).
    pub custom_themes: Vec<CustomTheme>,
    /// Save session history on this device (Settings > General).
    pub history_enabled: bool,
    /// Keep history this many days: 30, 90 or 365; 0 keeps it forever.
    pub history_retention_days: u32,
    /// The "Session history is saved on this device" reminder was turned off (Don't show again).
    /// Recording and its error notices are not affected.
    pub hide_history_recording_notice: bool,
    /// The map keeps the current room centred as the player moves (its toolbar toggle).
    pub map_auto_center: bool,
    /// The map editor's inspector width, closed sections and grid snap (a Rust addition).
    pub map_editor: MapEditorPrefs,
    /// Colour rooms without terrain with the local classifier, once a package is installed
    /// (Map tools, Room terrain inference; the C# default is on).
    pub classify_rooms_locally: bool,
    /// The classifier's confidence threshold, 0.5 to 0.99 (the package's 0.8 by default).
    pub room_classification_threshold: f64,
    /// Ask the directory, at most once a day, whether a newer release is out (Settings >
    /// General). Nothing is ever downloaded or installed.
    pub check_for_updates: bool,
    /// A random id for this installation (hyphenated UUID), made the first time the settings
    /// load and kept with them. It says nothing about the person or the machine; see
    /// [`crate::directory::install`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_id: Option<String>,
    /// Requests to the directory's world list and update check carry [`Self::install_id`].
    pub send_install_id: bool,
    /// When the last update check ran and what it learned, so a restart within the day does
    /// not ask again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_update_check: Option<crate::updates::UpdateCheckRecord>,
    /// A release the person chose to skip; only a newer one is offered again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped_update_version: Option<String>,
}

/// A colour scheme copied from a preset, with its own chrome and terminal colours. Selected by
/// its id in [`Settings::theme`]. (The C# `UserTheme`.)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomTheme {
    /// `custom-` and 32 hex digits.
    pub id: String,
    pub name: String,
    /// The preset it was copied from, which gives everything but the terminal colours.
    pub base: String,
    /// Terminal colours 0 to 15 that differ from [`ANSI_DEFAULTS`], as `#RRGGBB`; missing ones
    /// are the defaults (the C# rule). A copy of a preset starts with the preset's colours.
    pub ansi_colors: std::collections::BTreeMap<u8, String>,
    /// The chrome is light (Settings > Appearance > Light mode).
    pub is_light: bool,
    /// The chrome colours by [`COLOR_KEYS`], as `#RRGGBB`. Empty in schemes saved before the
    /// Appearance editor existed: those take every chrome colour from [`Self::base`].
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub colors: std::collections::BTreeMap<String, String>,
}

/// The chrome colours a custom scheme names, in the C# `UserTheme.ColorKeys` order (the order of
/// the Appearance editor's rows).
pub const COLOR_KEYS: [&str; 18] = [
    "Shell",
    "Panel",
    "Terminal",
    "Text",
    "Muted",
    "Accent",
    "AccentSecondary",
    "Border",
    "TerminalText",
    "Chrome",
    "Selection",
    "Button",
    "ButtonText",
    "PrimaryText",
    "MapBackground",
    "MapGrid",
    "EditorBackground",
    "EditorText",
];

/// The window skins, in menu order.
pub const SKINS: [&str; 3] = ["Fleet", "Armored", "System"];

/// A skin name as one of [`SKINS`]; anything else is Fleet (the C# `WindowSkinId.Normalize`).
pub fn normalize_skin(name: &str) -> &'static str {
    SKINS.iter().copied().find(|s| *s == name).unwrap_or(SKINS[0])
}

impl Default for CustomTheme {
    fn default() -> Self {
        Self {
            id: new_custom_theme_id(),
            name: String::new(),
            base: DEFAULT_THEME.into(),
            ansi_colors: Default::default(),
            is_light: false,
            colors: Default::default(),
        }
    }
}

impl CustomTheme {
    /// Keep only the colours that differ from [`ANSI_DEFAULTS`] (C# drops the others).
    pub fn set_ansi(&mut self, colors: &[String; 16]) {
        self.ansi_colors = colors
            .iter()
            .enumerate()
            .filter(|(i, c)| !c.eq_ignore_ascii_case(ANSI_DEFAULTS[*i]))
            .map(|(i, c)| (i as u8, c.to_uppercase()))
            .collect();
    }

    /// The sixteen colours, defaults filled in.
    pub fn ansi(&self) -> [String; 16] {
        std::array::from_fn(|i| {
            self.ansi_colors
                .get(&(i as u8))
                .cloned()
                .unwrap_or_else(|| ANSI_DEFAULTS[i].to_string())
        })
    }
}

/// The preset names a custom scheme may not take (the C# `UserTheme.PresetNames`).
pub const PRESET_NAMES: [&str; 11] = [
    "Hull",
    "Ember",
    "Moonlight",
    "Forest",
    "Midnight",
    "Slate",
    "Rose",
    "Paper",
    "Parchment",
    "Daylight",
    "Linen",
];

/// Custom scheme names differ from each other and from every preset, ignoring case and the
/// spaces around them (the C# rule).
pub fn unique_theme_names(themes: &[CustomTheme]) -> bool {
    let mut seen: std::collections::HashSet<String> = PRESET_NAMES.iter().map(|n| n.to_lowercase()).collect();
    themes.iter().all(|t| seen.insert(t.name.trim().to_lowercase()))
}

/// A fresh custom scheme id.
pub fn new_custom_theme_id() -> String {
    format!("custom-{}", uuid::Uuid::new_v4().simple())
}

/// Custom schemes kept at most (the C# bound).
pub const MAX_CUSTOM_THEMES: usize = 100;

/// The sixteen terminal colours a custom scheme starts its Reset buttons from (the C#
/// `AnsiPalette.Defaults`).
pub const ANSI_DEFAULTS: [&str; 16] = [
    "#16161C", "#CD3131", "#0DBC79", "#E5C07B", "#61AFEF", "#C678DD", "#56B6C2", "#D4D4D4", "#808080", "#F14C4C",
    "#23D18B", "#F5F543", "#82AAFF", "#D670D6", "#29B8DB", "#FFFFFF",
];

/// Whether `value` is a colour written as `#RRGGBB`.
pub fn is_color(value: &str) -> bool {
    value.len() == 7 && value.starts_with('#') && value[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// `#RRGGBB` as red, green and blue.
pub fn parse_color(value: &str) -> Option<(u8, u8, u8)> {
    if !is_color(value) {
        return None;
    }
    let n = u32::from_str_radix(&value[1..], 16).ok()?;
    Some(((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

impl CustomTheme {
    /// Whether the scheme is well formed (the C# `UserTheme.Validate` for what is kept here).
    pub fn is_valid(&self) -> bool {
        let id_ok = self.id.len() > 7
            && self.id.len() <= 80
            && self.id.starts_with("custom-")
            && self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        let name = self.name.trim();
        id_ok
            && !name.is_empty()
            && name.chars().count() <= 100
            && !name.chars().any(char::is_control)
            && self.ansi_colors.iter().all(|(&i, c)| i < 16 && is_color(c))
            && self.colors_valid()
    }

    /// The chrome colours are absent (taken from the base preset) or exactly [`COLOR_KEYS`], each
    /// a `#RRGGBB` colour (the C# rule for a scheme with colours).
    pub fn colors_valid(&self) -> bool {
        self.colors.is_empty()
            || (self.colors.len() == COLOR_KEYS.len()
                && COLOR_KEYS
                    .iter()
                    .all(|k| self.colors.get(*k).is_some_and(|c| is_color(c))))
    }

    /// The chrome colour `key` the scheme names, if it names its colours.
    pub fn color(&self, key: &str) -> Option<&str> {
        self.colors.get(key).map(String::as_str)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            scrollback: 2_000,
            // The C# default (ClientSettings.FontSize).
            font_size: DEFAULT_FONT_SIZE,
            output_fps: 30,
            // Off, as in C#: "Local echo starts off".
            local_echo: false,
            auto_reconnect: false,
            prompt_quiet_ms: 250,
            worlds: Vec::new(),
            recent: Vec::new(),
            theme: DEFAULT_THEME.into(),
            skin: SKINS[0].into(),
            use_world_themes: true,
            directory_url: String::new(),
            show_adult: false,
            language: String::new(),
            show_channels: true,
            composer_suggestions: true,
            enable_lua_scripts: false,
            command_style: crate::command_line::CommandStyle::Wandur,
            command_style_tip_answered: false,
            allow_blinking_text: false,
            wrap_words: true,
            wrap_indent: false,
            limit_text_width: false,
            text_width_columns: DEFAULT_TEXT_WIDTH_COLUMNS,
            scroll_tail_share: DEFAULT_TAIL_SHARE,
            foreground: None,
            background: None,
            custom_themes: Vec::new(),
            history_enabled: true,
            history_retention_days: crate::history::DEFAULT_RETENTION_DAYS,
            hide_history_recording_notice: false,
            map_auto_center: true,
            map_editor: MapEditorPrefs::default(),
            classify_rooms_locally: true,
            room_classification_threshold: crate::classify::DEFAULT_THRESHOLD,
            check_for_updates: true,
            install_id: None,
            send_install_id: true,
            last_update_check: None,
            skipped_update_version: None,
        }
    }
}

/// The session text size in points (the C# default).
pub const DEFAULT_FONT_SIZE: f32 = 15.0;

/// The C# client's default theme.
pub const DEFAULT_THEME: &str = "Hull";

/// The map editor's remembered layout: the inspector's width, the sections closed by hand and
/// whether moves snap to whole cells.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MapEditorPrefs {
    /// The inspector's width in points ([`MAP_INSPECTOR_WIDTHS`]).
    pub inspector_width: f32,
    /// Inspector sections closed by hand, by key ("room", "position", "exits", "notes", ...).
    pub closed_sections: Vec<String>,
    /// Moving rooms lands them on whole cells.
    pub snap: bool,
}

/// The inspector's width range and default, in points.
pub const MAP_INSPECTOR_WIDTHS: std::ops::RangeInclusive<f32> = 260.0..=640.0;
pub const DEFAULT_MAP_INSPECTOR_WIDTH: f32 = 320.0;

impl Default for MapEditorPrefs {
    fn default() -> Self {
        Self {
            inspector_width: DEFAULT_MAP_INSPECTOR_WIDTH,
            closed_sections: Vec::new(),
            snap: true,
        }
    }
}

impl MapEditorPrefs {
    /// Bring the values into range.
    pub fn clamp(&mut self) {
        if !self.inspector_width.is_finite() {
            self.inspector_width = DEFAULT_MAP_INSPECTOR_WIDTH;
        }
        self.inspector_width = self
            .inspector_width
            .clamp(*MAP_INSPECTOR_WIDTHS.start(), *MAP_INSPECTOR_WIDTHS.end());
        self.closed_sections.retain(|s| !s.is_empty() && s.len() <= 32);
        self.closed_sections.truncate(16);
    }

    /// Whether a section is open.
    pub fn is_open(&self, key: &str) -> bool {
        !self.closed_sections.iter().any(|s| s == key)
    }

    /// Open or close a section.
    pub fn set_open(&mut self, key: &str, open: bool) {
        self.closed_sections.retain(|s| s != key);
        if !open {
            self.closed_sections.push(key.to_string());
        }
    }
}

/// The live view's share of the output area while scrolled back (the C# default).
pub const DEFAULT_TAIL_SHARE: f32 = 0.25;
/// The live view's share, when it is on.
pub const TAIL_SHARES: std::ops::RangeInclusive<f32> = 0.1..=0.6;

/// The limited transcript width when nothing was chosen, in columns.
pub const DEFAULT_TEXT_WIDTH_COLUMNS: u32 = 100;
/// The limited transcript width at least and at most, in columns.
pub const TEXT_WIDTH_COLUMNS: std::ops::RangeInclusive<u32> = 60..=240;

/// Recent addresses kept.
pub const RECENT_LIMIT: usize = 10;

impl Settings {
    /// Bring every value into its allowed range.
    pub fn clamp(&mut self, max_scrollback: usize) {
        self.version = SETTINGS_VERSION;
        self.scrollback = self.scrollback.clamp(MIN_SCROLLBACK, max_scrollback);
        if !self.font_size.is_finite() {
            self.font_size = DEFAULT_FONT_SIZE;
        }
        self.font_size = self.font_size.clamp(*FONT_SIZES.start(), *FONT_SIZES.end());
        self.output_fps = self.output_fps.min(MAX_OUTPUT_FPS);
        self.prompt_quiet_ms = self.prompt_quiet_ms.clamp(50, 5_000);
        self.worlds.retain(|w| !w.host.trim().is_empty() && w.port != 0);
        self.recent.truncate(RECENT_LIMIT);
        self.custom_themes.retain(CustomTheme::is_valid);
        self.custom_themes.truncate(MAX_CUSTOM_THEMES);
        let mut ids = std::collections::HashSet::new();
        self.custom_themes.retain(|t| ids.insert(t.id.clone()));
        self.skin = normalize_skin(&self.skin).into();
        let custom_missing = self.theme.starts_with("custom-") && self.custom_theme().is_none();
        if self.theme.trim().is_empty() || self.theme.len() > 80 || custom_missing {
            self.theme = DEFAULT_THEME.into();
        }
        self.scroll_tail_share = clamp_tail_share(self.scroll_tail_share);
        self.text_width_columns = self
            .text_width_columns
            .clamp(*TEXT_WIDTH_COLUMNS.start(), *TEXT_WIDTH_COLUMNS.end());
        for color in [&mut self.foreground, &mut self.background] {
            if color.as_deref().is_some_and(|c| !is_color(c.trim())) {
                *color = None;
            } else if let Some(c) = color {
                *c = c.trim().to_string();
            }
        }
        if self.directory_url.len() > 512 {
            self.directory_url.clear();
        }
        self.map_editor.clamp();
        if !crate::classify::THRESHOLD_RANGE.contains(&self.room_classification_threshold) {
            self.room_classification_threshold = crate::classify::DEFAULT_THRESHOLD;
        }
        if !crate::history::valid_retention(self.history_retention_days) {
            self.history_retention_days = crate::history::DEFAULT_RETENTION_DAYS;
        }
        self.install_id = self
            .install_id
            .as_deref()
            .and_then(crate::directory::install::normalize_id);
        if self
            .skipped_update_version
            .as_deref()
            .is_some_and(|v| crate::updates::ReleaseVersion::parse(v).is_none())
        {
            self.skipped_update_version = None;
        }
        if !self.language.is_empty() {
            match crate::l10n::Language::from_code(&self.language) {
                Some(l) => self.language = l.code().to_string(),
                None => self.language.clear(),
            }
        }
    }

    /// Give every saved world without an id one: the id the database already has for its
    /// endpoint (`known`), else a new one. Returns whether anything changed.
    pub fn assign_world_ids(&mut self, known: impl Fn(&SavedWorld) -> Option<String>) -> bool {
        let mut changed = false;
        for world in &mut self.worlds {
            if !crate::db::worlds::valid_world_id(&world.world_id) {
                world.world_id = known(world).unwrap_or_else(crate::db::worlds::new_world_id);
                changed = true;
            }
        }
        changed
    }

    /// The custom scheme [`Self::theme`] names, if it names one.
    pub fn custom_theme(&self) -> Option<&CustomTheme> {
        self.custom_themes.iter().find(|t| t.id == self.theme)
    }

    /// Check what the person entered on the Terminal page; the message says what to fix (the
    /// C# rules: colours as `#RRGGBB` or blank, the live view 10 to 60 percent or off).
    pub fn validate_preferences(&self) -> Result<(), String> {
        let share = self.scroll_tail_share;
        if share != 0.0 && !(share.is_finite() && TAIL_SHARES.contains(&share)) {
            return Err(t(S::LiveViewShareMustBeBetween10And60).into());
        }
        for color in [&self.foreground, &self.background].into_iter().flatten() {
            if !is_color(color.trim()) {
                return Err(t(S::CustomColorsMustUseRRGGBBOrBeLeftBlank).into());
            }
        }
        if self.custom_themes.len() > MAX_CUSTOM_THEMES || !self.custom_themes.iter().all(CustomTheme::is_valid) {
            return Err(t(S::InvalidCustomTheme).into());
        }
        if !unique_theme_names(&self.custom_themes) {
            return Err(t(S::ThemeNameUnique).into());
        }
        if !crate::history::valid_retention(self.history_retention_days) {
            return Err(t(S::HistoryRetentionInvalid).into());
        }
        if !crate::classify::THRESHOLD_RANGE.contains(&self.room_classification_threshold) {
            return Err(t(S::MapInferenceThresholdInvalid).into());
        }
        Ok(())
    }

    /// Remember an address as the most recent.
    pub fn note_recent(&mut self, address: &str) {
        self.recent.retain(|a| a != address);
        self.recent.insert(0, address.to_string());
        self.recent.truncate(RECENT_LIMIT);
    }

    /// Load from `dir`. A missing file gives defaults; an unreadable one is renamed to
    /// `settings.json.bad` and gives defaults plus a message for the person.
    pub fn load(dir: &Path, max_scrollback: usize) -> (Settings, Option<String>) {
        let path = dir.join(SETTINGS_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Settings::fresh(), None),
            Err(e) => {
                return (Settings::fresh(), Some(tf(S::CouldNotReadFile, &[&path.display(), &e])));
            }
        };
        match serde_json::from_str::<Settings>(&text) {
            Ok(mut settings) => {
                settings.clamp(max_scrollback);
                settings.ensure_install_id();
                (settings, None)
            }
            Err(e) => {
                let aside = dir.join(format!("{SETTINGS_FILE}.bad"));
                let _ = std::fs::rename(&path, &aside);
                (
                    Settings::fresh(),
                    Some(tf(S::SettingsUnreadable, &[&e, &aside.display()])),
                )
            }
        }
    }

    /// Defaults with a new install id: what a first start (or unreadable settings) begins with.
    fn fresh() -> Self {
        let mut settings = Settings::default();
        settings.ensure_install_id();
        settings
    }

    /// Give these settings an install id if they have none. Returns whether one was made (the
    /// caller saves, so the id is kept from then on).
    pub fn ensure_install_id(&mut self) -> bool {
        if self.install_id.is_some() {
            return false;
        }
        self.install_id = Some(crate::directory::install::new_id());
        true
    }

    /// Write to `dir` atomically (temporary file, then rename). The install id already in the
    /// file wins over the one these settings carry (the C# `InstallIdentity.Keep`), so a copy
    /// made before the id existed, or carrying another one, never replaces it.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let path = dir.join(SETTINGS_FILE);
        let stored = crate::directory::install::stored_id(&path);
        let kept = crate::directory::install::keep(self, stored);
        let json = serde_json::to_string_pretty(kept.as_ref().unwrap_or(self)).map_err(std::io::Error::other)?;
        write_atomic(&path, json.as_bytes())
    }
}

/// A live view share as the settings keep it: 0 (off) below 10 percent, else 10 to 60 percent.
pub fn clamp_tail_share(share: f32) -> f32 {
    if !share.is_finite() || share < 0.095 {
        0.0
    } else {
        share.clamp(*TAIL_SHARES.start(), *TAIL_SHARES.end())
    }
}

/// Write a file atomically: a temporary file next to it (`<name>.<pid>-<n>.tmp`), flushed, then renamed
/// over the old one, so a crash never leaves half a file. Creates the directory if needed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // A name of its own per write, so two writers of one file (the background saver and a
    // synchronous save) never rename each other's temporary file away.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}-{n}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// The platform's application data directory for this app, if one can be found.
pub fn default_data_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library").join("Application Support"))
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.map(|h| h.join(".local").join("share")))
    };
    base.map(|b| b.join(APP_DIR))
}

/// Saves settings on a background thread. Each [`Saver::save`] replaces any save still waiting;
/// the thread writes the newest one after a short pause.
pub struct Saver {
    tx: Option<Sender<Settings>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Saver {
    pub fn new(dir: PathBuf) -> Self {
        let (tx, rx) = channel::<Settings>();
        let worker = thread::Builder::new()
            .name("wandur-settings".into())
            .spawn(move || saver_main(&dir, &rx))
            .ok();
        Self { tx: Some(tx), worker }
    }

    pub fn save(&self, settings: &Settings) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(settings.clone());
        }
    }

    /// Write anything pending and stop (also done on drop).
    pub fn flush(&mut self) {
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Saver {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Writes one file on a background thread, like [`Saver`] but for any bytes (the workspace
/// layout). Saves in quick succession are coalesced; the newest wins.
pub struct FileSaver {
    tx: Option<Sender<Vec<u8>>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl FileSaver {
    pub fn new(path: PathBuf) -> Self {
        let (tx, rx) = channel::<Vec<u8>>();
        let worker = thread::Builder::new()
            .name("wandur-file-saver".into())
            .spawn(move || {
                while let Ok(mut latest) = rx.recv() {
                    while let Ok(newer) = rx.recv_timeout(Duration::from_millis(200)) {
                        latest = newer;
                    }
                    if let Err(e) = write_atomic(&path, &latest) {
                        eprintln!("wandur: could not save {}: {e}", path.display());
                    }
                }
            })
            .ok();
        Self { tx: Some(tx), worker }
    }

    pub fn save(&self, bytes: Vec<u8>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(bytes);
        }
    }

    /// Write anything pending and stop (also done on drop).
    pub fn flush(&mut self) {
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for FileSaver {
    fn drop(&mut self) {
        self.flush();
    }
}

fn saver_main(dir: &Path, rx: &Receiver<Settings>) {
    while let Ok(mut latest) = rx.recv() {
        // Coalesce a burst (a slider being dragged) into one write.
        while let Ok(newer) = rx.recv_timeout(Duration::from_millis(200)) {
            latest = newer;
        }
        if let Err(e) = latest.save(dir) {
            eprintln!("wandur: could not save settings in {}: {e}", dir.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wandur-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trip_and_defaults() {
        let dir = temp_dir("roundtrip");
        let (s, warning) = Settings::load(&dir, 50_000);
        assert_eq!(
            s,
            Settings {
                install_id: s.install_id.clone(),
                ..Settings::default()
            }
        );
        assert!(warning.is_none());
        let mut s = Settings {
            scrollback: 5_000,
            auto_reconnect: true,
            ..s
        };
        s.worlds.push(SavedWorld::from_endpoint(
            "Bench",
            &"tls://127.0.0.1:4400".parse().unwrap(),
        ));
        s.note_recent("127.0.0.1:4400");
        s.save(&dir).unwrap();
        let (loaded, warning) = Settings::load(&dir, 50_000);
        assert!(warning.is_none());
        assert_eq!(loaded, s);
        assert!(loaded.worlds[0].endpoint().tls);
        assert!(!dir.join("settings.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_settings_default_and_clamp() {
        let s = Settings::default();
        assert!(s.wrap_words && !s.wrap_indent && !s.limit_text_width);
        assert_eq!(s.text_width_columns, DEFAULT_TEXT_WIDTH_COLUMNS);
        for (given, kept) in [(5, 60), (100, 100), (9_999, 240)] {
            let mut s = Settings {
                text_width_columns: given,
                ..Settings::default()
            };
            s.clamp(10_000);
            assert_eq!(s.text_width_columns, kept);
        }
    }

    #[test]
    fn values_are_clamped_and_unknown_fields_ignored() {
        let dir = temp_dir("clamp");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(SETTINGS_FILE),
            r#"{"scrollback": 999999999, "font_size": 2.0, "output_fps": 100000, "future_field": 1,
                "worlds": [{"name": "x", "host": "", "port": 4000}, {"name": "ok", "host": "h", "port": 23}]}"#,
        )
        .unwrap();
        let (s, warning) = Settings::load(&dir, 50_000);
        assert!(warning.is_none());
        assert_eq!(s.scrollback, 50_000);
        assert_eq!(s.font_size, 11.0);
        assert_eq!(s.output_fps, MAX_OUTPUT_FPS);
        assert_eq!(s.worlds.len(), 1);
        assert!(!s.local_echo && s.composer_suggestions, "missing fields take defaults");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreadable_file_is_moved_aside() {
        let dir = temp_dir("bad");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SETTINGS_FILE), "{ not json").unwrap();
        let (s, warning) = Settings::load(&dir, 50_000);
        assert_eq!(
            s,
            Settings {
                install_id: s.install_id.clone(),
                ..Settings::default()
            }
        );
        assert!(warning.unwrap().contains("unreadable"));
        assert!(dir.join("settings.json.bad").exists());
        assert!(!dir.join(SETTINGS_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saver_coalesces_and_flushes() {
        let dir = temp_dir("saver");
        let mut saver = Saver::new(dir.clone());
        for fps in 1..=20 {
            saver.save(&Settings {
                output_fps: fps,
                ..Settings::default()
            });
        }
        saver.flush();
        let (s, _) = Settings::load(&dir, 50_000);
        assert_eq!(s.output_fps, 20);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_worlds_persist_with_edits_deletes_and_recent_order() {
        let dir = temp_dir("worlds");
        let mut s = Settings::default();
        for (name, port) in [("Alpha", 4000), ("Beta", 4001), ("Gamma", 4002)] {
            let mut w = SavedWorld::from_endpoint(name, &Endpoint::new("mud.example.org", port));
            w.listing_id = format!("id-{name}");
            s.worlds.push(w);
        }
        // Edit, connect twice (Gamma last), delete Alpha.
        s.worlds[1].name = "Beta Prime".into();
        s.worlds[1].charset = Charset::Latin1;
        s.worlds[1].note_connected(100);
        s.worlds[2].note_connected(200);
        s.worlds[2].note_connected(300);
        s.worlds.remove(0);
        s.save(&dir).unwrap();
        let (loaded, _) = Settings::load(&dir, 50_000);
        assert_eq!(loaded.worlds, s.worlds);
        assert_eq!(loaded.worlds[0].name, "Beta Prime");
        assert_eq!(loaded.worlds[1].connections, 2);
        assert_eq!(loaded.worlds[1].last_connected, Some(300));
        let order: Vec<&str> = recent_order(&loaded.worlds)
            .into_iter()
            .map(|i| loaded.worlds[i].name.as_str())
            .collect();
        assert_eq!(order, ["Gamma", "Beta Prime"]);
        assert!(loaded.worlds[1].is_at(&Endpoint::new("MUD.example.org.", 4002)));
        // A settings file from Milestone 2 (no usage fields, no theme) still loads.
        std::fs::write(
            dir.join(SETTINGS_FILE),
            r#"{"version":1,"worlds":[{"name":"Old","host":"h","port":23,"tls":false,"charset":"utf8","auto_reconnect":true}]}"#,
        )
        .unwrap();
        let (old, warning) = Settings::load(&dir, 50_000);
        assert!(warning.is_none());
        assert_eq!(old.worlds[0].connections, 0);
        assert_eq!(old.theme, DEFAULT_THEME);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn world_ids_are_assigned_once_and_survive_address_edits() {
        let dir = temp_dir("world-ids");
        // A file from before world ids.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(SETTINGS_FILE),
            r#"{"version":1,"worlds":[{"name":"A","host":"a.example","port":23},{"name":"B","host":"b.example","port":23}]}"#,
        )
        .unwrap();
        let (mut s, _) = Settings::load(&dir, 50_000);
        assert!(s.worlds.iter().all(|w| w.world_id.is_empty()));
        // The database already knows B's endpoint.
        assert!(s.assign_world_ids(|w| (w.host == "b.example").then(|| "known-b".to_string())));
        assert_eq!(s.worlds[0].world_id.len(), 32);
        assert_eq!(s.worlds[1].world_id, "known-b");
        assert!(!s.assign_world_ids(|_| None), "nothing left to assign");
        let id = s.worlds[0].world_id.clone();
        // Edit host and port, as the world form does (it edits a clone of the saved world).
        let mut edited = s.worlds[0].clone();
        edited.host = "new.example".into();
        edited.port = 4000;
        s.worlds[0] = edited;
        s.save(&dir).unwrap();
        let (loaded, _) = Settings::load(&dir, 50_000);
        assert_eq!(loaded.worlds[0].world_id, id);
        assert_eq!(loaded.worlds[0].host, "new.example");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn world_validation_names_the_problem() {
        let mut w = SavedWorld::from_endpoint("", &Endpoint::new("h", 23));
        assert!(w.validate().unwrap_err().contains("name"));
        w.name = "Ok".into();
        assert!(w.validate().is_ok());
        w.host = "bad host".into();
        assert!(w.validate().is_err());
        w.host = "h".into();
        w.port = 0;
        assert!(w.validate().is_err());
    }

    #[test]
    fn file_saver_writes_the_newest() {
        let dir = temp_dir("filesaver");
        let path = dir.join("layout.json");
        let mut saver = FileSaver::new(path.clone());
        for i in 0..10 {
            saver.save(format!("{i}").into_bytes());
        }
        saver.flush();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "9");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recent_addresses_are_unique_and_bounded() {
        let mut s = Settings::default();
        for i in 0..15 {
            s.note_recent(&format!("host{i}:23"));
        }
        s.note_recent("host3:23");
        assert_eq!(s.recent.len(), RECENT_LIMIT);
        assert_eq!(s.recent[0], "host3:23");
        assert_eq!(s.recent.iter().filter(|a| *a == "host3:23").count(), 1);
    }

    #[test]
    fn data_dir_is_separate_from_the_csharp_client() {
        if let Some(dir) = default_data_dir() {
            assert!(dir.ends_with(APP_DIR));
        }
    }

    #[test]
    fn history_settings_have_the_csharp_defaults_and_choices() {
        let s = Settings::default();
        assert!(s.history_enabled);
        assert_eq!(s.history_retention_days, 30);
        assert!(!s.hide_history_recording_notice);
        let mut odd = Settings {
            history_retention_days: 45,
            ..Settings::default()
        };
        assert_eq!(odd.validate_preferences(), Err(t(S::HistoryRetentionInvalid).into()));
        odd.clamp(100_000);
        assert_eq!(odd.history_retention_days, 30);
        for days in [0, 30, 90, 365] {
            let s = Settings {
                history_retention_days: days,
                ..Settings::default()
            };
            assert!(s.validate_preferences().is_ok());
        }
        // A settings file from before history keeps recording on, for 30 days.
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert!(old.history_enabled && old.history_retention_days == 30);
    }

    #[test]
    fn terminal_and_input_settings_have_the_csharp_defaults_and_ranges() {
        let s = Settings::default();
        assert!(s.composer_suggestions);
        assert!(!s.allow_blinking_text);
        assert_eq!(s.scroll_tail_share, 0.25);
        assert_eq!((s.foreground.as_deref(), s.background.as_deref()), (None, None));
        assert_eq!(clamp_tail_share(0.05), 0.0);
        assert_eq!(clamp_tail_share(0.0), 0.0);
        assert_eq!(clamp_tail_share(0.1), 0.1);
        assert_eq!(clamp_tail_share(0.9), 0.6);
        assert_eq!(clamp_tail_share(f32::NAN), 0.0);
        let bad = Settings {
            foreground: Some("red".into()),
            ..Settings::default()
        };
        assert_eq!(
            bad.validate_preferences(),
            Err(t(S::CustomColorsMustUseRRGGBBOrBeLeftBlank).into())
        );
        let bad = Settings {
            scroll_tail_share: 0.8,
            ..Settings::default()
        };
        assert!(bad.validate_preferences().is_err());
        let good = Settings {
            foreground: Some("#A0B1c2".into()),
            scroll_tail_share: 0.0,
            ..Settings::default()
        };
        assert_eq!(good.validate_preferences(), Ok(()));
        assert_eq!(parse_color("#A0B1c2"), Some((0xA0, 0xB1, 0xC2)));
        assert_eq!(parse_color("#A0B1c"), None);
    }

    #[test]
    fn skins_default_to_fleet_and_unknown_ids_fall_back() {
        let defaults = Settings::default();
        assert_eq!(defaults.skin, "Fleet");
        assert!(defaults.use_world_themes, "world themes are on by default, as in C#");
        for (given, expected) in [
            ("Fleet", "Fleet"),
            ("Armored", "Armored"),
            ("System", "System"),
            ("armored", "Fleet"),
            ("", "Fleet"),
            ("Chrome", "Fleet"),
        ] {
            assert_eq!(normalize_skin(given), expected, "{given}");
            let mut settings = Settings {
                skin: given.into(),
                ..Settings::default()
            };
            settings.clamp(10_000);
            assert_eq!(settings.skin, expected);
        }
        // A file from before skins existed loads with Fleet and world themes on.
        let old: Settings = serde_json::from_str(r#"{"theme":"Slate"}"#).unwrap();
        assert_eq!((old.skin.as_str(), old.use_world_themes), ("Fleet", true));
    }

    #[test]
    fn a_saved_world_keeps_its_valid_theme_and_drops_a_broken_one() {
        let theme = serde_json::json!({
            "version": 1, "id": "lotj-navy-cyan-gold", "name": "Legends of the Jedi", "variant": "dark",
            "corner_radius": 3, "colors": {"shell": "#091821", "panel": "#112532", "terminal": "#070D15",
            "text": "#E4EEF5", "muted": "#9CB3C3", "accent": "#65DDEB", "accent_secondary": "#F4CD72",
            "border": "#294959", "terminal_text": "#DDE8F0"}
        });
        let world: SavedWorld =
            serde_json::from_value(serde_json::json!({"name": "LotJ", "host": "h", "port": 1, "theme": theme}))
                .unwrap();
        let parsed = world.theme.clone().unwrap();
        assert_eq!(parsed.colors.accent, "#65DDEB");
        let again: SavedWorld = serde_json::from_str(&serde_json::to_string(&world).unwrap()).unwrap();
        assert_eq!(again.theme, Some(parsed));
        let broken: SavedWorld = serde_json::from_value(
            serde_json::json!({"name": "LotJ", "host": "h", "port": 1, "theme": {"version": 1, "id": "x"}}),
        )
        .unwrap();
        assert!(broken.theme.is_none());
    }

    #[test]
    fn custom_chrome_colours_are_all_or_nothing_and_names_are_unique() {
        let mut theme = CustomTheme {
            name: "Night copy".into(),
            ..CustomTheme::default()
        };
        assert!(theme.is_valid(), "no chrome colours: they come from the base preset");
        for key in COLOR_KEYS {
            theme.colors.insert(key.into(), "#102030".into());
        }
        assert!(theme.is_valid());
        theme.colors.insert("Panel".into(), "#10203".into());
        assert!(!theme.is_valid(), "a colour that is not #RRGGBB");
        theme.colors.insert("Panel".into(), "#102030".into());
        theme.colors.remove("EditorText");
        assert!(!theme.is_valid(), "a missing key");
        theme.colors.insert("EditorText".into(), "#FFFFFF".into());
        theme.colors.insert("Sparkle".into(), "#FFFFFF".into());
        assert!(!theme.is_valid(), "an unknown key");
        theme.colors.remove("Sparkle");
        let settings = Settings {
            custom_themes: vec![theme.clone()],
            ..Settings::default()
        };
        assert_eq!(settings.validate_preferences(), Ok(()));
        let twin = CustomTheme {
            name: " night COPY ".into(),
            ..CustomTheme::default()
        };
        let preset = CustomTheme {
            name: "slate".into(),
            ..CustomTheme::default()
        };
        for themes in [vec![theme.clone(), twin], vec![preset]] {
            let settings = Settings {
                custom_themes: themes,
                ..Settings::default()
            };
            assert_eq!(settings.validate_preferences(), Err(t(S::ThemeNameUnique).into()));
        }
    }

    #[test]
    fn custom_schemes_round_trip_validate_and_fall_back() {
        let dir = temp_dir("custom-themes");
        let mut theme = CustomTheme {
            name: "Hull copy 1".into(),
            base: "Hull".into(),
            ..CustomTheme::default()
        };
        theme.ansi_colors.insert(1, "#112233".into());
        theme.ansi_colors.insert(15, "#ABCDEF".into());
        assert!(theme.is_valid());
        let mut settings = Settings {
            theme: theme.id.clone(),
            custom_themes: vec![theme.clone()],
            foreground: Some(" #102030 ".into()),
            background: Some("nonsense".into()),
            ..Settings::default()
        };
        settings.clamp(10_000);
        assert_eq!(settings.foreground.as_deref(), Some("#102030"));
        assert_eq!(settings.background, None);
        settings.save(&dir).unwrap();
        let (loaded, warning) = Settings::load(&dir, 10_000);
        assert!(warning.is_none());
        assert_eq!(loaded.custom_theme(), Some(&theme));
        // Out-of-range entries and unknown ids are dropped; the scheme falls back to Hull.
        let mut broken = theme.clone();
        broken.ansi_colors.insert(16, "#112233".into());
        assert!(!broken.is_valid());
        let mut gone = Settings {
            theme: "custom-0123456789abcdef0123456789abcdef".into(),
            custom_themes: vec![broken],
            ..Settings::default()
        };
        gone.clamp(10_000);
        assert!(gone.custom_themes.is_empty());
        assert_eq!(gone.theme, DEFAULT_THEME);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every field of the C# `ClientSettings` (main, plus `EnableLuaScripts` from the
    /// `feature/mudlet-import` branch), its Rust field and its C# default as JSON. `null` for
    /// the install id: C# makes one when the settings first load, as [`Settings::load`] does.
    const CSHARP_FIELDS: &[(&str, &str, &str)] = &[
        ("Theme", "theme", r#""Hull""#),
        ("Skin", "skin", r#""Fleet""#),
        ("Language", "language", r#""""#),
        ("FontSize", "font_size", "15"),
        ("Foreground", "foreground", "null"),
        ("Background", "background", "null"),
        ("LocalEcho", "local_echo", "false"),
        ("AllowBlinkingText", "allow_blinking_text", "false"),
        ("ScrollTailShare", "scroll_tail_share", "0.25"),
        ("UseWorldThemes", "use_world_themes", "true"),
        ("ClassifyRoomsLocally", "classify_rooms_locally", "true"),
        ("RoomClassificationThreshold", "room_classification_threshold", "0.8"),
        ("MapAutoCenter", "map_auto_center", "true"),
        ("ShowChannelsPanel", "show_channels", "true"),
        ("ComposerSuggestions", "composer_suggestions", "true"),
        ("HistoryEnabled", "history_enabled", "true"),
        ("HideHistoryRecordingNotice", "hide_history_recording_notice", "false"),
        ("CheckForUpdates", "check_for_updates", "true"),
        ("InstallId", "install_id", "null"),
        ("SendInstallId", "send_install_id", "true"),
        ("LastUpdateCheck", "last_update_check", "null"),
        ("SkippedUpdateVersion", "skipped_update_version", "null"),
        ("HistoryRetentionDays", "history_retention_days", "30"),
        ("CustomThemes", "custom_themes", "[]"),
        ("Profiles", "worlds", "[]"),
        ("EnableLuaScripts", "enable_lua_scripts", "false"),
    ];

    /// Rust settings with no C# field, and why.
    const RUST_ONLY: &[(&str, &str)] = &[
        ("version", "the settings file's format version"),
        ("scrollback", "Settings > Connections: C# has a fixed scrollback"),
        (
            "output_fps",
            "Settings > Connections: the frame cap of the Rust renderer",
        ),
        (
            "auto_reconnect",
            "Settings > Connections: C# keeps reconnect per session",
        ),
        (
            "prompt_quiet_ms",
            "Settings > Connections: the quiet-line prompt rule's delay",
        ),
        ("recent", "recently used addresses for the address box"),
        ("wrap_words", "Settings > Terminal: C# lets xterm.js wrap at the column"),
        (
            "wrap_indent",
            "Settings > Terminal: the hanging indent of wrapped lines",
        ),
        (
            "limit_text_width",
            "Settings > Terminal: a narrower transcript centred in wide panes",
        ),
        (
            "text_width_columns",
            "Settings > Terminal: the limited width in columns",
        ),
        (
            "directory_url",
            "Settings > Connections: C# reads only WANDUR_DIRECTORY_URL",
        ),
        (
            "show_adult",
            "the directory's adult filter (C# keeps it in the directory view)",
        ),
        (
            "map_editor",
            "the map editor's inspector width, closed sections and grid snap (ui/map-editor)",
        ),
        (
            "command_style",
            "Settings > Input: how the command line spells its own commands",
        ),
        (
            "command_style_tip_answered",
            "the one-time tip offering # commands was answered",
        ),
    ];

    /// The command style: Wandur's by default and in files from before it existed; the global
    /// one, the tip's answer and a world's own choice round-trip; a world without one writes none.
    #[test]
    fn the_command_style_round_trips_and_old_files_load_as_wandur() {
        use crate::command_line::CommandStyle;
        let old: Settings = serde_json::from_str(r#"{"version":1,"worlds":[{"name":"Old","host":"a.org"}]}"#).unwrap();
        assert_eq!(old.command_style, CommandStyle::Wandur);
        assert!(!old.command_style_tip_answered);
        assert_eq!(old.worlds[0].command_style, None);
        let mut settings = Settings {
            command_style: CommandStyle::TinTin,
            command_style_tip_answered: true,
            ..Settings::default()
        };
        settings.worlds.push(SavedWorld {
            command_style: Some(CommandStyle::MushSafe),
            ..SavedWorld::default()
        });
        settings.worlds.push(SavedWorld::default());
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["command_style"], "tintin");
        assert_eq!(json["worlds"][0]["command_style"], "mush-safe");
        assert!(json["worlds"][1].get("command_style").is_none());
        let back: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(back, settings);
        let later: Settings =
            serde_json::from_str(r#"{"command_style":"later","worlds":[{"command_style":7}]}"#).unwrap();
        assert_eq!(later.command_style, CommandStyle::Wandur, "a name from a later version");
        assert_eq!(later.worlds[0].command_style, None);
    }

    #[test]
    fn map_editor_prefs_round_trip_and_are_clamped() {
        let mut settings = Settings::default();
        assert_eq!(settings.map_editor.inspector_width, DEFAULT_MAP_INSPECTOR_WIDTH);
        assert!(settings.map_editor.snap && settings.map_editor.is_open("room"));
        settings.map_editor.set_open("exits", false);
        settings.map_editor.inspector_width = 420.0;
        let json = serde_json::to_string(&settings).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.map_editor, settings.map_editor);
        assert!(!back.map_editor.is_open("exits"));
        // Older files have no map_editor; odd values come back in range.
        let old: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(old.map_editor, MapEditorPrefs::default());
        let mut odd = Settings::default();
        odd.map_editor.inspector_width = f32::NAN;
        odd.map_editor.closed_sections = vec![String::new(), "x".repeat(40), "notes".into()];
        odd.clamp(10_000);
        assert_eq!(odd.map_editor.inspector_width, DEFAULT_MAP_INSPECTOR_WIDTH);
        assert_eq!(odd.map_editor.closed_sections, ["notes"]);
        odd.map_editor.inspector_width = 5_000.0;
        odd.clamp(10_000);
        assert_eq!(odd.map_editor.inspector_width, *MAP_INSPECTOR_WIDTHS.end());
    }

    /// The C# `ClientSettings` field list and defaults, compared with the Rust settings. When
    /// the exported C# client is present (`.superpowers/csharp-ref`), its source is read too, so
    /// a field added there fails this test until it is mapped here.
    #[test]
    fn settings_match_the_csharp_client_settings_fields_and_defaults() {
        let defaults = serde_json::to_value(Settings::default()).unwrap();
        let object = defaults.as_object().unwrap();
        for (csharp, rust, default) in CSHARP_FIELDS {
            let expected: serde_json::Value = serde_json::from_str(default).unwrap();
            let actual = object.get(*rust).cloned().unwrap_or(serde_json::Value::Null);
            let same = match (expected.as_f64(), actual.as_f64()) {
                (Some(a), Some(b)) => (a - b).abs() < 1e-6,
                _ => expected == actual,
            };
            assert!(same, "{csharp} ({rust}): C# {expected}, Rust {actual}");
        }
        let all = serde_json::to_value(Settings {
            install_id: Some("x".into()),
            last_update_check: Some(crate::updates::UpdateCheckRecord::default()),
            skipped_update_version: Some("0.1.6".into()),
            ..Settings::default()
        })
        .unwrap();
        let fields: Vec<&str> = all.as_object().unwrap().keys().map(String::as_str).collect();
        for field in &fields {
            assert!(
                CSHARP_FIELDS.iter().any(|(_, r, _)| r == field) || RUST_ONLY.iter().any(|(r, _)| r == field),
                "Rust field {field} is neither a C# setting nor listed as Rust-only"
            );
        }
        for (_, rust, _) in CSHARP_FIELDS {
            assert!(fields.contains(rust), "{rust} missing");
        }

        // The C# source, when it is here: every `public ... { get; init; }` of the record.
        let property =
            regex::Regex::new(r"public\s+[\w.<>?]+\s+(\w+)\s*\{\s*get;\s*init;\s*\}(?:\s*=\s*([^;]+);)?").unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.superpowers");
        for (file, branch) in [
            ("csharp-ref/src/Wandur.Core/Settings/ClientSettings.cs", false),
            ("csharp-ref-mudlet/src/Wandur.Core/Settings/ClientSettings.cs", true),
        ] {
            let Ok(source) = std::fs::read_to_string(root.join(file)) else {
                eprintln!("{file} not present; compared with the table only");
                continue;
            };
            let record = source
                .split("public sealed record ClientSettings")
                .nth(1)
                .and_then(|r| r.split("public void Validate()").next())
                .expect("the ClientSettings record");
            let mut seen = Vec::new();
            for caps in property.captures_iter(record) {
                let name = &caps[1];
                seen.push(name.to_string());
                let entry = CSHARP_FIELDS.iter().find(|(c, _, _)| *c == name);
                let Some((_, _, default)) = entry else {
                    panic!("C# setting {name} is not in the table");
                };
                // Literal defaults must agree with the table.
                if let Some(init) = caps.get(2).map(|m| m.as_str().trim()) {
                    let literal = match init {
                        "WindowSkinId.Fleet" => r#""Fleet""#.to_string(),
                        "[]" => "[]".to_string(),
                        other => other.to_string(),
                    };
                    let a: serde_json::Value = serde_json::from_str(&literal).unwrap_or(serde_json::Value::Null);
                    let b: serde_json::Value = serde_json::from_str(default).unwrap();
                    assert_eq!(a, b, "{name}: C# `{init}`");
                } else {
                    assert!(
                        ["false", "null"].contains(default),
                        "{name} has no initializer in C#, so its default is false or null, not {default}"
                    );
                }
            }
            if branch {
                // The branch export is older than main: its fields are all in the table (checked
                // above), and it adds the Lua switch.
                assert!(seen.iter().any(|s| s == "EnableLuaScripts"), "{file}");
                continue;
            }
            let mut expected: Vec<String> = CSHARP_FIELDS
                .iter()
                .map(|(c, _, _)| c.to_string())
                .filter(|c| c != "EnableLuaScripts")
                .collect();
            expected.sort();
            seen.sort();
            assert_eq!(seen, expected, "{file}");
        }
    }

    /// InstallIdentityTests.TheIdIsMadeOnceAndSurvivesRestartsAndSettingsUpdates, over
    /// `settings.json` (Rust keeps settings in the file, not in `wandur.db`).
    #[test]
    fn the_install_id_is_made_once_and_survives_restarts_and_settings_updates() {
        let dir = temp_dir("install-id");
        let (first, _) = Settings::load(&dir, 50_000);
        let id = first.install_id.clone().expect("an id on first load");
        assert!(crate::directory::install::normalize_id(&id).is_some());
        assert!(first.send_install_id);
        first.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir, 50_000).0.install_id.as_deref(), Some(id.as_str()));

        // Saving changed settings keeps it, and so does a copy made before the id existed or
        // one carrying another id.
        Settings {
            theme: "Ember".into(),
            font_size: 17.0,
            ..first.clone()
        }
        .save(&dir)
        .unwrap();
        Settings {
            theme: "Ember".into(),
            ..Settings::default()
        }
        .save(&dir)
        .unwrap();
        Settings {
            theme: "Ember".into(),
            install_id: Some(crate::directory::install::new_id()),
            ..first.clone()
        }
        .save(&dir)
        .unwrap();
        let (reloaded, _) = Settings::load(&dir, 50_000);
        assert_eq!(reloaded.install_id.as_deref(), Some(id.as_str()));
        assert_eq!(reloaded.theme, "Ember");

        // Turning sending off is a setting like any other and leaves the id alone.
        Settings {
            send_install_id: false,
            ..reloaded
        }
        .save(&dir)
        .unwrap();
        let (off, _) = Settings::load(&dir, 50_000);
        assert!(!off.send_install_id);
        assert_eq!(off.install_id.as_deref(), Some(id.as_str()));

        // Other settings, deleted or never made, get their own id.
        let other = temp_dir("install-id-other");
        assert_ne!(
            Settings::load(&other, 50_000).0.install_id.as_deref(),
            Some(id.as_str())
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn update_settings_round_trip_and_bad_values_are_dropped() {
        let dir = temp_dir("update-settings");
        let s = Settings {
            check_for_updates: false,
            last_update_check: Some(crate::updates::UpdateCheckRecord {
                checked_at: 1_791_028_800,
                version: Some("0.1.6".into()),
                page: Some(crate::updates::DOWNLOADS_PAGE.into()),
                notes: None,
            }),
            skipped_update_version: Some("0.1.6".into()),
            ..Settings::fresh()
        };
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir, 50_000).0, s);
        std::fs::write(
            dir.join(SETTINGS_FILE),
            r#"{"install_id": "not an id", "skipped_update_version": "soon"}"#,
        )
        .unwrap();
        let (loaded, warning) = Settings::load(&dir, 50_000);
        assert!(warning.is_none());
        assert_eq!(loaded.skipped_update_version, None);
        assert!(
            loaded
                .install_id
                .as_deref()
                .and_then(crate::directory::install::normalize_id)
                .is_some()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
