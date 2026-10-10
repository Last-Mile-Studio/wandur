//! World scripts in JavaScript, as the C# client runs them: one engine per script (here a
//! QuickJS runtime and context through `rquickjs`), all of a session's engines on one script
//! thread, the `mud` and `Events` host API of `docs/scripting-reference.json`, the C# limits per
//! callback, and the C# rules for privacy, rate limits and restarts.
//!
//! - [`engine`]: one script's engine: the bootstrap, the limits, load and dispatch.
//! - [`engines`]: the engines of one session, keyed by script id (the C# `ScriptEngineSet`).
//! - [`thread`]: the session's script thread: an ordered queue of requests to the engines.
//! - [`session`]: one session's scripts: what runs, statuses and logs, which events reach the
//!   scripts, what their results may do (privacy, the send limits), and the restart policy.
//! - [`completion`]: the editor's API completion (the C# `ScriptCompletionCatalog`).
//! - [`panels`]: the panels scripts declare (`mud.panel`), validated and kept per session.
//! - [`packs`]: scripts a world's directory listing supplies, installed and upgraded by version.
//! - [`lua`]: Lua scripts (the `lua` feature) on the same host API, and the Mudlet layer.
//!
//! Scripts are stored per world in `wandur.db` with the macros ([`crate::db::scripts`]).
//!
//! The engine needs the `scripting` cargo feature (on by default). Without it every script fails
//! to load with a message saying so; everything else (the library, the editor) still works.

pub mod completion;
pub mod engine;
pub mod engines;
#[cfg(feature = "lua")]
pub mod lua;
pub mod packs;
pub mod panels;
pub mod session;
pub mod thread;

use serde::Deserialize;

/// The limits of `docs/scripting-reference.json` (its `limits` object), by the same names. A
/// test keeps the two in step; the engine and the session read their values from here.
pub mod limits {
    pub const SCRIPTS_PER_WORLD: usize = crate::db::scripts::MAX_ENTRIES;
    pub const SOURCE_BYTES: usize = crate::db::scripts::MAX_SOURCE_BYTES;
    pub const HOOKS_PER_SCRIPT: usize = 256;
    pub const SEND_AND_ECHO_ACTIONS_PER_EVENT: usize = 32;
    pub const SEND_AND_ECHO_CHARACTERS_PER_EVENT: usize = 32_768;
    pub const SEND_CHARACTERS: usize = 4096;
    pub const ECHO_CHARACTERS: usize = 8192;
    pub const EVENT_CHARACTERS: usize = 32_768;
    pub const QUEUED_EVENTS_PER_SESSION: usize = 1024;
    pub const SENDS_PER_SECOND: usize = 20;
    pub const SENDS_PER_MINUTE: usize = 200;
    pub const TIMER_SECONDS_MINIMUM: u64 = 1;
    pub const TIMER_SECONDS_MAXIMUM: u64 = 2_147_483;
    pub const PANELS_PER_SCRIPT: usize = 8;
    pub const WIDGETS_PER_PANEL: usize = 64;
    pub const PANEL_ACTIONS_PER_EVENT: usize = 32;
    pub const PANEL_FOCUS_PER_SECOND: usize = 1;
    pub const FORMAT_CHARACTERS: usize = 4096;
    pub const FORMAT_DEPTH: usize = 4;
    pub const PANEL_ACTION_CHARACTERS: usize = 65_536;
    pub const PANEL_CHARACTERS_PER_EVENT: usize = 262_144;
    pub const PROPERTY_STRING_CHARACTERS: usize = 4096;
    pub const LIST_ITEMS: usize = 500;
    pub const TABLE_ROWS: usize = 500;
    pub const TABLE_COLUMNS: usize = 32;
    pub const GROUP_CHILDREN: usize = 64;
    pub const STATE_PATH_CHARACTERS: usize = 512;
    pub const STATE_ENTRIES_PER_BUCKET: usize = crate::protocol::state::MAX_ENTRIES;
    pub const STATE_VALUE_CHARACTERS: usize = crate::protocol::state::MAX_VALUE_CHARS;
    pub const STATE_BUCKET_CHARACTERS: usize = crate::protocol::state::MAX_BUCKET_CHARS;
    pub const MSDP_VARIABLES_PER_PAYLOAD: usize = crate::protocol::state::MAX_PAYLOAD_VARIABLES;
    pub const MSDP_VALUE_CHARACTERS: usize = crate::protocol::state::MAX_PAYLOAD_VALUE_CHARS;
    pub const MSDP_REPORTS_PER_SCRIPT: usize = 64;
    pub const STATE_SEED_CHARACTERS: usize = 1024 * 1024;
    pub const CALLBACK_TIMEOUT_MILLISECONDS: u64 = 300;
    pub const CALLBACK_STATEMENTS: u64 = 100_000;
    pub const WORKER_TIMEOUT_SECONDS: u64 = 2;

