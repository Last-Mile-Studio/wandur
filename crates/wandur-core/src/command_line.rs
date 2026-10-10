//! The command line's shorthand, in the person's chosen [`CommandStyle`]. In Wandur's own style
//! `;` separates commands (`get all;wear all`), `\;` is a literal semicolon, and `/N command` sends
//! a command N times (`/10 say 1`). Braces group a repeat: `/3 {get coin;put coin bag}` repeats
//! both, while `/3 get coin;look` repeats `get coin` and sends `look` once. `/wait {text}` holds
//! the rest until a line from the world contains the text, at most 60 seconds (`/wait 120 {text}`
//! for longer), and `/wait 2` pauses two seconds, so a repeat can wait for each round to finish:
//! `/10 {say 1;kill droid;/wait {the droid is dead}}`. A `/` not followed by a count or `wait` and
//! a space is ordinary text and goes to the world unchanged; a line that starts `//` goes with one
//! `/` removed and nothing else applied (`//me waves` sends `/me waves`). `/help` lists the
//! client's commands in the transcript, and `/help wait` explains one.
//!
//! The commands themselves are listed in [`crate::client_commands`]; one scanner here reads a
//! line for [`expand`] (what is sent), [`check`] (problems, with where they are) and
//! [`context_at`] (what the caret is in, for the command box's list and hints), so the three
//! cannot disagree.
//!
//! The TinTin++ style (as zMUD and CMUD before it) spells the same with `#` (`#10 say 1`,
//! `#wait 2`, `##` to send a `#`). The MUSH-safe style keeps `/` but separates commands with `;;`,
//! because on a MUSH a `;` starts a pose: there a single `;` is ordinary text, and `\;` is still a
//! literal `;` (so `\;;` sends `;;`).

use std::ops::Range;
use std::time::Duration;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::client_commands::{CommandId, CommandSpec, Head, ParamKind};
use crate::l10n::{S, tf};

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
    /// `/help`: list the client's commands in the transcript, or explain one.
    Help(Option<CommandId>),
}

/// Why a line could not be expanded; nothing is sent. The person's text for it is
/// [`Diagnostic::message`] (localized, naming the active command character).
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
    /// `/help` with a name that is not one of the client's commands.
    UnknownCommand(String),
}

/// What [`check`] finds in a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The line is refused as it stands.
    Refused(ExpandError),
    /// `/3` with no command after it: it goes to the world as text, which may not be meant.
    LoneCount(String),
}

/// A problem and the bytes of the line it is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub problem: Problem,
    pub span: Range<usize>,
    /// More typing at the end can fix it (an open brace, `/wait` with nothing yet): shown muted,
    /// never as an error while typing, though Enter still refuses it.
    pub incomplete: bool,
}

impl Diagnostic {
    fn refused(error: ExpandError, span: Range<usize>, incomplete: bool) -> Self {
        Self {
            problem: Problem::Refused(error),
            span,
            incomplete,
        }
    }

    /// Enter would refuse the line.
    pub fn refuses(&self) -> bool {
        matches!(self.problem, Problem::Refused(_))
    }

    /// Certainly wrong: more typing at the end cannot fix it.
    pub fn is_wrong(&self) -> bool {
        self.refuses() && !self.incomplete
    }

    /// The person's text for it, in the current language, with the style's characters.
    pub fn message(&self, syntax: Syntax) -> String {
        match &self.problem {
            Problem::Refused(error) => error_message(error, syntax),
            Problem::LoneCount(head) => tf(S::CmdLoneCount, &[head]),
        }
    }
}

