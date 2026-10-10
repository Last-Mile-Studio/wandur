//! The command line's shorthand, in the person's chosen [`CommandStyle`]. In Wandur's own style
//! `;` separates commands (`get all;wear all`), `\;` is a literal semicolon, and `/N command` sends
//! a command N times (`/10 say 1`). Braces group a repeat: `/3 {get coin;put coin bag}` repeats
//! both, while `/3 get coin;look` repeats `get coin` and sends `look` once. `/wait {text}` holds
//! the rest until a line from the world contains the text, at most 60 seconds (`/wait 120 {text}`
//! for longer), and `/wait 2` pauses two seconds, so a repeat can wait for each round to finish:
//! `/10 {say 1;kill droid;/wait {the droid is dead}}`. A `/` not followed by a count or `wait` and
//! a space is ordinary text and goes to the world unchanged; a line that starts `//` goes with one
//! `/` removed and nothing else applied (`//me waves` sends `/me waves`).
//!
//! The TinTin++ style (as zMUD and CMUD before it) spells the same with `#` (`#10 say 1`,
//! `#wait 2`, `##` to send a `#`). The MUSH-safe style keeps `/` but separates commands with `;;`,
//! because on a MUSH a `;` starts a pose: there a single `;` is ordinary text, and `\;` is still a
//! literal `;` (so `\;;` sends `;;`).

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The most times one `/N` may repeat.
pub const MAX_REPEAT: u32 = 100;
/// The most commands (and waits) one typed line may expand to.
pub const MAX_COMMANDS: usize = 200;
/// The longest `/wait`, in seconds.
pub const MAX_WAIT_SECS: u64 = 600;
/// How long `/wait {text}` waits when no time is given.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(60);

/// How the command line spells its own commands: the character that starts one and what
/// separates commands in a chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Syntax {
    /// `/` or `#`. Always ASCII.
    pub command: char,
    /// `;` or `;;`.
    pub separator: &'static str,
}

impl Syntax {
    pub const WANDUR: Syntax = Syntax {
        command: '/',
        separator: ";",
    };
    pub const TINTIN: Syntax = Syntax {
        command: '#',
        separator: ";",
    };
    pub const MUSH_SAFE: Syntax = Syntax {
        command: '/',
        separator: ";;",
    };
}

/// The command styles offered in Settings > Input and the world editor, kept in the settings by
/// a stable name ([`Self::name`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CommandStyle {
    /// `/` commands, `;` chains (the default).
    #[default]
    Wandur,
    /// `#` commands, `;` chains: TinTin++, zMUD and CMUD.
    TinTin,
    /// `/` commands, `;;` chains: a MUSH or MUX, where `;` starts a pose.
    MushSafe,
}

impl CommandStyle {
    pub const ALL: [CommandStyle; 3] = [CommandStyle::Wandur, CommandStyle::TinTin, CommandStyle::MushSafe];

    /// The name the settings file keeps.
    pub fn name(self) -> &'static str {
        match self {
            CommandStyle::Wandur => "wandur",
            CommandStyle::TinTin => "tintin",
            CommandStyle::MushSafe => "mush-safe",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|s| s.name().eq_ignore_ascii_case(name.trim()))
    }

    pub fn syntax(self) -> Syntax {
        match self {
            CommandStyle::Wandur => Syntax::WANDUR,
            CommandStyle::TinTin => Syntax::TINTIN,
            CommandStyle::MushSafe => Syntax::MUSH_SAFE,
        }
    }
}

impl Serialize for CommandStyle {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for CommandStyle {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = String::deserialize(d)?;
        Self::from_name(&name)
            .ok_or_else(|| serde::de::Error::unknown_variant(&name, &["wandur", "tintin", "mush-safe"]))
    }
}

/// The global style from the settings file: a name this build does not know (a later version's)
/// is the default rather than an unreadable file.
pub fn lenient_style<'de, D: Deserializer<'de>>(d: D) -> Result<CommandStyle, D::Error> {
    Ok(lenient_override(d)?.unwrap_or_default())
}

/// A world's own style from the settings file: an unknown name is no override.
pub fn lenient_override<'de, D: Deserializer<'de>>(d: D) -> Result<Option<CommandStyle>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(d)?;
    Ok(value
        .as_ref()
        .and_then(serde_json::Value::as_str)
        .and_then(CommandStyle::from_name))
}

