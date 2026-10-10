//! The no-code macro data, as the C# `MacroDefinition` record: a kind (trigger, alias, timer,
//! function key), a literal pattern, how a trigger matches, case, an interval, and one command per
//! line. Validation follows the C# `MacroCompiler` limits and messages. The C# client turns each
//! macro into JavaScript; [`MacroDefinition::compile_javascript`] produces the same text, so the
//! `scripts` table keeps the C# shape (source plus `macro_json`), while this client runs macros
//! natively (see [`super::rules`]).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::l10n::{S, t};

/// Most command lines a macro sends.
pub const MAX_COMMANDS: usize = 20;
/// Longest pattern and longest command line, in UTF-16 units (the C# string length).
pub const MAX_LINE: usize = 1024;
/// Longest command text in all, in UTF-16 units.
pub const MAX_COMMAND_TEXT: usize = 8192;
/// Timer intervals, in seconds (one second to a day).
pub const MIN_INTERVAL: u32 = 1;
pub const MAX_INTERVAL: u32 = 86_400;
/// The default interval of a new timer.
pub const DEFAULT_INTERVAL: u32 = 30;
/// The function keys a shortcut can use.
pub const SHORTCUT_KEYS: [&str; 12] = [
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
];

/// When a macro runs. Stored as its C# number (0 to 3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "u8", try_from = "u8")]
pub enum MacroKind {
    /// A complete received line matches ("Text received").
    #[default]
    Trigger,
    /// The whole typed command equals the pattern ("Command alias").
    Alias,
    /// Every interval while running ("Repeating timer").
    Timer,
    /// A function key in the command input ("Keyboard shortcut").
    Shortcut,
}

impl MacroKind {
    pub const ALL: [MacroKind; 4] = [
        MacroKind::Trigger,
        MacroKind::Alias,
        MacroKind::Timer,
        MacroKind::Shortcut,
    ];

    pub fn label(self) -> &'static str {
        t(match self {
            MacroKind::Trigger => S::MacroTrigger,
            MacroKind::Alias => S::MacroAlias,
            MacroKind::Timer => S::MacroTimer,
            MacroKind::Shortcut => S::MacroShortcut,
        })
    }
}

impl From<MacroKind> for u8 {
    fn from(kind: MacroKind) -> u8 {
        kind as u8
    }
}

impl TryFrom<u8> for MacroKind {
    type Error = String;
    fn try_from(n: u8) -> Result<Self, String> {
        MacroKind::ALL
            .get(n as usize)
            .copied()
            .ok_or_else(|| t(S::MacroInvalid).into())
    }
}

/// How a trigger's pattern meets the line. Stored as its C# number (0 to 2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "u8", try_from = "u8")]
pub enum MacroMatch {
    #[default]
    Contains,
    StartsWith,
    Exact,
}

impl MacroMatch {
    pub const ALL: [MacroMatch; 3] = [MacroMatch::Contains, MacroMatch::StartsWith, MacroMatch::Exact];

    pub fn label(self) -> &'static str {
        t(match self {
            MacroMatch::Contains => S::MacroContains,
            MacroMatch::StartsWith => S::MacroStartsWith,
            MacroMatch::Exact => S::MacroExact,
        })
    }
}

impl From<MacroMatch> for u8 {
    fn from(m: MacroMatch) -> u8 {
        m as u8
    }
}

impl TryFrom<u8> for MacroMatch {
    type Error = String;
    fn try_from(n: u8) -> Result<Self, String> {
        MacroMatch::ALL
            .get(n as usize)
            .copied()
            .ok_or_else(|| t(S::MacroInvalid).into())
    }
}

/// Why a macro cannot be saved. The message is the C# one, in the UI language.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacroError {
    Invalid,
    InvalidPattern,
    InvalidCommands,
    InvalidInterval,
    InvalidShortcut,
}

impl fmt::Display for MacroError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(t(match self {
            MacroError::Invalid => S::MacroInvalid,
            MacroError::InvalidPattern => S::MacroInvalidPattern,
            MacroError::InvalidCommands => S::MacroInvalidCommands,
            MacroError::InvalidInterval => S::MacroInvalidInterval,
            MacroError::InvalidShortcut => S::MacroInvalidShortcut,
        }))
    }
}