/// The person's text for an error, in the current language, with the style's characters.
pub fn error_message(error: &ExpandError, syntax: Syntax) -> String {
    match error {
        ExpandError::BadCount(head) => tf(S::CommandRepeatCount, &[head, &MAX_REPEAT]),
        ExpandError::Unclosed => tf(S::CommandRepeatUnclosed, &[]),
        ExpandError::TooMany => tf(S::CommandTooMany, &[&MAX_COMMANDS]),
        ExpandError::BadWait => tf(S::CommandWaitUsage, &[&syntax.command, &MAX_WAIT_SECS]),
        ExpandError::UnknownCommand(name) => tf(S::CmdHelpUnknown, &[name, &syntax.command]),
    }
}

/// The steps a typed line stands for, in order. A line without shorthand comes back as itself,
/// exactly (spacing kept); pieces of a chain are trimmed and empty ones dropped. A line that
/// starts with the command character twice comes back once, with one of them removed.
pub fn expand(line: &str, syntax: Syntax) -> Result<Vec<Step>, ExpandError> {
    let parse = Parser::parse(line, syntax);
    if let Some(problem) = parse.diagnostics.into_iter().find_map(|d| match d.problem {
        Problem::Refused(error) => Some(error),
        Problem::LoneCount(_) => None,
    }) {
        return Err(problem);
    }
    let plain = !line.contains(syntax.separator)
        && !line.contains("\\;")
        && matches!(parse.items.as_slice(), [] | [Item::Send(_)]);
    if plain && !parse.literal {
        return Ok(vec![Step::Send(line.to_string())]);
    }
    let mut out = Vec::new();
    flatten(&parse.items, &mut out)?;
    Ok(out)
}

/// Whether the line would use one of the client's commands that send or wait (a repeat or a
/// wait) in this style, anywhere in its chain. A line that starts with the command character
/// twice uses none, and `/help` does not count: the word is too common on a MUD to mean a style.
pub fn uses_commands(line: &str, syntax: Syntax) -> bool {
    fn any(items: &[Item]) -> bool {
        items.iter().any(|item| match item {
            Item::Send(_) | Item::Step(Step::Help(_) | Step::Send(_)) => false,
            Item::Repeat(..) | Item::Step(_) => true,
        })
    }
    any(&Parser::parse(line, syntax).items)
}

/// Everything wrong or worth a word in a line, in the order it appears: what [`expand`] would
/// refuse (certainly wrong, or incomplete), and a lone `/3`.
pub fn check(line: &str, syntax: Syntax) -> Vec<Diagnostic> {
    let parse = Parser::parse(line, syntax);
    let mut diagnostics = parse.diagnostics;
    if !diagnostics.iter().any(Diagnostic::refuses) && flatten(&parse.items, &mut Vec::new()).is_err() {
        diagnostics.push(Diagnostic::refused(ExpandError::TooMany, 0..line.len(), false));
    }
    diagnostics
}

/// Where a caret is, in client-command terms, for the command box's list and hints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Context {
    /// Not in a client command: plain text, a world command.
    None,
    /// Typing a head at a command start: `range` is the whole head (`/`, `/w`, `/10`), the caret
    /// inside it or at its end.
    Head { range: Range<usize> },
    /// After a client command's head: which command, the forms (indices into its `forms`) that
    /// still fit what is typed, the parameter the caret is in or comes to next (an index into
    /// each fitting form), and whether that parameter has text yet.
    Args {
        id: CommandId,
        head: Range<usize>,
        fitting: Vec<usize>,
        active: usize,
        typed: bool,
    },
}

impl Context {
    /// The parameters still to type, as placeholders after the caret (`<seconds> {text}`): the
    /// first fitting form's from the one the caret is in (when it is still empty) or the next.
    pub fn remaining(&self, syntax: Syntax) -> Option<String> {
        let Context::Args {
            id,
            fitting,
            active,
            typed,
            ..
        } = self
        else {
            return None;
        };
        let form = CommandSpec::get(*id).forms.get(*fitting.first()?)?;
        let from = active + usize::from(*typed);
        let rest: Vec<String> = form.params.get(from..)?.iter().map(|p| p.placeholder(syntax)).collect();
        (!rest.is_empty()).then(|| rest.join(" "))
    }
}