/// Whether a codebase is a MUSH or MUX family (PennMUSH, TinyMUSH, TinyMUX, RhostMUSH...), where
/// `;` starts a pose.
pub fn is_mush_family(codebase: &str) -> bool {
    let lower = codebase.to_lowercase();
    lower.contains("mush") || lower.contains("mux")
}

/// The style a session uses: the world's own choice, else MUSH-safe on a MUSH or MUX (by the
/// codebase its listing or its server gave; `None` for a session not opened from a saved world),
/// else the global setting.
pub fn resolve(world: Option<CommandStyle>, codebase: Option<&str>, global: CommandStyle) -> CommandStyle {
    world.unwrap_or(if codebase.is_some_and(is_mush_family) {
        CommandStyle::MushSafe
    } else {
        global
    })
}

/// One step of a typed line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// A command, sent as if typed alone.
    Send(String),
    /// Hold the rest until a line from the world contains `text` (any case), giving up after
    /// `timeout`.
    WaitText { text: String, timeout: Duration },
    /// Hold the rest this long.
    WaitTime(Duration),
}

/// Why a line could not be expanded; nothing is sent. The person's text for it is the app's
/// (localized, naming the active command character).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExpandError {
    /// `/0 ...` or a count over [`MAX_REPEAT`].
    BadCount(String),
    /// `/3 {...` with no closing brace.
    Unclosed,
    /// More than [`MAX_COMMANDS`] in all.
    TooMany,
    /// `/wait` without text or with seconds outside 1 to [`MAX_WAIT_SECS`].
    BadWait,
}

impl fmt::Display for ExpandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadCount(n) => write!(f, "{n}: a repeat count runs from 1 to {MAX_REPEAT}"),
            Self::Unclosed => write!(f, "a {{ after a repeat count needs its closing }}"),
            Self::TooMany => write!(f, "that line makes more than {MAX_COMMANDS} commands"),
            Self::BadWait => write!(f, "wait takes seconds from 1 to {MAX_WAIT_SECS}, or text"),
        }
    }
}

/// The steps a typed line stands for, in order. A line without shorthand comes back as itself,
/// exactly (spacing kept); pieces of a chain are trimmed and empty ones dropped. A line that
/// starts with the command character twice comes back once, with one of them removed.
pub fn expand(line: &str, syntax: Syntax) -> Result<Vec<Step>, ExpandError> {
    let start = line.trim_start();
    if let Some(rest) = literal(start, syntax) {
        let lead = line.len() - start.len();
        return Ok(vec![Step::Send(format!("{}{rest}", &line[..lead]))]);
    }
    if !line.contains(syntax.separator)
        && !line.contains("\\;")
        && repeat_prefix(start, syntax).is_none()
        && wait_prefix(start, syntax).is_none()
    {
        return Ok(vec![Step::Send(line.to_string())]);
    }
    let mut out = Vec::new();
    sequence(line, syntax, &mut out)?;
    Ok(out)
}

/// Whether the line would use one of the client's commands (a repeat or a wait) in this style,
/// anywhere in its chain. A line that starts with the command character twice uses none.
pub fn uses_commands(line: &str, syntax: Syntax) -> bool {
    let mut s = line.trim_start();
    if literal(s, syntax).is_some() {
        return false;
    }
    while !s.is_empty() {
        if repeat_prefix(s, syntax).is_some() || wait_prefix(s, syntax).is_some() {
            return true;
        }
        s = piece(s, syntax).1.trim_start();
    }
    false
}

/// The rest of a line that starts with the command character twice, after the first.
fn literal(start: &str, syntax: Syntax) -> Option<&str> {
    let rest = start.strip_prefix(syntax.command)?;
    rest.starts_with(syntax.command).then_some(rest)
}

/// `/N` and the whitespace after it, when a command follows: the count's digits and the rest.
fn repeat_prefix(piece: &str, syntax: Syntax) -> Option<(&str, &str)> {
    let after_char = piece.strip_prefix(syntax.command)?;
    let digits_end = after_char.find(|c: char| !c.is_ascii_digit())?;
    let digits = &after_char[..digits_end];
    let after = &after_char[digits_end..];
    let rest = after.trim_start();
    let spaced = rest.len() < after.len();
    (!digits.is_empty() && spaced && !rest.is_empty() && !rest.starts_with(syntax.separator)).then_some((digits, rest))
}

