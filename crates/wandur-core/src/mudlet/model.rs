//! What a Mudlet profile or package holds, as far as an import can use it (the C#
//! `MudletModel`). Everything here is untrusted text from the file.

use std::fmt;

use crate::l10n::{S, t};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ItemKind {
    Trigger,
    Alias,
    Timer,
    Key,
    Script,
    Button,
}

impl ItemKind {
    pub const ALL: [ItemKind; 6] = [
        ItemKind::Trigger,
        ItemKind::Alias,
        ItemKind::Timer,
        ItemKind::Key,
        ItemKind::Script,
        ItemKind::Button,
    ];

    /// The stable name kept on an imported item (`trigger`, `alias` and so on).
    pub fn key(self) -> &'static str {
        match self {
            ItemKind::Trigger => "trigger",
            ItemKind::Alias => "alias",
            ItemKind::Timer => "timer",
            ItemKind::Key => "key",
            ItemKind::Script => "script",
            ItemKind::Button => "button",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.key().eq_ignore_ascii_case(key))
    }

    /// The plural label the summary and the generated comments use ("Triggers").
    pub fn label(self) -> &'static str {
        t(match self {
            ItemKind::Trigger => S::MudletKindTriggers,
            ItemKind::Alias => S::MudletKindAliases,
            ItemKind::Timer => S::MudletKindTimers,
            ItemKind::Key => S::MudletKindKeys,
            ItemKind::Script => S::MudletKindScripts,
            ItemKind::Button => S::MudletKindButtons,
        })
    }
}

/// One trigger pattern as Mudlet stores it: the pattern text and its numeric type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    pub kind: i32,
    pub text: String,
}

impl Pattern {
    pub const SUBSTRING: i32 = 0;
    pub const REGEX: i32 = 1;
    pub const BEGINNING_OF_LINE: i32 = 2;
    pub const EXACT: i32 = 3;
    pub const LUA_FUNCTION: i32 = 4;
    pub const LINE_SPACER: i32 = 5;
    pub const COLOUR: i32 = 6;
    pub const PROMPT: i32 = 7;

    pub fn new(kind: i32, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }

    /// A short stable name, used in the kept record of an item and in the import summary.
    pub fn type_name(&self) -> String {
        match self.kind {
            Self::SUBSTRING => "substring".into(),
            Self::REGEX => "regex".into(),
            Self::BEGINNING_OF_LINE => "begin".into(),
            Self::EXACT => "exact".into(),
            Self::LUA_FUNCTION => "lua".into(),
            Self::LINE_SPACER => "spacer".into(),
            Self::COLOUR => "colour".into(),
            Self::PROMPT => "prompt".into(),
            other => format!("type-{other}"),
        }
    }
}

/// One item of a Mudlet package (trigger, alias, timer, key, script or button), or a folder of
/// them. Only the fields an import can use are kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub kind: ItemKind,
    pub name: String,
    pub is_folder: bool,
    pub is_active: bool,
    pub script: String,
    pub command: String,
    /// Trigger patterns, in order.
    pub patterns: Vec<Pattern>,
    /// The alias pattern, always a Perl-style regular expression in Mudlet.
    pub alias_pattern: String,
    /// A timer's interval as Mudlet writes it, hh:mm:ss.zzz.
    pub time: String,
    pub key_code: i64,
    pub key_modifier: i64,
    pub event_handlers: Vec<String>,
    pub is_multiline: bool,
    pub is_filter: bool,
    pub is_highlight: bool,
    pub is_sound: bool,
    pub is_colour_trigger: bool,
    pub is_temporary: bool,
    pub is_offset_timer: bool,
    pub children: Vec<Item>,
}

impl Item {
    pub fn new(kind: ItemKind) -> Self {
        Self {
            kind,
            name: String::new(),
            is_folder: false,
            is_active: true,
            script: String::new(),
            command: String::new(),
            patterns: Vec::new(),
            alias_pattern: String::new(),
            time: String::new(),
            key_code: 0,
            key_modifier: 0,
            event_handlers: Vec::new(),
            is_multiline: false,
            is_filter: false,
            is_highlight: false,
            is_sound: false,
            is_colour_trigger: false,
            is_temporary: false,
            is_offset_timer: false,
            children: Vec::new(),
        }
    }
}

/// The contents of one MudletPackage document: a whole profile or one package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub host_name: String,
    pub url: String,
    pub port: Option<u16>,
    pub tls: Option<bool>,
    pub command_separator: String,
    pub modules: Vec<String>,
    pub triggers: Vec<Item>,
    pub aliases: Vec<Item>,
    pub timers: Vec<Item>,
    pub keys: Vec<Item>,
    pub scripts: Vec<Item>,
    pub buttons: Vec<Item>,
    pub variable_count: usize,
    pub has_host: bool,
}

impl Default for Package {
    fn default() -> Self {
        Self {
            host_name: String::new(),
            url: String::new(),
            port: None,
            tls: None,
            command_separator: ";;".into(),
            modules: Vec::new(),
            triggers: Vec::new(),
            aliases: Vec::new(),
            timers: Vec::new(),
            keys: Vec::new(),
            scripts: Vec::new(),
            buttons: Vec::new(),
            variable_count: 0,
            has_host: false,
        }
    }
}

/// What was read from the place the person picked: a profile folder, a profile or package
/// file, or a zipped package. Connection details come from the profile's own files where
/// Mudlet keeps them there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Source {
    /// The profile or package name: it names the world and recognises a later import.
    pub name: String,
    pub is_profile: bool,
    pub host: String,
    pub port: Option<u16>,
    pub tls: Option<bool>,
    pub login: String,
    pub encoding: String,
    /// A stored password existed. It is never opened or imported.
    pub password_skipped: bool,
    pub packages: Vec<Package>,
}

/// The file could not be imported; the message is already in the UI language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportError(pub String);

impl ImportError {
    pub fn new(key: S) -> Self {
        Self(t(key).to_string())
    }
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ImportError {}