/// What the caret at byte `caret` of `line` is in.
pub fn context_at(line: &str, caret: usize, syntax: Syntax) -> Context {
    let parse = Parser::parse(line, syntax);
    for &start in &parse.starts {
        if !line[start..].starts_with(syntax.command) {
            continue;
        }
        let after = start + syntax.command.len_utf8();
        let end = line[after..]
            .find(|c: char| !c.is_ascii_alphanumeric())
            .map_or(line.len(), |i| after + i);
        if start < caret && caret <= end {
            return Context::Head { range: start..end };
        }
    }
    let Some(found) = parse
        .uses
        .iter()
        .filter(|u| u.head.end < caret && caret <= u.end)
        .min_by_key(|u| u.end - u.head.start)
    else {
        return Context::None;
    };
    let (active, typed) = match found.args.iter().position(|(_, r)| r.start <= caret && caret <= r.end) {
        Some(i) => (i, true),
        None => (found.args.iter().filter(|(_, r)| r.end < caret).count(), false),
    };
    let spec = CommandSpec::get(found.id);
    let fitting = (0..spec.forms.len())
        .filter(|&i| {
            let params = spec.forms[i].params;
            params.len() >= found.args.len() && found.args.iter().zip(params).all(|((kind, _), p)| *kind == p.kind)
        })
        .collect();
    Context::Args {
        id: found.id,
        head: found.head.clone(),
        fitting,
        active,
        typed,
    }
}

/// A line read into commands, with what the command box needs to know about it.
struct Parse {
    items: Vec<Item>,
    diagnostics: Vec<Diagnostic>,
    /// Byte offsets where a command starts: the line's first non-blank character, the first
    /// after a separator, the first inside a repeat's braces.
    starts: Vec<usize>,
    /// Every client command found, with where its parts are.
    uses: Vec<Use>,
    /// The line starts with the command character twice.
    literal: bool,
}

/// One client command in a line (a lone `/3` too, so its hint can show).
struct Use {
    id: CommandId,
    /// The command character and the head (`/wait`, `/10`).
    head: Range<usize>,
    /// The parameters typed, in order, with their bytes (a repeat's count first).
    args: Vec<(ParamKind, Range<usize>)>,
    /// Where the command's own text ends (before a separator).
    end: usize,
}

/// A piece of a line before repeats are multiplied out.
enum Item {
    Send(String),
    Repeat(u32, Vec<Item>),
    Step(Step),
}

/// The steps `items` stand for, into `out`, refused past [`MAX_COMMANDS`].
fn flatten(items: &[Item], out: &mut Vec<Step>) -> Result<(), ExpandError> {
    for item in items {
        match item {
            Item::Send(command) => push(out, Step::Send(command.clone()))?,
            Item::Step(step) => push(out, step.clone())?,
            Item::Repeat(count, body) => {
                let mut once = Vec::new();
                flatten(body, &mut once)?;
                for _ in 0..*count {
                    if out.len() + once.len() > MAX_COMMANDS {
                        return Err(ExpandError::TooMany);
                    }
                    out.extend(once.iter().cloned());
                }
            }
        }
    }
    Ok(())
}

fn push(out: &mut Vec<Step>, step: Step) -> Result<(), ExpandError> {
    if out.len() >= MAX_COMMANDS {
        return Err(ExpandError::TooMany);
    }
    out.push(step);
    Ok(())
}

/// The one scanner behind [`expand`], [`check`] and [`context_at`], so the command box cannot
/// disagree with what is sent. It reads the whole line even past a problem, so a caret after an
/// open brace still finds its command starts.
struct Parser<'a> {
    line: &'a str,
    syntax: Syntax,
    diagnostics: Vec<Diagnostic>,
    starts: Vec<usize>,
    uses: Vec<Use>,
}

