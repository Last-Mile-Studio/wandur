//! The command line's own commands, one entry each: how the head is spelled, the forms it takes
//! with their parameters, a summary and examples. The parser ([`crate::command_line`]) recognizes
//! heads through this table, and the command box lists, explains and checks from it, so the list
//! cannot drift from what is sent. Adding a command is one entry here, its arm in the parser and
//! its strings.
//!
//! Spellings here use Wandur's own style (`/` and `;`); [`styled`] turns them into the active one.

use crate::command_line::{DEFAULT_WAIT, MAX_REPEAT, MAX_WAIT_SECS, Syntax};
use crate::l10n::{S, t, tf};

/// Which command an entry is (the parser's arm for it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandId {
    /// `/10 say 1`, `/3 {a;b}`.
    Repeat,
    /// `/help`, `/help wait`.
    Help,
    /// `/wait {text}`, `/wait 120 {text}`, `/wait 2`.
    Wait,
}

/// How a command's head is spelled after the command character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Head {
    /// Digits, the repeat count (`/10`).
    Count,
    /// A word, any case, standing alone: followed by whitespace, the separator or the end.
    Word(&'static str),
}

/// What one parameter takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// The repeat count, part of the head (`/10`).
    Count,
    /// A whole number of seconds.
    Seconds,
    /// Text to the next separator, or anything in braces.
    Text,
    /// One command to the next separator.
    Command,
    /// A `{...}` sequence of commands, which may hold more client commands.
    Group,
    /// A command name from this table (`/help wait`).
    Name,
}

#[derive(Clone, Copy, Debug)]
pub struct Param {
    pub kind: ParamKind,
    /// Shown in signature help: "count", "seconds" (localized).
    pub label: S,
}

/// One spelling of a command, as signature help shows it: `/wait <seconds> {text}`.
#[derive(Clone, Copy, Debug)]
pub struct Form {
    pub params: &'static [Param],
    /// What this form does, one line (localized; `{0}` is the limit that applies).
    pub summary: S,
}

#[derive(Debug)]
pub struct CommandSpec {
    pub id: CommandId,
    pub head: Head,
    /// One line for the list (localized).
    pub summary: S,
    /// What one types, in Wandur's style; never translated.
    pub examples: &'static [&'static str],
    pub forms: &'static [Form],
}

const COUNT: Param = Param {
    kind: ParamKind::Count,
    label: S::CmdParamCount,
};
const COMMAND: Param = Param {
    kind: ParamKind::Command,
    label: S::CmdParamCommand,
};
const GROUP: Param = Param {
    kind: ParamKind::Group,
    label: S::CmdParamCommands,
};
const SECONDS: Param = Param {
    kind: ParamKind::Seconds,
    label: S::CmdParamSeconds,
};
const TEXT: Param = Param {
    kind: ParamKind::Text,
    label: S::CmdParamText,
};
const NAME: Param = Param {
    kind: ParamKind::Name,
    label: S::CmdParamCommand,
};

pub static REPEAT: CommandSpec = CommandSpec {
    id: CommandId::Repeat,
    head: Head::Count,
    summary: S::CmdRepeatSummary,
    examples: &["/10 say 1", "/3 {get coin;put coin bag}"],
    forms: &[
        Form {
            params: &[COUNT, COMMAND],
            summary: S::CmdRepeatOne,
        },
        Form {
            params: &[COUNT, GROUP],
            summary: S::CmdRepeatGroup,
        },
    ],
};

pub static HELP: CommandSpec = CommandSpec {
    id: CommandId::Help,
    head: Head::Word("help"),
    summary: S::CmdHelpSummary,
    examples: &["/help wait", "/help"],
    forms: &[
        Form {
            params: &[NAME],
            summary: S::CmdHelpOne,
        },
        Form {
            params: &[],
            summary: S::CmdHelpAll,
        },
    ],
};

pub static WAIT: CommandSpec = CommandSpec {
    id: CommandId::Wait,
    head: Head::Word("wait"),
    summary: S::CmdWaitSummary,
    examples: &["/wait 2", "/wait {the droid is dead}", "/wait 120 {the droid is dead}"],
    forms: &[
        Form {
            params: &[SECONDS, TEXT],
            summary: S::CmdWaitTextFor,
        },
        Form {
            params: &[TEXT],
            summary: S::CmdWaitText,
        },
        Form {
            params: &[SECONDS],
            summary: S::CmdWaitSeconds,
        },
    ],
};

/// Every command, in the order the list shows them: the repeat first, then by name.
pub static COMMANDS: [&CommandSpec; 3] = [&REPEAT, &HELP, &WAIT];