/// `/wait` (any case) and what follows it, when it stands alone as a word.
fn wait_prefix(piece: &str, syntax: Syntax) -> Option<&str> {
    let rest = piece.strip_prefix(syntax.command)?;
    let head = rest.get(..4)?;
    let after = &rest[4..];
    (head.eq_ignore_ascii_case("wait")
        && (after.is_empty() || after.starts_with(char::is_whitespace) || after.starts_with(syntax.separator)))
    .then(|| after.trim_start())
}

/// A `/wait`'s step from what follows the word, and the rest of the sequence.
fn wait_step(rest: &str, syntax: Syntax) -> Result<(Step, &str), ExpandError> {
    let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    let (timeout, rest) = if digits > 0 {
        let secs = rest[..digits]
            .parse::<u64>()
            .ok()
            .filter(|n| (1..=MAX_WAIT_SECS).contains(n))
            .ok_or(ExpandError::BadWait)?;
        let after = rest[digits..].trim_start();
        if after.is_empty() || after.starts_with(syntax.separator) {
            return Ok((Step::WaitTime(Duration::from_secs(secs)), after));
        }
        if !after.starts_with('{') {
            return Err(ExpandError::BadWait);
        }
        (Duration::from_secs(secs), after)
    } else {
        (DEFAULT_WAIT, rest)
    };
    let (text, next) = if let Some(group) = rest.strip_prefix('{') {
        let close = closing_brace(group).ok_or(ExpandError::Unclosed)?;
        (group[..close].trim().to_string(), &group[close + 1..])
    } else {
        piece(rest, syntax)
    };
    if text.is_empty() {
        return Err(ExpandError::BadWait);
    }
    Ok((Step::WaitText { text, timeout }, next))
}

/// Expand a separated sequence into `out`.
fn sequence(mut s: &str, syntax: Syntax, out: &mut Vec<Step>) -> Result<(), ExpandError> {
    loop {
        s = s.trim_start();
        if s.is_empty() {
            return Ok(());
        }
        if let Some(rest) = wait_prefix(s, syntax) {
            let (step, next) = wait_step(rest, syntax)?;
            if out.len() >= MAX_COMMANDS {
                return Err(ExpandError::TooMany);
            }
            out.push(step);
            s = next;
        } else if let Some((digits, rest)) = repeat_prefix(s, syntax) {
            let count = digits
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=MAX_REPEAT).contains(n))
                .ok_or_else(|| ExpandError::BadCount(format!("{}{digits}", syntax.command)))?;
            let mut once = Vec::new();
            if let Some(group) = rest.strip_prefix('{') {
                let close = closing_brace(group).ok_or(ExpandError::Unclosed)?;
                sequence(&group[..close], syntax, &mut once)?;
                s = &group[close + 1..];
                // Anything after the group up to the next separator is a command of its own.
            } else {
                let (command, next) = piece(rest, syntax);
                once.push(Step::Send(command));
                s = next;
            }
            for _ in 0..count {
                if out.len() + once.len() > MAX_COMMANDS {
                    return Err(ExpandError::TooMany);
                }
                out.extend(once.iter().cloned());
            }
        } else {
            let (command, next) = piece(s, syntax);
            if !command.is_empty() {
                if out.len() >= MAX_COMMANDS {
                    return Err(ExpandError::TooMany);
                }
                out.push(Step::Send(command));
            }
            s = next;
        }
    }
}

/// One command up to the next unescaped separator (trimmed, `\;` made a semicolon), and what
/// follows it.
fn piece(s: &str, syntax: Syntax) -> (String, &str) {
    let mut command = String::new();
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '\\' && chars.peek().is_some_and(|&(_, n)| n == ';') {
            command.push(';');
            chars.next();
        } else if s[i..].starts_with(syntax.separator) {
            return (command.trim().to_string(), &s[i + syntax.separator.len()..]);
        } else {
            command.push(c);
        }
    }
    (command.trim().to_string(), "")
}