impl<'a> Parser<'a> {
    fn parse(line: &'a str, syntax: Syntax) -> Parse {
        let lead = line.len() - line.trim_start().len();
        if let Some(rest) = literal(&line[lead..], syntax) {
            return Parse {
                items: vec![Item::Send(format!("{}{rest}", &line[..lead]))],
                diagnostics: Vec::new(),
                starts: Vec::new(),
                uses: Vec::new(),
                literal: true,
            };
        }
        let mut parser = Parser {
            line,
            syntax,
            diagnostics: Vec::new(),
            starts: Vec::new(),
            uses: Vec::new(),
        };
        let items = parser.sequence(0, line.len());
        Parse {
            items,
            diagnostics: parser.diagnostics,
            starts: parser.starts,
            uses: parser.uses,
            literal: false,
        }
    }

    /// A separated sequence of commands between `at` and `end`.
    fn sequence(&mut self, mut at: usize, end: usize) -> Vec<Item> {
        let mut items = Vec::new();
        loop {
            at = self.skip_space(at, end);
            if at >= end {
                return items;
            }
            self.starts.push(at);
            let (item, next) = match self.word_head(at, end) {
                Some(CommandId::Wait) => self.wait(at, end),
                Some(_) => self.help(at, end),
                None => match self.count_head(at, end) {
                    Some(digits) => self.repeat(at, digits, end),
                    None => {
                        let (command, _, next) = self.piece(at, end);
                        ((!command.is_empty()).then_some(Item::Send(command)), next)
                    }
                },
            };
            items.extend(item);
            at = next;
        }
    }

    /// A word head (`/wait`, `/help`, any case) standing alone at `at`.
    fn word_head(&self, at: usize, end: usize) -> Option<CommandId> {
        let rest = self.line[at..end].strip_prefix(self.syntax.command)?;
        crate::client_commands::COMMANDS.into_iter().find_map(|spec| {
            let Head::Word(word) = spec.head else {
                return None;
            };
            let after = &rest[rest.get(..word.len()).filter(|w| w.eq_ignore_ascii_case(word))?.len()..];
            (after.is_empty() || after.starts_with(char::is_whitespace) || after.starts_with(self.syntax.separator))
                .then_some(spec.id)
        })
    }

    /// A repeat's count at `at` (the digits' bytes) when a space and a command follow it. A count
    /// with nothing after it (`/3`, `/3 `) is text, but it is noted ([`Problem::LoneCount`] and a
    /// use, for the repeat's hint) so the box can say so.
    fn count_head(&mut self, at: usize, end: usize) -> Option<Range<usize>> {
        let first = at + self.syntax.command.len_utf8();
        if !self.line[at..end].starts_with(self.syntax.command) {
            return None;
        }
        let digits_end = self.line[first..end]
            .find(|c: char| !c.is_ascii_digit())
            .map_or(end, |i| first + i);
        if digits_end == first {
            return None;
        }
        let after = &self.line[digits_end..end];
        let rest = after.trim_start();
        let spaced = rest.len() < after.len();
        if spaced && !rest.is_empty() && !rest.starts_with(self.syntax.separator) {
            return Some(first..digits_end);
        }
        if after.is_empty() || (spaced && (rest.is_empty() || rest.starts_with(self.syntax.separator))) {
            let head = format!("{}{}", self.syntax.command, &self.line[first..digits_end]);
            self.diagnostics.push(Diagnostic {
                problem: Problem::LoneCount(head),
                span: at..digits_end,
                incomplete: true,
            });
            self.uses.push(Use {
                id: CommandId::Repeat,
                head: at..digits_end,
                args: vec![(ParamKind::Count, first..digits_end)],
                end: end - rest.len(),
            });
        }
        None
    }