impl CommandSpec {
    pub fn get(id: CommandId) -> &'static CommandSpec {
        match id {
            CommandId::Repeat => &REPEAT,
            CommandId::Help => &HELP,
            CommandId::Wait => &WAIT,
        }
    }

    /// The command named `name` (without the command character, any case): a word head, or
    /// `count` for the repeat.
    pub fn named(name: &str) -> Option<&'static CommandSpec> {
        COMMANDS.into_iter().find(|c| c.name().eq_ignore_ascii_case(name))
    }

    /// The name `/help` takes: the word, or `count` for the repeat.
    pub fn name(&self) -> &'static str {
        match self.head {
            Head::Count => "count",
            Head::Word(word) => word,
        }
    }

    /// The head as the list shows it in `syntax`: `/wait`, or `/<count>` for the repeat.
    pub fn display(&self, syntax: Syntax) -> String {
        match self.head {
            Head::Count => format!("{}<{}>", syntax.command, t(S::CmdParamCount)),
            Head::Word(word) => format!("{}{word}", syntax.command),
        }
    }

    /// One form as signature help shows it: `/wait <seconds> {text}`, `/<count> {commands}`.
    pub fn syntax_line(&self, form: &Form, syntax: Syntax) -> String {
        let mut line = String::new();
        line.push(syntax.command);
        if let Head::Word(word) = self.head {
            line.push_str(word);
        }
        for (i, param) in form.params.iter().enumerate() {
            if !(i == 0 && param.kind == ParamKind::Count) {
                line.push(' ');
            }
            line.push_str(&param.placeholder(syntax));
        }
        line
    }

    /// A form's summary with its limit filled in.
    pub fn form_summary(form: &Form) -> String {
        match form.summary {
            S::CmdRepeatOne => tf(form.summary, &[&MAX_REPEAT]),
            S::CmdWaitText => tf(form.summary, &[&DEFAULT_WAIT.as_secs()]),
            S::CmdWaitSeconds => tf(form.summary, &[&MAX_WAIT_SECS]),
            key => t(key).to_string(),
        }
    }

    /// The examples in `syntax`.
    pub fn examples_in(&self, syntax: Syntax) -> impl Iterator<Item = String> + '_ {
        self.examples.iter().map(move |e| styled(e, syntax))
    }
}

impl Param {
    /// How signature help writes the parameter: `<seconds>`, `{text}`, `{commands}`.
    pub fn placeholder(&self, syntax: Syntax) -> String {
        let label = t(self.label);
        match self.kind {
            ParamKind::Text => format!("{{{label}}}"),
            ParamKind::Group => format!("{{{label}{}…}}", syntax.separator),
            _ => format!("<{label}>"),
        }
    }
}

/// What `/help` prints in the transcript, line by line: every command (its name and summary,
/// then its forms and an example), or one command's forms and examples.
pub fn help_lines(topic: Option<CommandId>, syntax: Syntax) -> Vec<String> {
    let specs: Vec<&CommandSpec> = match topic {
        Some(id) => vec![CommandSpec::get(id)],
        None => COMMANDS.to_vec(),
    };
    let forms: Vec<(String, String)> = specs
        .iter()
        .flat_map(|spec| {
            spec.forms
                .iter()
                .map(|form| (spec.syntax_line(form, syntax), CommandSpec::form_summary(form)))
        })
        .collect();
    let width = forms.iter().map(|(line, _)| line.chars().count()).max().unwrap_or(0);
    let mut out = Vec::new();
    if topic.is_none() {
        let c = syntax.command;
        out.push(tf(S::CmdHelpHeader, &[&c, &syntax.separator]));
    }
    let mut forms = forms.into_iter();
    for spec in specs {
        if topic.is_none() {
            out.push(format!("{}  {}", spec.display(syntax), t(spec.summary)));
        }
        for (line, summary) in forms.by_ref().take(spec.forms.len()) {
            out.push(format!("  {line:<width$}  {summary}"));
        }
        let examples: Vec<String> = spec.examples_in(syntax).collect();
        let shown = if topic.is_some() { &examples[..] } else { &examples[..1] };
        for example in shown {
            out.push(format!("  {}", tf(S::CmdHelpExample, &[example])));
        }
    }
    out
}

/// A spelling from this table (`/3 {a;b}`) in another style: its `/` as the command character,
/// its `;` as the separator.
pub fn styled(text: &str, syntax: Syntax) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '/' => out.push(syntax.command),
            ';' => out.push_str(syntax.separator),
            c => out.push(c),
        }
    }
    out
}