impl std::error::Error for MacroError {}

/// One macro. Field names on disk are the C# ones (`macro_json` in the `scripts` table).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MacroDefinition {
    #[serde(rename = "Kind")]
    pub kind: MacroKind,
    #[serde(rename = "Pattern")]
    pub pattern: String,
    /// One command per line.
    #[serde(rename = "Commands")]
    pub commands: String,
    #[serde(rename = "Match", default)]
    pub match_kind: MacroMatch,
    #[serde(rename = "IgnoreCase", default)]
    pub ignore_case: bool,
    #[serde(rename = "IntervalSeconds", default = "default_interval")]
    pub interval_seconds: u32,
}

fn default_interval() -> u32 {
    DEFAULT_INTERVAL
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

impl MacroDefinition {
    pub fn new(kind: MacroKind, pattern: &str, commands: &str) -> Self {
        Self {
            kind,
            pattern: pattern.into(),
            commands: commands.into(),
            match_kind: MacroMatch::Contains,
            ignore_case: false,
            interval_seconds: DEFAULT_INTERVAL,
        }
    }

    /// What the + button adds (the C# `AddMacro`): a trigger on "You are hungry" that eats bread.
    pub fn starter() -> Self {
        Self::new(MacroKind::Trigger, "You are hungry", "eat bread")
    }

    pub fn with_match(mut self, m: MacroMatch) -> Self {
        self.match_kind = m;
        self
    }

    pub fn ignoring_case(mut self) -> Self {
        self.ignore_case = true;
        self
    }

    pub fn every(mut self, seconds: u32) -> Self {
        self.interval_seconds = seconds;
        self
    }

    /// The command lines in order (CR LF read as LF). Only meaningful once validated.
    pub fn command_lines(&self) -> Vec<String> {
        self.commands
            .replace("\r\n", "\n")
            .split('\n')
            .map(str::to_owned)
            .collect()
    }

    /// Check the C# limits; the error names what to fix.
    pub fn validate(&self) -> Result<(), MacroError> {
        if utf16_len(&self.pattern) > MAX_LINE || self.pattern.chars().any(char::is_control) {
            return Err(MacroError::InvalidPattern);
        }
        let lines = self.command_lines();
        let bad_line = |line: &String| {
            line.trim().is_empty()
                || utf16_len(line) > MAX_LINE
                || line
                    .chars()
                    .any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}')
        };
        if lines.is_empty()
            || lines.len() > MAX_COMMANDS
            || lines.iter().any(bad_line)
            || utf16_len(&self.commands) > MAX_COMMAND_TEXT
        {
            return Err(MacroError::InvalidCommands);
        }
        match self.kind {
            MacroKind::Timer if !(MIN_INTERVAL..=MAX_INTERVAL).contains(&self.interval_seconds) => {
                Err(MacroError::InvalidInterval)
            }
            MacroKind::Timer => Ok(()),
            MacroKind::Shortcut if !SHORTCUT_KEYS.contains(&self.pattern.as_str()) => Err(MacroError::InvalidShortcut),
            MacroKind::Shortcut => Ok(()),
            MacroKind::Trigger | MacroKind::Alias if self.pattern.trim().is_empty() => Err(MacroError::InvalidPattern),
            MacroKind::Trigger | MacroKind::Alias => Ok(()),
        }
    }

    /// The pattern as an ECMAScript regular expression source, as the C# client builds it:
    /// metacharacters escaped (and nothing else), anchored for aliases, exact and starts-with.
    pub fn regex_source(&self) -> String {
        let mut source = String::with_capacity(self.pattern.len() + 2);
        let anchored_start = self.kind == MacroKind::Alias || self.match_kind != MacroMatch::Contains;
        let anchored_end = self.kind == MacroKind::Alias || self.match_kind == MacroMatch::Exact;
        if anchored_start {
            source.push('^');
        }
        for c in self.pattern.chars() {
            if ".*+?^${}()|[]\\".contains(c) {
                source.push('\\');
            }
            source.push(c);
        }
        if anchored_end {
            source.push('$');
        }
        source
    }

    /// The JavaScript the C# client generates for this macro (byte for byte), stored as the
    /// script's source so the `scripts` table reads the same in both clients.
    pub fn compile_javascript(&self) -> Result<String, MacroError> {
        self.validate()?;
        let actions = self
            .command_lines()
            .iter()
            .map(|c| format!("mud.send({});", js_string(c)))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(match self.kind {
            MacroKind::Timer => format!("mud.every({}, () => {{\n{actions}\n}});", self.interval_seconds),
            MacroKind::Shortcut => format!(
                "mud.on(Events.Key, event => {{ if (event.text === {}) {{\n{actions}\n}} }});",
                js_string(&self.pattern)
            ),
            MacroKind::Trigger | MacroKind::Alias => {
                let method = if self.kind == MacroKind::Alias {
                    "alias"
                } else {
                    "trigger"
                };
                format!(
                    "mud.{method}(new RegExp({}, {}), () => {{\n{actions}\n}});",
                    js_string(&self.regex_source()),
                    js_string(if self.ignore_case { "i" } else { "" })
                )
            }
        })
    }
}