    /// `/N command` or `/N {commands}`; `digits` are the count's bytes.
    fn repeat(&mut self, at: usize, digits: Range<usize>, end: usize) -> (Option<Item>, usize) {
        let text = &self.line[digits.clone()];
        let count = match text.parse::<u32>().ok().filter(|n| (1..=MAX_REPEAT).contains(n)) {
            Some(n) => n,
            None => {
                let head = format!("{}{text}", self.syntax.command);
                self.diagnostics
                    .push(Diagnostic::refused(ExpandError::BadCount(head), digits.clone(), false));
                0
            }
        };
        let mut args = vec![(ParamKind::Count, digits.clone())];
        let r = self.skip_space(digits.end, end);
        let use_at = self.uses.len();
        self.uses.push(Use {
            id: CommandId::Repeat,
            head: at..digits.end,
            args: Vec::new(),
            end,
        });
        let (body, next, own_end) = if self.line[r..end].starts_with('{') {
            match closing_brace(&self.line[r + 1..end]) {
                Some(close) => {
                    let close = r + 1 + close;
                    args.push((ParamKind::Group, r..close + 1));
                    (self.sequence(r + 1, close), close + 1, close + 1)
                }
                None => {
                    self.diagnostics
                        .push(Diagnostic::refused(ExpandError::Unclosed, r..end, true));
                    args.push((ParamKind::Group, r..end));
                    (self.sequence(r + 1, end), end, end)
                }
            }
        } else {
            let (command, separator, next) = self.piece(r, end);
            args.push((ParamKind::Command, r..self.trim_end(r, separator)));
            (vec![Item::Send(command)], next, separator)
        };
        let own = &mut self.uses[use_at];
        own.args = args;
        own.end = own_end;
        (Some(Item::Repeat(count, body)), next)
    }

    /// `/wait {text}`, `/wait text`, `/wait 120 {text}` or `/wait 2`.
    fn wait(&mut self, at: usize, end: usize) -> (Option<Item>, usize) {
        let head = at..at + self.syntax.command.len_utf8() + "wait".len();
        let r = self.skip_space(head.end, end);
        let mut args = Vec::new();
        let mut wrong = false;
        let digits_end = self.line[r..end]
            .find(|c: char| !c.is_ascii_digit())
            .map_or(end, |i| r + i);
        let finish = |parser: &mut Self, args, own_end, item: Option<Item>, next| {
            parser.uses.push(Use {
                id: CommandId::Wait,
                head: head.clone(),
                args,
                end: own_end,
            });
            (item, next)
        };
        let (timeout, pos) = if digits_end > r {
            let secs = self.line[r..digits_end]
                .parse::<u64>()
                .ok()
                .filter(|n| (1..=MAX_WAIT_SECS).contains(n));
            if secs.is_none() {
                self.diagnostics
                    .push(Diagnostic::refused(ExpandError::BadWait, r..digits_end, false));
                wrong = true;
            }
            args.push((ParamKind::Seconds, r..digits_end));
            let after = self.skip_space(digits_end, end);
            if after == end || self.line[after..end].starts_with(self.syntax.separator) {
                let item = secs.map(|s| Item::Step(Step::WaitTime(Duration::from_secs(s))));
                return finish(self, args, after, item, after);
            }
            if !self.line[after..end].starts_with('{') {
                let (_, separator, next) = self.piece(after, end);
                let text_end = self.trim_end(after, separator);
                args.push((ParamKind::Text, after..text_end));
                if !wrong {
                    self.diagnostics
                        .push(Diagnostic::refused(ExpandError::BadWait, after..text_end, false));
                }
                return finish(self, args, separator, None, next);
            }
            (Duration::from_secs(secs.unwrap_or(1)), after)
        } else {
            (DEFAULT_WAIT, r)
        };
        let (text, own_end, next) = if self.line[pos..end].starts_with('{') {
            match closing_brace(&self.line[pos + 1..end]) {
                Some(close) => {
                    let close = pos + 1 + close;
                    args.push((ParamKind::Text, pos..close + 1));
                    (self.line[pos + 1..close].trim().to_string(), close + 1, close + 1)
                }
                None => {
                    self.diagnostics
                        .push(Diagnostic::refused(ExpandError::Unclosed, pos..end, true));
                    args.push((ParamKind::Text, pos..end));
                    wrong = true;
                    (self.line[pos + 1..end].trim().to_string(), end, end)
                }
            }
        } else {
            let (text, separator, next) = self.piece(pos, end);
            if !text.is_empty() {
                args.push((ParamKind::Text, pos..self.trim_end(pos, separator)));
            }
            (text, separator, next)
        };
        if text.is_empty() && !wrong {
            // `/wait` with nothing yet can still be finished; `/wait {}` cannot.
            let incomplete = pos == end;
            let span = if incomplete { head.clone() } else { pos..own_end };
            self.diagnostics
                .push(Diagnostic::refused(ExpandError::BadWait, span, incomplete));
            wrong = true;
        }
        let item = (!wrong).then_some(Item::Step(Step::WaitText { text, timeout }));
        finish(self, args, own_end, item, next)
    }