    /// Every limit by its reference name, for the reference test.
    pub const ALL: &[(&str, u64)] = &[
        ("scriptsPerWorld", SCRIPTS_PER_WORLD as u64),
        ("sourceBytes", SOURCE_BYTES as u64),
        ("hooksPerScript", HOOKS_PER_SCRIPT as u64),
        ("sendAndEchoActionsPerEvent", SEND_AND_ECHO_ACTIONS_PER_EVENT as u64),
        (
            "sendAndEchoCharactersPerEvent",
            SEND_AND_ECHO_CHARACTERS_PER_EVENT as u64,
        ),
        ("sendCharacters", SEND_CHARACTERS as u64),
        ("echoCharacters", ECHO_CHARACTERS as u64),
        ("eventCharacters", EVENT_CHARACTERS as u64),
        ("queuedEventsPerSession", QUEUED_EVENTS_PER_SESSION as u64),
        ("sendsPerSecond", SENDS_PER_SECOND as u64),
        ("sendsPerMinute", SENDS_PER_MINUTE as u64),
        ("timerSecondsMinimum", TIMER_SECONDS_MINIMUM),
        ("timerSecondsMaximum", TIMER_SECONDS_MAXIMUM),
        ("panelsPerScript", PANELS_PER_SCRIPT as u64),
        ("widgetsPerPanel", WIDGETS_PER_PANEL as u64),
        ("panelActionsPerEvent", PANEL_ACTIONS_PER_EVENT as u64),
        ("panelFocusPerSecond", PANEL_FOCUS_PER_SECOND as u64),
        ("formatCharacters", FORMAT_CHARACTERS as u64),
        ("formatDepth", FORMAT_DEPTH as u64),
        ("panelActionCharacters", PANEL_ACTION_CHARACTERS as u64),
        ("panelCharactersPerEvent", PANEL_CHARACTERS_PER_EVENT as u64),
        ("propertyStringCharacters", PROPERTY_STRING_CHARACTERS as u64),
        ("listItems", LIST_ITEMS as u64),
        ("tableRows", TABLE_ROWS as u64),
        ("tableColumns", TABLE_COLUMNS as u64),
        ("groupChildren", GROUP_CHILDREN as u64),
        ("statePathCharacters", STATE_PATH_CHARACTERS as u64),
        ("stateEntriesPerBucket", STATE_ENTRIES_PER_BUCKET as u64),
        ("stateValueCharacters", STATE_VALUE_CHARACTERS as u64),
        ("stateBucketCharacters", STATE_BUCKET_CHARACTERS as u64),
        ("msdpVariablesPerPayload", MSDP_VARIABLES_PER_PAYLOAD as u64),
        ("msdpValueCharacters", MSDP_VALUE_CHARACTERS as u64),
        ("msdpReportsPerScript", MSDP_REPORTS_PER_SCRIPT as u64),
        ("stateSeedCharacters", STATE_SEED_CHARACTERS as u64),
        ("callbackTimeoutMilliseconds", CALLBACK_TIMEOUT_MILLISECONDS),
        ("callbackStatements", CALLBACK_STATEMENTS),
        ("workerTimeoutSeconds", WORKER_TIMEOUT_SECONDS),
    ];
}

/// The language a world script is written in (the C# `ScriptLanguages`). Stored as NULL for
/// JavaScript, the language of every script written before Lua, and `lua` for Lua.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Language {
    #[default]
    JavaScript,
    Lua,
}

impl Language {
    /// The stored name (`javascript` or `lua`).
    pub fn as_str(self) -> &'static str {
        match self {
            Language::JavaScript => "javascript",
            Language::Lua => "lua",
        }
    }

    /// A stored name: missing or empty is JavaScript; anything unknown is refused.
    pub fn parse(stored: Option<&str>) -> Option<Self> {
        match stored.unwrap_or("") {
            "" | "javascript" => Some(Language::JavaScript),
            "lua" => Some(Language::Lua),
            _ => None,
        }
    }
}

/// An optional layer a script runs with (the C# `ScriptCompatibility`): a Mudlet-compatible Lua
/// script gets Mudlet's names (send, tempTrigger, gmcp and the rest) on top of `mud`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Compatibility {
    #[default]
    None,
    Mudlet,
}

impl Compatibility {
    /// The stored name: empty for none, `mudlet`.
    pub fn as_str(self) -> &'static str {
        match self {
            Compatibility::None => "",
            Compatibility::Mudlet => "mudlet",
        }
    }

    pub fn parse(stored: Option<&str>) -> Option<Self> {
        match stored.unwrap_or("") {
            "" => Some(Compatibility::None),
            "mudlet" => Some(Compatibility::Mudlet),
            _ => None,
        }
    }
}