/// The commands a typed head could become, in list order. `prefix` starts with the command
/// character: alone it matches every command, followed by digits only the repeat, followed by
/// letters the words that start with them (any case).
pub fn matching(prefix: &str, syntax: Syntax) -> Vec<&'static CommandSpec> {
    let Some(rest) = prefix.strip_prefix(syntax.command) else {
        return Vec::new();
    };
    COMMANDS
        .into_iter()
        .filter(|c| match c.head {
            Head::Count => rest.bytes().all(|b| b.is_ascii_digit()),
            Head::Word(word) => word.len() >= rest.len() && word[..rest.len()].eq_ignore_ascii_case(rest),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_line::expand;
    use crate::l10n::{Language, text_in};

    const STYLES: [Syntax; 3] = [Syntax::WANDUR, Syntax::TINTIN, Syntax::MUSH_SAFE];

    #[test]
    fn the_registry_is_whole_and_its_examples_expand_in_every_style() {
        let mut names: Vec<&str> = COMMANDS.iter().map(|c| c.name()).collect();
        names.dedup();
        assert_eq!(names.len(), COMMANDS.len(), "unique heads");
        for spec in COMMANDS {
            assert!(std::ptr::eq(CommandSpec::get(spec.id), spec));
            assert!(!spec.forms.is_empty() && !spec.examples.is_empty(), "{}", spec.name());
            let mut keys = vec![spec.summary];
            for form in spec.forms {
                keys.push(form.summary);
                keys.extend(form.params.iter().map(|p| p.label));
            }
            for key in keys {
                for language in Language::ALL {
                    assert!(!text_in(language, key).trim().is_empty(), "{key:?} in {language:?}");
                }
            }
            for syntax in STYLES {
                for example in spec.examples_in(syntax) {
                    assert!(expand(&example, syntax).is_ok(), "{example} in {syntax:?}");
                }
            }
        }
    }

    #[test]
    fn heads_filter_by_prefix_in_any_case() {
        let names = |prefix: &str, syntax| -> Vec<&str> { matching(prefix, syntax).iter().map(|c| c.name()).collect() };
        assert_eq!(names("/", Syntax::WANDUR), ["count", "help", "wait"], "all, in order");
        assert_eq!(names("/1", Syntax::WANDUR), ["count"]);
        assert_eq!(names("/10", Syntax::WANDUR), ["count"]);
        assert_eq!(names("/W", Syntax::WANDUR), ["wait"]);
        assert_eq!(names("/he", Syntax::MUSH_SAFE), ["help"]);
        assert_eq!(names("/wait", Syntax::WANDUR), ["wait"]);
        assert!(names("/x", Syntax::WANDUR).is_empty());
        assert!(names("/waits", Syntax::WANDUR).is_empty());
        assert!(names("/1a", Syntax::WANDUR).is_empty());
        assert_eq!(names("#w", Syntax::TINTIN), ["wait"]);
        assert!(names("#w", Syntax::WANDUR).is_empty(), "only the active character");
    }

    #[test]
    fn help_lists_every_command_or_explains_one_in_the_active_style() {
        crate::l10n::override_thread(Some(Language::En));
        let all = help_lines(None, Syntax::TINTIN);
        assert_eq!(
            all[0],
            "Wandur’s commands start with #; ; separates commands and ## sends a #. #help wait explains one."
        );
        assert!(all.contains(&"#wait  Wait for text or time".to_string()), "{all:#?}");
        assert!(all.contains(&"#<count>  Repeat a command".to_string()));
        assert!(all.iter().any(|l| l.starts_with("  #wait <seconds> {text}  ")));
        assert!(all.contains(&"  Example: #10 say 1".to_string()));
        let wait = help_lines(Some(CommandId::Wait), Syntax::MUSH_SAFE);
        assert_eq!(
            wait,
            [
                "  /wait <seconds> {text}  Hold the rest until a line contains the text, up to this many seconds",
                "  /wait {text}            Hold the rest until a line contains the text, 60 s at most",
                "  /wait <seconds>         Pause this many seconds, 1 to 600",
                "  Example: /wait 2",
                "  Example: /wait {the droid is dead}",
                "  Example: /wait 120 {the droid is dead}",
            ]
        );
        crate::l10n::override_thread(None);
    }

    #[test]
    fn forms_read_as_typed_with_translated_placeholders() {
        crate::l10n::override_thread(Some(Language::En));
        assert_eq!(
            WAIT.syntax_line(&WAIT.forms[0], Syntax::WANDUR),
            "/wait <seconds> {text}"
        );
        assert_eq!(WAIT.syntax_line(&WAIT.forms[1], Syntax::TINTIN), "#wait {text}");
        assert_eq!(
            REPEAT.syntax_line(&REPEAT.forms[0], Syntax::WANDUR),
            "/<count> <command>"
        );
        assert_eq!(
            REPEAT.syntax_line(&REPEAT.forms[1], Syntax::MUSH_SAFE),
            "/<count> {commands;;…}"
        );
        assert_eq!(HELP.syntax_line(&HELP.forms[1], Syntax::WANDUR), "/help");
        assert_eq!(REPEAT.display(Syntax::TINTIN), "#<count>");
        assert_eq!(styled("/3 {a;b}", Syntax::MUSH_SAFE), "/3 {a;;b}");
        crate::l10n::override_thread(Some(Language::De));
        assert_eq!(
            WAIT.syntax_line(&WAIT.forms[0], Syntax::WANDUR),
            "/wait <Sekunden> {Text}"
        );
        crate::l10n::override_thread(None);
    }
}