    /// `/help` or `/help wait` (the name with or without the command character).
    fn help(&mut self, at: usize, end: usize) -> (Option<Item>, usize) {
        let head = at..at + self.syntax.command.len_utf8() + "help".len();
        let r = self.skip_space(head.end, end);
        let (name, separator, next) = self.piece(r, end);
        let mut args = Vec::new();
        let item = if name.is_empty() {
            Some(Item::Step(Step::Help(None)))
        } else {
            let span = r..self.trim_end(r, separator);
            args.push((ParamKind::Name, span.clone()));
            let bare = name.strip_prefix(self.syntax.command).unwrap_or(&name);
            match CommandSpec::named(bare) {
                Some(spec) => Some(Item::Step(Step::Help(Some(spec.id)))),
                None => {
                    let lower = bare.to_ascii_lowercase();
                    let incomplete = span.end == end
                        && crate::client_commands::COMMANDS
                            .iter()
                            .any(|c| c.name().starts_with(&lower));
                    self.diagnostics.push(Diagnostic::refused(
                        ExpandError::UnknownCommand(name.clone()),
                        span,
                        incomplete,
                    ));
                    None
                }
            }
        };
        self.uses.push(Use {
            id: CommandId::Help,
            head,
            args,
            end: separator,
        });
        (item, next)
    }

    fn skip_space(&self, at: usize, end: usize) -> usize {
        end - self.line[at..end].trim_start().len()
    }

    /// `end` moved back over trailing whitespace, not before `at`.
    fn trim_end(&self, at: usize, end: usize) -> usize {
        at + self.line[at..end].trim_end().len()
    }

    /// One command from `at` up to the next unescaped separator (trimmed, `\;` made a
    /// semicolon), where the separator is (or `end`), and where the next command begins.
    fn piece(&self, at: usize, end: usize) -> (String, usize, usize) {
        let s = &self.line[at..end];
        let separator = self.syntax.separator;
        let mut command = String::new();
        let mut chars = s.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if c == '\\' && chars.peek().is_some_and(|&(_, n)| n == ';') {
                command.push(';');
                chars.next();
            } else if s[i..].starts_with(separator) {
                return (command.trim().to_string(), at + i, at + i + separator.len());
            } else {
                command.push(c);
            }
        }
        (command.trim().to_string(), end, end)
    }
}

