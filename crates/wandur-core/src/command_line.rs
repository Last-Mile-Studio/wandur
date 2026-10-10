//! The command line's shorthand, as TinTin++ (and zMUD and CMUD before it) spell it: `;` separates
//! commands (`get all;wear all`), `\;` is a literal semicolon, and `#N command` sends a command N
//! times (`#10 say 1`). Braces group a repeat: `#3 {get coin;put coin bag}` repeats both, while
//! `#3 get coin;look` repeats `get coin` and sends `look` once. `#wait {text}` (Wandur's own, in the
//! same spelling) holds the rest until a line from the world contains the text, at most 60 seconds
//! (`#wait 120 {text}` for longer), and `#wait 2` pauses two seconds, so a repeat can wait for each
//! round to finish: `#10 {say 1;kill droid;#wait {the droid is dead}}`. A `#` not followed by a
//! count or `wait` and a space is ordinary text and goes to the world unchanged.

use std::fmt;
use std::time::Duration;

/// The most times one `#N` may repeat.
pub const MAX_REPEAT: u32 = 100;
/// The most commands (and waits) one typed line may expand to.
pub const MAX_COMMANDS: usize = 200;
/// The longest `#wait`, in seconds.
pub const MAX_WAIT_SECS: u64 = 600;
/// How long `#wait {text}` waits when no time is given.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(60);

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

/// Why a line could not be expanded; nothing is sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExpandError {
    /// `#0 ...` or a count over [`MAX_REPEAT`].
    BadCount(String),
    /// `#3 {...` with no closing brace.
    Unclosed,
    /// More than [`MAX_COMMANDS`] in all.
    TooMany,
    /// `#wait` without text or with seconds outside 1 to [`MAX_WAIT_SECS`].
    BadWait,
}

impl fmt::Display for ExpandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadCount(n) => write!(f, "#{n}: a repeat count runs from 1 to {MAX_REPEAT}"),
            Self::Unclosed => write!(f, "a {{ after a repeat count needs its closing }}"),
            Self::TooMany => write!(f, "that line makes more than {MAX_COMMANDS} commands"),
            Self::BadWait => write!(
                f,
                "#wait takes seconds from 1 to {MAX_WAIT_SECS}, or text: #wait {{the droid is dead}}"
            ),
        }
    }
}

/// The steps a typed line stands for, in order. A line without shorthand comes back as itself,
/// exactly (spacing kept); pieces of a chain are trimmed and empty ones dropped.
pub fn expand(line: &str) -> Result<Vec<Step>, ExpandError> {
    let start = line.trim_start();
    if !line.contains(';') && repeat_prefix(start).is_none() && wait_prefix(start).is_none() {
        return Ok(vec![Step::Send(line.to_string())]);
    }
    let mut out = Vec::new();
    sequence(line, &mut out)?;
    Ok(out)
}

/// `#N` and the whitespace after it, when a command follows: the count's digits and the rest.
fn repeat_prefix(piece: &str) -> Option<(&str, &str)> {
    let digits_end = piece.strip_prefix('#')?.find(|c: char| !c.is_ascii_digit())? + 1;
    let digits = &piece[1..digits_end];
    let after = &piece[digits_end..];
    let rest = after.trim_start();
    let spaced = rest.len() < after.len();
    (!digits.is_empty() && spaced && !rest.is_empty() && !rest.starts_with(';')).then_some((digits, rest))
}

/// `#wait` (any case) and what follows it, when it stands alone as a word.
fn wait_prefix(piece: &str) -> Option<&str> {
    let head = piece.get(..5)?;
    let after = &piece[5..];
    (head.eq_ignore_ascii_case("#wait") && after.chars().next().is_none_or(|c| c.is_whitespace() || c == ';'))
        .then(|| after.trim_start())
}

/// A `#wait`'s step from what follows the word, and the rest of the sequence.
fn wait_step(rest: &str) -> Result<(Step, &str), ExpandError> {
    let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    let (timeout, rest) = if digits > 0 {
        let secs = rest[..digits]
            .parse::<u64>()
            .ok()
            .filter(|n| (1..=MAX_WAIT_SECS).contains(n))
            .ok_or(ExpandError::BadWait)?;
        let after = rest[digits..].trim_start();
        if after.is_empty() || after.starts_with(';') {
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
        piece(rest)
    };
    if text.is_empty() {
        return Err(ExpandError::BadWait);
    }
    Ok((Step::WaitText { text, timeout }, next))
}

/// Expand a `;`-separated sequence into `out`.
fn sequence(mut s: &str, out: &mut Vec<Step>) -> Result<(), ExpandError> {
    loop {
        s = s.trim_start();
        if s.is_empty() {
            return Ok(());
        }
        if let Some(rest) = wait_prefix(s) {
            let (step, next) = wait_step(rest)?;
            if out.len() >= MAX_COMMANDS {
                return Err(ExpandError::TooMany);
            }
            out.push(step);
            s = next;
        } else if let Some((digits, rest)) = repeat_prefix(s) {
            let count = digits
                .parse::<u32>()
                .ok()
                .filter(|n| (1..=MAX_REPEAT).contains(n))
                .ok_or_else(|| ExpandError::BadCount(digits.to_string()))?;
            let mut once = Vec::new();
            if let Some(group) = rest.strip_prefix('{') {
                let close = closing_brace(group).ok_or(ExpandError::Unclosed)?;
                sequence(&group[..close], &mut once)?;
                s = &group[close + 1..];
                // Anything after the group up to the next `;` is a command of its own.
            } else {
                let (command, next) = piece(rest);
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
            let (command, next) = piece(s);
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

/// One command up to the next unescaped `;` (trimmed, `\;` made a semicolon), and what follows it.
fn piece(s: &str) -> (String, &str) {
    let mut command = String::new();
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' if chars.peek().is_some_and(|&(_, n)| n == ';') => {
                command.push(';');
                chars.next();
            }
            ';' => return (command.trim().to_string(), &s[i + 1..]),
            _ => command.push(c),
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

    /// The steps as text: commands as they are, waits as `<wait …>`.
    fn ok(line: &str) -> Vec<String> {
        expand(line)
            .unwrap()
            .into_iter()
            .map(|step| match step {
                Step::Send(command) => command,
                Step::WaitText { text, timeout } => format!("<wait {}s {text}>", timeout.as_secs()),
                Step::WaitTime(time) => format!("<wait {}s>", time.as_secs()),
            })
            .collect()
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
            assert_eq!(expand(line), Err(ExpandError::BadWait), "{line}");
        }
        assert_eq!(expand("#wait {dead"), Err(ExpandError::Unclosed));
    }

    #[test]
    fn a_plain_line_is_itself() {
        assert_eq!(ok("say hello there"), ["say hello there"]);
        assert_eq!(ok("  look  "), ["  look  "], "spacing kept when there is no shorthand");
        assert_eq!(ok(""), [""]);
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
        assert_eq!(expand("#0 look"), Err(ExpandError::BadCount("0".into())));
        assert_eq!(expand("#101 look"), Err(ExpandError::BadCount("101".into())));
        assert_eq!(
            expand("#99999999999999999999 look"),
            Err(ExpandError::BadCount("99999999999999999999".into()))
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
        assert_eq!(expand("#2 {get coin;look"), Err(ExpandError::Unclosed));
    }

    #[test]
    fn a_line_may_not_make_too_many_commands() {
        assert_eq!(ok("#100 n;#100 s").len(), 200);
        assert_eq!(expand("#100 n;#100 s;look"), Err(ExpandError::TooMany));
        assert_eq!(expand("#100 {#100 n}"), Err(ExpandError::TooMany));
    }
}