/// A JSON string literal escaped as .NET's `System.Text.Json` does by default: only printable
/// ASCII stays as it is, except `"` `&` `'` `+` `<` `>` and the backtick, which become `\uXXXX`
/// like every other character outside that range; `\` and the usual control escapes are short.
pub fn js_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for unit in text.encode_utf16() {
        match unit {
            0x5C => out.push_str("\\\\"),
            0x0A => out.push_str("\\n"),
            0x0D => out.push_str("\\r"),
            0x09 => out.push_str("\\t"),
            0x08 => out.push_str("\\b"),
            0x0C => out.push_str("\\f"),
            0x22 | 0x26 | 0x27 | 0x2B | 0x3C | 0x3E | 0x60 => out.push_str(&format!("\\u{unit:04X}")),
            0x20..=0x7E => out.push(unit as u8 as char),
            _ => out.push_str(&format!("\\u{unit:04X}")),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four C# `InvalidRulesAreRejectedBeforeTheyCanReplaceSavedRules` cases, plus the limits.
    #[test]
    fn invalid_rules_are_rejected() {
        assert_eq!(
            MacroDefinition::new(MacroKind::Trigger, "", "look").validate(),
            Err(MacroError::InvalidPattern)
        );
        assert_eq!(
            MacroDefinition::new(MacroKind::Timer, "", "look").every(0).validate(),
            Err(MacroError::InvalidInterval)
        );
        assert_eq!(
            MacroDefinition::new(MacroKind::Shortcut, "Enter", "look").validate(),
            Err(MacroError::InvalidShortcut)
        );
        assert_eq!(
            MacroDefinition::new(MacroKind::Alias, "h", "say hi\u{1b}").validate(),
            Err(MacroError::InvalidCommands)
        );
        // Limits: 20 lines, 1,024 per line and pattern, 8,192 in all, no blank lines.
        let ok = |commands: &str| MacroDefinition::new(MacroKind::Alias, "h", commands).validate();
        assert!(ok(&vec!["look"; 20].join("\n")).is_ok());
        assert!(ok(&vec!["look"; 21].join("\n")).is_err());
        assert!(ok(&"x".repeat(1024)).is_ok());
        assert!(ok(&"x".repeat(1025)).is_err());
        assert!(ok(&vec!["y".repeat(1000); 9].join("\n")).is_err(), "over 8,192 in all");
        assert!(ok("look\n\nscore").is_err());
        assert!(ok("look\r\nscore").is_ok(), "CR LF is a line break");
        assert!(ok("say a\u{2028}b").is_err());
        assert!(
            MacroDefinition::new(MacroKind::Trigger, &"p".repeat(1025), "look")
                .validate()
                .is_err()
        );
        assert!(
            MacroDefinition::new(MacroKind::Trigger, "a\tb", "look")
                .validate()
                .is_err()
        );
        assert!(
            MacroDefinition::new(MacroKind::Trigger, "   ", "look")
                .validate()
                .is_err()
        );
        // A timer needs no pattern; 1 and 86,400 seconds are the bounds.
        assert!(
            MacroDefinition::new(MacroKind::Timer, "", "look")
                .every(1)
                .validate()
                .is_ok()
        );
        assert!(
            MacroDefinition::new(MacroKind::Timer, "", "look")
                .every(86_400)
                .validate()
                .is_ok()
        );
        assert!(
            MacroDefinition::new(MacroKind::Timer, "", "look")
                .every(86_401)
                .validate()
                .is_err()
        );
        for key in SHORTCUT_KEYS {
            assert!(
                MacroDefinition::new(MacroKind::Shortcut, key, "look")
                    .validate()
                    .is_ok()
            );
        }
        assert!(
            MacroDefinition::new(MacroKind::Shortcut, "F13", "look")
                .validate()
                .is_err()
        );
        assert!(
            MacroDefinition::new(MacroKind::Shortcut, "f4", "look")
                .validate()
                .is_err()
        );
    }

    /// Pattern escaping and the generated JavaScript are the C# `MacroCompiler` output.
    #[test]
    fn escaping_and_generated_javascript_match_the_csharp_compiler() {
        let hunger = MacroDefinition::new(
            MacroKind::Trigger,
            "[Hungry]",
            "eat bread\nsay \"thanks\"; mud.send('oops')",
        )
        .ignoring_case();
        assert_eq!(hunger.regex_source(), "\\[Hungry\\]");
        assert_eq!(
            hunger.compile_javascript().unwrap(),
            "mud.trigger(new RegExp(\"\\\\[Hungry\\\\]\", \"i\"), () => {\n\
             mud.send(\"eat bread\");\n\
             mud.send(\"say \\u0022thanks\\u0022; mud.send(\\u0027oops\\u0027)\");\n});"
        );
        let all = MacroDefinition::new(MacroKind::Trigger, r".*+?^${}()|[]\ -/", "x");
        assert_eq!(all.regex_source(), r"\.\*\+\?\^\$\{\}\(\)\|\[\]\\ -/");
        let alias = MacroDefinition::new(MacroKind::Alias, "h+", "look");
        assert_eq!(alias.regex_source(), "^h\\+$");
        assert_eq!(
            alias.compile_javascript().unwrap(),
            "mud.alias(new RegExp(\"^h\\\\\\u002B$\", \"\"), () => {\nmud.send(\"look\");\n});"
        );
        let starts = MacroDefinition::new(MacroKind::Trigger, "You", "x").with_match(MacroMatch::StartsWith);
        assert_eq!(starts.regex_source(), "^You");
        let exact = MacroDefinition::new(MacroKind::Trigger, "You", "x").with_match(MacroMatch::Exact);
        assert_eq!(exact.regex_source(), "^You$");
        assert_eq!(
            MacroDefinition::new(MacroKind::Timer, "", "score")
                .every(10)
                .compile_javascript()
                .unwrap(),
            "mud.every(10, () => {\nmud.send(\"score\");\n});"
        );
        assert_eq!(
            MacroDefinition::new(MacroKind::Shortcut, "F4", "score")
                .compile_javascript()
                .unwrap(),
            "mud.on(Events.Key, event => { if (event.text === \"F4\") {\nmud.send(\"score\");\n} });"
        );
        assert_eq!(
            js_string("é<>&`\u{1F600}"),
            "\"\\u00E9\\u003C\\u003E\\u0026\\u0060\\uD83D\\uDE00\""
        );
    }

    #[test]
    fn stored_json_uses_the_csharp_names_and_numbers() {
        let m = MacroDefinition::new(MacroKind::Trigger, "[Hungry]", "eat bread").ignoring_case();
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(
            json,
            r#"{"Kind":0,"Pattern":"[Hungry]","Commands":"eat bread","Match":0,"IgnoreCase":true,"IntervalSeconds":30}"#
        );
        assert_eq!(serde_json::from_str::<MacroDefinition>(&json).unwrap(), m);
        let csharp = r#"{"Kind":3,"Pattern":"F4","Commands":"score"}"#;
        let read: MacroDefinition = serde_json::from_str(csharp).unwrap();
        assert_eq!(read.kind, MacroKind::Shortcut);
        assert_eq!(read.interval_seconds, 30);
        assert!(serde_json::from_str::<MacroDefinition>(r#"{"Kind":9,"Pattern":"","Commands":"x"}"#).is_err());
    }
}