/// The rest of a line that starts with the command character twice, after the first.
fn literal(start: &str, syntax: Syntax) -> Option<&str> {
    let rest = start.strip_prefix(syntax.command)?;
    rest.starts_with(syntax.command).then_some(rest)
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
                Step::Help(topic) => format!("<help {topic:?}>"),
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
        assert_eq!(ok("#helpful"), ["#helpful"]);
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

    /// `/help` is a client command: alone, or with a command's name.
    #[test]
    fn help_lists_or_explains_the_commands() {
        let w = |line| steps(line, Syntax::WANDUR);
        assert_eq!(w("/help"), ["<help None>"]);
        assert_eq!(w("/HELP wait"), ["<help Some(Wait)>"]);
        assert_eq!(
            w("/help /wait"),
            ["<help Some(Wait)>"],
            "the name may keep its character"
        );
        assert_eq!(w("/help count"), ["<help Some(Repeat)>"]);
        assert_eq!(w("look;/help;n"), ["look", "<help None>", "n"]);
        assert_eq!(ok("#help"), ["<help None>"]);
        assert_eq!(w("/helpme"), ["/helpme"], "a whole word only");
        assert_eq!(
            expand("/help dance", Syntax::WANDUR),
            Err(ExpandError::UnknownCommand("dance".into()))
        );
        assert!(
            !uses_commands("#help", Syntax::TINTIN),
            "too common a word to mean a style"
        );
    }

    /// Spans and kinds: certainly wrong, incomplete (more typing can fix it), or a note.
    #[test]
    fn check_tells_wrong_from_incomplete_with_spans() {
        let one = |line: &str, syntax| -> (Problem, Range<usize>, bool) {
            let found = check(line, syntax);
            assert_eq!(found.len(), 1, "{line}: {found:?}");
            let d = found.into_iter().next().unwrap();
            (d.problem, d.span, d.incomplete)
        };
        let refused = |e| Problem::Refused(e);
        for syntax in [Syntax::WANDUR, Syntax::TINTIN, Syntax::MUSH_SAFE] {
            let c = syntax.command;
            let l = |text: &str| crate::client_commands::styled(text, syntax);
            assert_eq!(
                one(&l("/0 look"), syntax),
                (refused(ExpandError::BadCount(format!("{c}0"))), 1..2, false)
            );
            assert_eq!(one(&l("/101 look"), syntax).1, 1..4);
            assert_eq!(
                one(&l("/wait 601"), syntax),
                (refused(ExpandError::BadWait), 6..9, false)
            );
            assert_eq!(one(&l("/wait 0"), syntax).1, 6..7);
            assert_eq!(
                one(&l("/wait 2 x y"), syntax),
                (refused(ExpandError::BadWait), 8..11, false)
            );
            assert_eq!(
                one(&l("/wait {}"), syntax),
                (refused(ExpandError::BadWait), 6..8, false)
            );
            assert_eq!(one(&l("/wait"), syntax), (refused(ExpandError::BadWait), 0..5, true));
            assert!(one(&l("/wait "), syntax).2);
            assert_eq!(
                one(&l("/3 {get coin"), syntax),
                (refused(ExpandError::Unclosed), 3..12, true)
            );
            assert!(one(&l("/wait {the dro"), syntax).2);
            assert_eq!(one(&l("/3"), syntax), (Problem::LoneCount(format!("{c}3")), 0..2, true));
            assert_eq!(one(&l("/3 "), syntax).0, Problem::LoneCount(format!("{c}3")));
            assert!(one(&l("/help wa"), syntax).2, "may become wait");
            assert!(!one(&l("/help dance"), syntax).2);
            let many = l("/100 n;/100 s;look");
            assert_eq!(
                one(&many, syntax),
                (refused(ExpandError::TooMany), 0..many.len(), false)
            );
            for fine in ["/3 look", "/xyz", "say /3", "/wait 2", "/help", "look;n", "//3"] {
                assert!(check(&l(fine), syntax).is_empty(), "{fine}");
            }
            // Expand refuses exactly what check calls wrong or incomplete; a note still sends.
            for line in ["/0 look", "/wait", "/3 {a", "/help x", "/3", "/3 "] {
                let line = l(line);
                assert_eq!(
                    expand(&line, syntax).is_err(),
                    check(&line, syntax).iter().any(Diagnostic::refuses),
                    "{line}"
                );
            }
        }
    }

    #[test]
    fn the_caret_finds_heads_only_at_command_starts() {
        let head = |line: &str, syntax| match context_at(line, line.len(), syntax) {
            Context::Head { range } => Some(range),
            _ => None,
        };
        let w = Syntax::WANDUR;
        assert_eq!(head("/", w), Some(0..1));
        assert_eq!(head("/w", w), Some(0..2));
        assert_eq!(head("  /10", w), Some(2..5));
        assert_eq!(head("get all;/w", w), Some(8..10));
        assert_eq!(head("get all; /", w), Some(9..10));
        assert_eq!(head("/3 {say 1;/wa", w), Some(10..13), "inside a repeat's braces");
        assert_eq!(head("/3 {/", w), Some(4..5));
        assert_eq!(head("say /", w), None, "mid-command");
        assert_eq!(head("/wait {a;/", w), None, "inside wait text");
        assert_eq!(head("say a\\;/", w), None, "an escaped separator starts nothing");
        assert_eq!(head("say {hi;/", w), Some(8..9), "plain braces do not group");
        assert_eq!(head("/3 /", w), None, "a repeated command is text");
        assert_eq!(head("//", w), None, "the literal escape");
        assert_eq!(head("#", w), None, "only the active character");
        assert_eq!(head("#", Syntax::TINTIN), Some(0..1));
        assert_eq!(head("look;/", Syntax::MUSH_SAFE), None, "a single ; is a pose there");
        assert_eq!(head("look;;/", Syntax::MUSH_SAFE), Some(6..7));
        // The caret inside the head.
        assert_eq!(context_at("/wait", 2, w), Context::Head { range: 0..5 });
        assert_eq!(context_at("/wait", 0, w), Context::None);
    }

    #[test]
    fn after_a_head_the_caret_has_a_parameter_and_the_forms_that_fit() {
        let w = Syntax::WANDUR;
        let at_end = |line: &str| context_at(line, line.len(), w);
        let args = |line: &str| match at_end(line) {
            Context::Args {
                id,
                fitting,
                active,
                typed,
                ..
            } => (id, fitting, active, typed),
            other => panic!("{line}: {other:?}"),
        };
        crate::l10n::override_thread(Some(crate::l10n::Language::En));
        assert_eq!(args("/wait "), (CommandId::Wait, vec![0, 1, 2], 0, false));
        assert_eq!(at_end("/wait ").remaining(w).as_deref(), Some("<seconds> {text}"));
        assert_eq!(args("/wait 12"), (CommandId::Wait, vec![0, 2], 0, true));
        assert_eq!(args("/wait 120 "), (CommandId::Wait, vec![0, 2], 1, false));
        assert_eq!(at_end("/wait 120 ").remaining(w).as_deref(), Some("{text}"));
        assert_eq!(args("/wait 120 {the dro"), (CommandId::Wait, vec![0], 1, true));
        assert_eq!(at_end("/wait 120 {the dro").remaining(w), None);
        assert_eq!(args("/wait the"), (CommandId::Wait, vec![1], 0, true));
        assert_eq!(args("/10 "), (CommandId::Repeat, vec![0, 1], 1, false));
        assert_eq!(at_end("/10 ").remaining(w).as_deref(), Some("<command>"));
        assert_eq!(args("/10 {say 1;kill"), (CommandId::Repeat, vec![1], 1, true));
        assert_eq!(
            args("/3 {say 1;/wait "),
            (CommandId::Wait, vec![0, 1, 2], 0, false),
            "the innermost"
        );
        assert_eq!(args("/help "), (CommandId::Help, vec![0, 1], 0, false));
        assert_eq!(at_end("/wait 2;look"), Context::None, "past the separator");
        assert_eq!(at_end("look"), Context::None);
        crate::l10n::override_thread(None);
    }
}