/// How a script is loaded: its language and layer (the C# `ScriptLoadOptions`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Runtime {
    pub language: Language,
    pub compatibility: Compatibility,
}

impl Runtime {
    pub const JAVASCRIPT: Runtime = Runtime {
        language: Language::JavaScript,
        compatibility: Compatibility::None,
    };
    pub const LUA: Runtime = Runtime {
        language: Language::Lua,
        compatibility: Compatibility::None,
    };
    pub const MUDLET: Runtime = Runtime {
        language: Language::Lua,
        compatibility: Compatibility::Mudlet,
    };

    /// Only Lua takes a layer.
    pub fn is_valid(self) -> bool {
        self.language == Language::Lua || self.compatibility == Compatibility::None
    }

    pub fn is_lua(self) -> bool {
        self.language == Language::Lua
    }
}

/// What a load with an impossible runtime gets (the C# worker's answer).
pub const UNKNOWN_LANGUAGE: &str = "Unknown script language.";

/// What an event is. The names are the strings the engine's dispatcher takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    /// A completed public server line, without styling (`Events.Line`).
    Line,
    /// A public prompt the server marked with GA or EOR (`Events.Prompt`).
    Prompt,
    /// `Package.Name {json}` (`Events.Gmcp`).
    Gmcp,
    /// `{"variable":..,"value":..}` (`Events.Msdp`).
    Msdp,
    /// A function key (`Events.Key`, used by generated shortcut macros in C#).
    Key,
    /// A typed command, for aliases.
    Command,
    /// Time passed: timers that are due run.
    Tick,
    /// A panel widget's callback (to the script that declared the panel).
    Panel,
    /// The host's protocol cache (the seed); no callback runs.
    State,
    /// Collect what the source did at load.
    Flush,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Line => "line",
            EventKind::Prompt => "prompt",
            EventKind::Gmcp => "gmcp",
            EventKind::Msdp => "msdp",
            EventKind::Key => "key",
            EventKind::Command => "command",
            EventKind::Tick => "tick",
            EventKind::Panel => "panel",
            EventKind::State => "state",
            EventKind::Flush => "flush",
        }
    }
}

/// One event for the scripts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptEvent {
    pub kind: EventKind,
    pub text: String,
    /// For ticks: milliseconds since the script started.
    pub elapsed_ms: u64,
}

impl ScriptEvent {
    pub fn new(kind: EventKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
            elapsed_ms: 0,
        }
    }

    pub fn tick(elapsed_ms: u64) -> Self {
        Self {
            kind: EventKind::Tick,
            text: String::new(),
            elapsed_ms,
        }
    }

    /// An `Events.Msdp` event for one variable whose value is already JSON.
    pub fn msdp(variable: &str, json: &str) -> Self {
        let name = serde_json::Value::String(variable.to_string());
        Self::new(EventKind::Msdp, format!("{{\"variable\":{name},\"value\":{json}}}"))
    }
}

/// What a script asked the host to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionKind {
    Send,
    Echo,
    Panel,
    Report,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ScriptAction {
    pub kind: ActionKind,
    pub text: String,
}

impl ScriptAction {
    pub fn new(kind: ActionKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }
}

/// The outcome of one engine call. An error discards the engine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScriptResult {
    /// An alias took the command.
    pub handled: bool,
    pub actions: Vec<ScriptAction>,
    pub error: Option<String>,
    /// The script's alias count, when it changed.
    pub aliases: Option<u32>,
}

impl ScriptResult {
    pub fn failed(message: impl Into<String>) -> Self {
        let mut message: String = message.into();
        if message.len() > 2048 {
            let mut cut = 2048;
            while !message.is_char_boundary(cut) {
                cut -= 1;
            }
            message.truncate(cut);
        }
        Self {
            error: Some(message),
            ..Self::default()
        }
    }

    /// Nothing to apply: not handled, no actions, no error, no news.
    pub fn is_empty(&self) -> bool {
        !self.handled && self.actions.is_empty() && self.error.is_none() && self.aliases.is_none()
    }
}

/// An MSDP variable name the client will put in a REPORT request: letters, digits and
/// underscore, 1 to 128 characters, not starting with a digit.
pub fn is_valid_msdp_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=128).contains(&bytes.len())
        && !bytes[0].is_ascii_digit()
        && bytes.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// Widget kinds a script may declare on a panel (the reference is checked against this list).
pub const PANEL_WIDGET_KINDS: &[&str] = &[
    "gauge",
    "label",
    "text",
    "list",
    "table",
    "button",
    "toggle",
    "input",
    "separator",
    "group",
];

/// Length in UTF-16 units (JavaScript string length), without counting when it cannot matter.
pub(crate) fn js_length(text: &str) -> usize {
    if text.is_ascii() {
        text.len()
    } else {
        text.encode_utf16().count()
    }
}