/// Where the brace matching an opening one (already consumed) closes, counting nested pairs.
fn closing_brace(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(i),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The steps as text under `syntax`: commands as they are, waits as `<wait …>`.
    fn steps(line: &str, syntax: Syntax) -> Vec<String> {
        expand(line, syntax)
            .unwrap()
            .into_iter()
            .map(|step| match step {
                Step::Send(command) => command,
                Step::WaitText { text, timeout } => format!("<wait {}s {text}>", timeout.as_secs()),
                Step::WaitTime(time) => format!("<wait {}s>", time.as_secs()),
            })
            .collect()
    }

    /// Under the TinTin++ style.
    fn ok(line: &str) -> Vec<String> {
        steps(line, Syntax::TINTIN)
    }

    fn tintin(line: &str) -> Result<Vec<Step>, ExpandError> {
        expand(line, Syntax::TINTIN)
    }

    #[test]
    fn wait_pauses_for_text_or_seconds() {
        assert_eq!(ok("#wait {the droid is dead}"), ["<wait 60s the droid is dead>"]);
        assert_eq!(
            ok("#wait the droid is dead"),
            ["<wait 60s the droid is dead>"],
            "braces optional"
        );
        assert_eq!(ok("#wait 2"), ["<wait 2s>"]);
        assert_eq!(ok("#wait 120 {the droid is dead}"), ["<wait 120s the droid is dead>"]);
        assert_eq!(ok("#WAIT 2"), ["<wait 2s>"], "any case, as TinTin++'s commands");
        assert_eq!(
            ok("#wait {a;b}"),
            ["<wait 60s a;b>"],
            "a semicolon inside the braces is text"
        );
        assert_eq!(ok("say #wait 2"), ["say #wait 2"], "only at a command's start");
        assert_eq!(ok("#waitress"), ["#waitress"]);
    }

    #[test]
    fn a_repeat_may_wait_between_commands() {
        let steps = ok("#3 {say 1;kill droid;#wait {the droid is dead}};look");
        assert_eq!(steps.len(), 10);
        assert_eq!(steps[..3], ["say 1", "kill droid", "<wait 60s the droid is dead>"]);
        assert_eq!(steps[9], "look");
    }

    #[test]
    fn a_wrong_wait_is_refused() {
        for line in ["#wait", "#wait 0", "#wait 601", "#wait {}", "#wait 5 {}", "#wait 2 x"] {
            assert_eq!(tintin(line), Err(ExpandError::BadWait), "{line}");
        }
        assert_eq!(tintin("#wait {dead"), Err(ExpandError::Unclosed));
    }

    #[test]
    fn a_plain_line_is_itself() {
        for syntax in [Syntax::WANDUR, Syntax::TINTIN, Syntax::MUSH_SAFE] {
            assert_eq!(steps("say hello there", syntax), ["say hello there"]);
            assert_eq!(
                steps("  look  ", syntax),
                ["  look  "],
                "spacing kept when there is no shorthand"
            );
            assert_eq!(steps("", syntax), [""]);
        }
    }

    #[test]
    fn a_hash_without_a_count_and_space_is_text() {
        assert_eq!(ok("#help"), ["#help"]);
        assert_eq!(ok("say #1 fan"), ["say #1 fan"]);
        assert_eq!(ok("#3"), ["#3"], "no command after the count");
        assert_eq!(ok("#3x look"), ["#3x look"]);
    }

    #[test]
    fn a_count_repeats_the_command() {
        assert_eq!(ok("#3 say 1"), ["say 1", "say 1", "say 1"]);
        assert_eq!(ok("#1 look"), ["look"]);
        assert_eq!(ok("#100 n").len(), 100);
    }

    #[test]
    fn counts_out_of_range_are_refused() {
        assert_eq!(tintin("#0 look"), Err(ExpandError::BadCount("#0".into())));
        assert_eq!(tintin("#101 look"), Err(ExpandError::BadCount("#101".into())));
        assert_eq!(
            tintin("#99999999999999999999 look"),
            Err(ExpandError::BadCount("#99999999999999999999".into()))
        );
        assert_eq!(
            expand("/0 look", Syntax::WANDUR),
            Err(ExpandError::BadCount("/0".into())),
            "the count as typed, with the style's character"
        );
    }

    #[test]
    fn semicolons_separate_commands() {
        assert_eq!(ok("get all;wear all"), ["get all", "wear all"]);
        assert_eq!(
            ok("get all ; wear all ;"),
            ["get all", "wear all"],
            "trimmed, empty pieces dropped"
        );
        assert_eq!(ok(";;"), Vec::<String>::new());
    }

    #[test]
    fn an_escaped_semicolon_is_text() {
        assert_eq!(ok("say wait\\; listen"), ["say wait; listen"]);
        assert_eq!(ok("say a\\;b;look"), ["say a;b", "look"]);
        assert_eq!(ok("say a \\ b"), ["say a \\ b"], "other backslashes stay");
    }

    #[test]
    fn a_count_repeats_one_piece_of_a_chain() {
        assert_eq!(ok("#2 get coin;look"), ["get coin", "get coin", "look"]);
        assert_eq!(ok("look;#2 n"), ["look", "n", "n"]);
    }

    #[test]
    fn braces_group_a_repeat() {
        assert_eq!(
            ok("#2 {get coin;put coin bag};look"),
            ["get coin", "put coin bag", "get coin", "put coin bag", "look"]
        );
        assert_eq!(ok("#2 {#2 n}"), ["n", "n", "n", "n"], "nested");
        assert_eq!(ok("say {hi};look"), ["say {hi}", "look"], "braces elsewhere are text");
        assert_eq!(tintin("#2 {get coin;look"), Err(ExpandError::Unclosed));
    }

    #[test]
    fn a_line_may_not_make_too_many_commands() {
        assert_eq!(ok("#100 n;#100 s").len(), 200);
        assert_eq!(tintin("#100 n;#100 s;look"), Err(ExpandError::TooMany));
        assert_eq!(tintin("#100 {#100 n}"), Err(ExpandError::TooMany));
    }

    /// Wandur's own style: the same commands spelled with `/`; `#` is text.
    #[test]
    fn the_wandur_style_spells_commands_with_a_slash() {
        let w = |line| steps(line, Syntax::WANDUR);
        assert_eq!(w("/10 say 1").len(), 10);
        assert_eq!(w("/3 {a;b}"), ["a", "b", "a", "b", "a", "b"]);
        assert_eq!(w("/wait {the droid is dead}"), ["<wait 60s the droid is dead>"]);
        assert_eq!(w("/wait 120 {dead}"), ["<wait 120s dead>"]);
        assert_eq!(w("/wait 2;look"), ["<wait 2s>", "look"]);
        assert_eq!(w("get all;wear all"), ["get all", "wear all"]);
        assert_eq!(w("say a\\;b;look"), ["say a;b", "look"]);
        assert_eq!(
            w("/2 {say 1;/wait {done}};look"),
            ["say 1", "<wait 60s done>", "say 1", "<wait 60s done>", "look"]
        );
        assert_eq!(w("#3 look"), ["#3 look"], "the other style's commands are text");
        assert_eq!(w("#wait 2"), ["#wait 2"]);
        assert_eq!(w("/me waves"), ["/me waves"], "a slash not starting a command is text");
        assert_eq!(expand("/wait", Syntax::WANDUR), Err(ExpandError::BadWait));
        assert_eq!(expand("/3 {a", Syntax::WANDUR), Err(ExpandError::Unclosed));
    }

    /// The command character twice sends the line once with one removed, nothing else applied.
    #[test]
    fn a_doubled_command_character_is_literal() {
        assert_eq!(steps("//me waves", Syntax::WANDUR), ["/me waves"]);
        assert_eq!(steps("//3 look;n", Syntax::WANDUR), ["/3 look;n"], "no chain either");
        assert_eq!(steps("  //wait", Syntax::WANDUR), ["  /wait"]);
        assert_eq!(steps("##12 look", Syntax::TINTIN), ["#12 look"]);
        assert_eq!(steps("//me waves", Syntax::MUSH_SAFE), ["/me waves"]);
        assert_eq!(
            steps("##12 look", Syntax::WANDUR),
            ["##12 look"],
            "only its own character"
        );
        assert_eq!(steps("//", Syntax::WANDUR), ["/"]);
    }

    /// MUSH-safe: `;;` separates, a lone `;` (a pose) is text, `\;` is still a literal `;`.
    #[test]
    fn the_mush_safe_style_chains_with_a_double_semicolon() {
        let m = |line| steps(line, Syntax::MUSH_SAFE);
        assert_eq!(m("get all;;wear all"), ["get all", "wear all"]);
        assert_eq!(m(";waves happily"), [";waves happily"], "a pose goes as typed");
        assert_eq!(m("say a; b  "), ["say a; b  "], "spacing kept without a separator");
        assert_eq!(m(";waves;;look"), [";waves", "look"]);
        assert_eq!(m("say a\\;;;look"), ["say a;", "look"], "\\; is a literal semicolon");
        assert_eq!(m("say a\\;;b"), ["say a;;b"], "and \\;; sends two");
        assert_eq!(
            m("/2 {say 1;;/wait 2};;look"),
            ["say 1", "<wait 2s>", "say 1", "<wait 2s>", "look"]
        );
        assert_eq!(m("/2 :waves; grins"), [":waves; grins", ":waves; grins"]);
        assert_eq!(m("/wait {a;;b}"), ["<wait 60s a;;b>"]);
        assert_eq!(m("/wait 2;;n"), ["<wait 2s>", "n"]);
    }

    #[test]
    fn styles_are_kept_by_name_and_unknown_names_fall_back() {
        for style in CommandStyle::ALL {
            let json = serde_json::to_string(&style).unwrap();
            assert_eq!(json, format!("\"{}\"", style.name()));
            assert_eq!(serde_json::from_str::<CommandStyle>(&json).unwrap(), style);
        }
        assert_eq!(CommandStyle::default(), CommandStyle::Wandur);
        assert_eq!(CommandStyle::TinTin.syntax(), Syntax::TINTIN);
        assert_eq!(CommandStyle::MushSafe.syntax(), Syntax::MUSH_SAFE);
        #[derive(serde::Deserialize)]
        struct Holder {
            #[serde(deserialize_with = "lenient_style", default)]
            global: CommandStyle,
            #[serde(deserialize_with = "lenient_override", default)]
            world: Option<CommandStyle>,
        }
        let h: Holder = serde_json::from_str(r#"{"global":"zmud","world":"later"}"#).unwrap();
        assert_eq!((h.global, h.world), (CommandStyle::Wandur, None));
        let h: Holder = serde_json::from_str(r#"{"global":"tintin","world":"mush-safe"}"#).unwrap();
        assert_eq!(
            (h.global, h.world),
            (CommandStyle::TinTin, Some(CommandStyle::MushSafe))
        );
        let h: Holder = serde_json::from_str("{}").unwrap();
        assert_eq!((h.global, h.world), (CommandStyle::Wandur, None));
    }

    #[test]
    fn a_session_takes_the_world_style_then_mush_then_the_global_one() {
        use CommandStyle::*;
        assert_eq!(
            resolve(Some(TinTin), Some("PennMUSH 1.8"), Wandur),
            TinTin,
            "the world's own wins"
        );
        assert_eq!(resolve(Some(Wandur), Some("TinyMUX 2.12"), TinTin), Wandur);
        for codebase in [
            "PennMUSH 1.8.8",
            "TinyMUSH 3.3",
            "TinyMUX 2.12",
            "RhostMUSH",
            "mux",
            "Custom MUSH",
        ] {
            assert_eq!(resolve(None, Some(codebase), TinTin), MushSafe, "{codebase}");
        }
        assert_eq!(resolve(None, Some("SMAUG 1.4a"), TinTin), TinTin);
        assert_eq!(resolve(None, Some(""), Wandur), Wandur);
        assert_eq!(resolve(None, None, MushSafe), MushSafe, "no codebase: the global one");
    }

    #[test]
    fn a_line_in_the_other_style_is_recognized() {
        let tt = |line| uses_commands(line, Syntax::TINTIN);
        assert!(tt("#3 look"));
        assert!(tt("#wait 2"));
        assert!(tt("  #10 {say 1;#wait {dead}}"));
        assert!(tt("look;#2 n"), "anywhere in the chain");
        assert!(!tt("#help"));
        assert!(!tt("say #3 look"));
        assert!(!tt("##3 look"), "the literal escape");
        assert!(!tt("look;n"));
        assert!(!tt(""));
        assert!(uses_commands("/3 look", Syntax::WANDUR));
        assert!(!uses_commands("#3 look", Syntax::WANDUR));
    }
}
