//! The script editor's API completion (the C# `ScriptCompletionCatalog`): local hints for `mud.`,
//! `Events.` and the fields of an event callback's parameter, not a JavaScript language server.

use std::sync::LazyLock;

use regex::Regex;

use crate::l10n::{S, t};

/// One suggestion: the name inserted, the signature shown, a description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub name: &'static str,
    pub signature: &'static str,
    pub description: &'static str,
}

/// What to offer at the caret: how many characters before it the suggestion replaces, and the
/// suggestions whose names start with them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Context {
    pub prefix_len: usize,
    pub suggestions: Vec<Suggestion>,
}

/// Completion looks at most this far back (the largest script must stay cheap).
pub const MAX_CONTEXT: usize = 8192;

static MEMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:(?<receiver>[$A-Za-z_][$\w]*)\s*\.\s*)?(?<prefix>[$A-Za-z_][$\w]*)?$").expect("member pattern")
});

static SUBSCRIPTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"mud\s*\.\s*on\s*\(\s*(?:Events\s*\.\s*(?<kind>Line|Prompt|Gmcp|Msdp)|["'](?<legacy>line|prompt|gmcp|msdp)["'])\s*,\s*(?:function\s*)?\(?\s*(?<parameter>[$A-Za-z_][$\w]*)\s*\)?\s*(?:=>|\{)"#,
    )
    .expect("subscription pattern")
});

fn s(name: &'static str, signature: &'static str, key: S) -> Suggestion {
    Suggestion {
        name,
        signature,
        description: t(key),
    }
}

/// The suggestions for the text before the caret.
pub fn complete(before_caret: &str) -> Context {
    let mut start = before_caret.len().saturating_sub(MAX_CONTEXT);
    while !before_caret.is_char_boundary(start) {
        start += 1;
    }
    let context = &before_caret[start..];
    let Some(found) = MEMBER.captures(context) else {
        return Context::default();
    };
    let receiver = found.name("receiver").map_or("", |m| m.as_str());
    let prefix = found.name("prefix").map_or("", |m| m.as_str());
    let choices = match receiver {
        "mud" => vec![
            s("on", "on(event, callback)", S::ScriptCompleteOn),
            s("send", "send(command)", S::ScriptCompleteSend),
            s("echo", "echo(text)", S::ScriptCompleteEcho),
            s("alias", "alias(pattern, callback)", S::ScriptCompleteAlias),
            s("trigger", "trigger(pattern, callback)", S::ScriptCompleteTrigger),
            s("every", "every(seconds, callback)", S::ScriptCompleteEvery),
            s("after", "after(seconds, callback)", S::ScriptCompleteAfter),
            s("remove", "remove(id)", S::ScriptCompleteRemove),
            s("panel", "panel(id, options)", S::ScriptCompletePanel),
            s("format", "format(value)", S::ScriptCompleteFormat),
            s("state", "state", S::ScriptCompleteState),
        ],
        "Events" => vec![
            s("Line", "Line", S::ScriptCompleteLine),
            s("Prompt", "Prompt", S::ScriptCompletePrompt),
            s("Gmcp", "Gmcp", S::ScriptCompleteGmcp),
            s("Msdp", "Msdp", S::ScriptCompleteMsdp),
        ],
        "" => vec![
            s("mud", "mud", S::ScriptCompleteMud),
            s("Events", "Events", S::ScriptCompleteEvents),
        ],
        parameter => {
            let Some(subscription) = SUBSCRIPTION.captures_iter(context).last() else {
                return empty(prefix);
            };
            if subscription.name("parameter").map(|m| m.as_str()) != Some(parameter) {
                return empty(prefix);
            }
            let kind = subscription
                .name("kind")
                .or_else(|| subscription.name("legacy"))
                .map_or(String::new(), |m| m.as_str().to_ascii_lowercase());
            match kind.as_str() {
                "line" => vec![s("text", "text: string", S::ScriptCompleteText)],
                "prompt" => vec![s("text", "text: string", S::ScriptCompletePromptText)],
                "msdp" => vec![
                    s("variable", "variable: string", S::ScriptCompleteVariable),
                    s("value", "value: JSON", S::ScriptCompleteValue),
                ],
                _ => vec![
                    s("package", "package: string", S::ScriptCompletePackage),
                    s("data", "data: JSON | null", S::ScriptCompleteData),
                ],
            }
        }
    };
    let lower = prefix.to_lowercase();
    Context {
        prefix_len: prefix.len(),
        suggestions: choices
            .into_iter()
            .filter(|c| c.name.to_lowercase().starts_with(&lower))
            .collect(),
    }
}

fn empty(prefix: &str) -> Context {
    Context {
        prefix_len: prefix.len(),
        suggestions: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(text: &str) -> Vec<&'static str> {
        complete(text).suggestions.iter().map(|s| s.name).collect()
    }

    #[test]
    fn members_of_mud_and_events_and_the_globals() {
        assert_eq!(
            names("mud."),
            [
                "on", "send", "echo", "alias", "trigger", "every", "after", "remove", "panel", "format", "state"
            ]
        );
        let tr = complete("draw();\nmud.tr");
        assert_eq!(tr.prefix_len, 2);
        assert_eq!(tr.suggestions.iter().map(|s| s.name).collect::<Vec<_>>(), ["trigger"]);
        assert_eq!(tr.suggestions[0].signature, "trigger(pattern, callback)");
        assert_eq!(names("mud.on(Events."), ["Line", "Prompt", "Gmcp", "Msdp"]);
        assert_eq!(names("mud.on(Events.g"), ["Gmcp"], "case-insensitive prefix");
        assert_eq!(names("const x = "), ["mud", "Events"]);
        assert_eq!(names("const x = E"), ["Events"]);
        assert!(names("foo.").is_empty(), "an unknown receiver");
        assert!(names("mud.zz").is_empty());
    }

    #[test]
    fn event_parameters_get_their_fields() {
        assert_eq!(names("mud.on(Events.Line, event => { event."), ["text"]);
        assert_eq!(names("mud.on(Events.Msdp, e => e."), ["variable", "value"]);
        assert_eq!(names("mud.on('gmcp', function (m) { m.d"), ["data"]);
        assert_eq!(names("mud.on(Events.Prompt, (p) => p."), ["text"]);
        assert!(names("mud.on(Events.Line, event => { other.").is_empty());
    }

    #[test]
    fn a_huge_script_only_looks_back_a_little() {
        let big = format!("{}mud.", "é".repeat(20_000));
        assert_eq!(names(&big).len(), 11);
    }
}
