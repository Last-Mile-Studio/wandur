//! The agent profile of a world (the C# `AgentProfile`, `AgentGoal`, `AgentGoals`,
//! `AgentCatalog`, `AgentConfiguration` and `BuiltInAgentGoals`).
//!
//! Field names in JSON are the C# ones (`Id`, `Provider`, `Endpoint`, ... `CredentialId`), so a
//! stored profile reads the same in both clients. Every length limit counts UTF-16 code units,
//! as C# `string.Length` does.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AgentError, len16};
use crate::l10n::{S, t};

/// The instructions a new profile starts with (the C# default, English as in C#: it is text
/// for the model, not for people).
pub const DEFAULT_SYSTEM_PROMPT: &str = "You are a practical MUD agent. Work toward the user's goal using only the available actions. Observe carefully, avoid needless risk, and stop when the goal is complete or cannot be safely continued.";
/// The allowed commands a new profile starts with: look and the four cardinal directions.
pub const DEFAULT_COMMANDS: &str = "look | look | Observe the current room\nnorth | north | Move north\nsouth | south | Move south\neast | east | Move east\nwest | west | Move west";
/// The server a new profile points at (LM Studio's default port, OpenAI-compatible path).
pub const DEFAULT_ENDPOINT: &str = "http://localhost:1234/v1";
/// Provider keys, as stored.
pub const OPENAI_COMPATIBLE: &str = "openai-compatible";
pub const LMSTUDIO_NATIVE: &str = "lmstudio-native";
/// The reserved actions: observe again without sending, and stop.
pub const WAIT: &str = "wait";
pub const DONE: &str = "done";

/// Limits (the C# checks).
pub const MAX_GOALS: usize = 32;
pub const MAX_GOAL_TEXT: usize = 4000;
pub const MAX_GOAL_NAME: usize = 120;
pub const MAX_MEMORY: usize = 2048;
pub const MAX_REASON: usize = 512;
pub const MAX_COMMANDS: usize = 64;

/// One world's agent settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct AgentProfile {
    pub id: Uuid,
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub system_prompt: String,
    /// Kept to read profiles saved before the goal list (one goal text).
    pub default_goal: String,
    pub goals: Vec<AgentGoal>,
    /// The allowed commands, one `id | command | description` per line.
    pub commands: String,
    pub max_decisions: i32,
    pub max_run_seconds: i32,
    pub action_interval_seconds: i32,
    pub response_timeout_seconds: i32,
    pub max_input_characters: i32,
    pub max_output_tokens: i32,
    pub json_mode: bool,
    /// The vault entry of the optional API key, when one is saved.
    pub credential_id: Option<Uuid>,
}

impl Default for AgentProfile {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            provider: OPENAI_COMPATIBLE.into(),
            endpoint: DEFAULT_ENDPOINT.into(),
            model: String::new(),
            system_prompt: DEFAULT_SYSTEM_PROMPT.into(),
            default_goal: String::new(),
            goals: Vec::new(),
            commands: DEFAULT_COMMANDS.into(),
            max_decisions: 30,
            max_run_seconds: 600,
            action_interval_seconds: 2,
            response_timeout_seconds: 120,
            max_input_characters: 12_000,
            max_output_tokens: 512,
            json_mode: false,
            credential_id: None,
        }
    }
}

/// A goal: `text` is its Markdown description (the stored name `Text` keeps older profiles),
/// `enabled` marks the one selected by default for new sessions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AgentGoal {
    pub id: Uuid,
    pub text: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub rules: String,
}

fn yes() -> bool {
    true
}

impl AgentGoal {
    pub fn new(text: &str, enabled: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            text: text.into(),
            enabled,
            name: String::new(),
            rules: String::new(),
        }
    }

    /// What lists show: the name, or the description when there is no name.
    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.text
        } else {
            &self.name
        }
    }

    /// The goal as the model reads it: the description alone, or the name, description and
    /// rules as one Markdown document.
    pub fn instructions(&self) -> String {
        if self.name.trim().is_empty() && self.rules.trim().is_empty() {
            return self.text.trim().to_string();
        }
        [
            GOAL_HEADING,
            self.name.trim(),
            DESCRIPTION_HEADING,
            self.text.trim(),
            RULES_HEADING,
            self.rules.trim(),
        ]
        .concat()
    }
}

/// The goal document's headings (Markdown for the model, as C# writes it).
const GOAL_HEADING: &str = "# Goal: ";
const DESCRIPTION_HEADING: &str = "\n\n## Description\n";
const RULES_HEADING: &str = "\n\n## Rules\n";

/// The goals of a profile: its list (only the first default kept selected), or the one goal
/// text of an older profile.
pub fn goals_of(profile: &AgentProfile) -> Vec<AgentGoal> {
    if !profile.goals.is_empty() {
        return single_default(profile.goals.clone());
    }
    if profile.default_goal.trim().is_empty() {
        return Vec::new();
    }
    vec![AgentGoal {
        id: profile.id,
        text: profile.default_goal.clone(),
        enabled: true,
        name: String::new(),
        rules: String::new(),
    }]
}

/// Older clients allowed several defaults: every goal is kept, only the first stays selected.
pub fn single_default(mut goals: Vec<AgentGoal>) -> Vec<AgentGoal> {
    let mut selected = false;
    for goal in &mut goals {
        if !goal.enabled {
            continue;
        }
        if selected {
            goal.enabled = false;
        }
        selected = true;
    }
    goals
}

/// The selected goal's instructions ("" when none is selected). Two selected goals are refused:
/// only one goal runs at a time.
pub fn compose(goals: &[AgentGoal]) -> Result<String, AgentError> {
    let mut selected = goals.iter().filter(|g| g.enabled);
    let first = selected.next();
    if selected.next().is_some() {
        return Err(AgentError::Invalid("Only one goal can run at a time."));
    }
    Ok(first.map(AgentGoal::instructions).unwrap_or_default())
}

/// One allowed action: its id (what the model names), the fixed command sent, and what it means.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AgentCommand {
    pub id: String,
    pub command: String,
    pub description: String,
}

/// Read the allowed commands, one `id | command | description` per line. Ids are unique (case
/// does not matter), use letters, digits, `_` or `-`, and are never `wait` or `done`. Commands
/// are single fixed commands: no chaining, placeholders, client commands or control characters.
pub fn parse_catalog(text: &str) -> Result<Vec<AgentCommand>, AgentError> {
    if len16(text) > 16_000 {
        return Err(AgentError::Invalid("Invalid command catalog."));
    }
    let mut result: Vec<AgentCommand> = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for line in text.replace("\r\n", "\n").split('\n') {
        if line.trim().is_empty() {
            continue;
        }
        if line.chars().any(char::is_control) {
            return Err(AgentError::Invalid("Commands must not contain control characters."));
        }
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() != 3 {
            return Err(AgentError::Invalid("Commands must use: id | command | description."));
        }
        let (id, command, description) = (parts[0].trim(), parts[1].trim(), parts[2].trim());
        let id_ok = (1..=64).contains(&len16(id))
            && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            && !id.eq_ignore_ascii_case(WAIT)
            && !id.eq_ignore_ascii_case(DONE)
            && ids.insert(id.to_ascii_lowercase());
        if !id_ok {
            return Err(AgentError::Invalid(
                "Command IDs must be unique, use letters, digits, underscores or dashes, and not use wait or done.",
            ));
        }
        let command_ok = (1..=256).contains(&len16(command))
            && !command
                .chars()
                .any(|c| matches!(c, ';' | '|' | '&' | '`' | '{' | '}') || c.is_control())
            && !command.starts_with('#')
            && !command.starts_with('/')
            && len16(description) <= 512;
        if !command_ok {
            return Err(AgentError::Invalid(
                "Commands must be single, fixed MUD commands without chaining or placeholders.",
            ));
        }
        result.push(AgentCommand {
            id: id.into(),
            command: command.into(),
            description: description.into(),
        });
        if result.len() > MAX_COMMANDS {
            return Err(AgentError::Invalid("At most 64 agent commands are supported."));
        }
    }
    if result.is_empty() {
        return Err(AgentError::Invalid("At least one agent command is required."));
    }
    Ok(result)
}

/// Whether `endpoint` is an absolute HTTP or HTTPS address without credentials, query or
/// fragment.
pub fn valid_endpoint(endpoint: &str) -> bool {
    if endpoint.len() > 2048 || endpoint.chars().any(char::is_control) {
        return false;
    }
    let Ok(url) = url::Url::parse(endpoint) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some_and(|h| !h.is_empty())
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

/// Check a profile (the C# `AgentConfiguration.Validate`). `require_model` is off while
/// settings are edited and models are discovered.
pub fn validate(profile: &AgentProfile, require_model: bool) -> Result<(), AgentError> {
    if profile.id.is_nil() || profile.credential_id.is_some_and(|c| c.is_nil()) {
        return Err(AgentError::Invalid("Invalid agent profile identity."));
    }
    if profile.provider.trim().is_empty()
        || len16(&profile.provider) > 64
        || profile.provider.chars().any(char::is_control)
    {
        return Err(AgentError::Invalid("Invalid agent provider."));
    }
    if !valid_endpoint(&profile.endpoint) {
        return Err(AgentError::Invalid(
            "Endpoint must be an absolute HTTP or HTTPS URL without credentials, query or fragment.",
        ));
    }
    if len16(&profile.model) > 256
        || profile.model.chars().any(char::is_control)
        || (require_model && profile.model.trim().is_empty())
    {
        return Err(AgentError::Invalid("Select a valid model."));
    }
    if len16(&profile.system_prompt) > 8000 || len16(&profile.default_goal) > MAX_GOAL_TEXT {
        return Err(AgentError::Invalid("Agent instructions are too long."));
    }
    let mut ids = std::collections::HashSet::new();
    let goals_ok = profile.goals.len() <= MAX_GOALS
        && profile.goals.iter().all(|g| {
            !g.id.is_nil()
                && !g.text.trim().is_empty()
                && len16(&g.text) <= MAX_GOAL_TEXT
                && len16(&g.name) <= MAX_GOAL_NAME
                && len16(&g.rules) <= MAX_GOAL_TEXT
                && len16(&g.instructions()) <= MAX_GOAL_TEXT
                && ids.insert(g.id)
        })
        && profile.goals.iter().filter(|g| g.enabled).count() <= 1;
    if !goals_ok {
        return Err(AgentError::Invalid("Invalid agent goals."));
    }
    let in_range = |v: i32, lo: i32, hi: i32| (lo..=hi).contains(&v);
    if !(in_range(profile.max_decisions, 1, 1000)
        && in_range(profile.max_run_seconds, 1, 86_400)
        && in_range(profile.action_interval_seconds, 1, 3600)
        && in_range(profile.response_timeout_seconds, 1, 600)
        && in_range(profile.max_input_characters, 1024, 128_000)
        && in_range(profile.max_output_tokens, 64, 8192))
    {
        return Err(AgentError::Invalid("Agent limits are outside supported ranges."));
    }
    parse_catalog(&profile.commands)?;
    Ok(())
}

/// The starter goals Add template... offers, by key.
pub const TEMPLATES: [(&str, S); 3] = [
    ("observe", S::AgentTemplateObserve),
    ("explore", S::AgentTemplateExplore),
    ("inventory", S::AgentTemplateInventory),
];

/// A starter goal (the C# `BuiltInAgentGoals`): an editable copy with a fresh id, never
/// selected, in the UI language. Templates never change the allowed commands.
pub fn template(key: &str) -> Result<AgentGoal, AgentError> {
    let (name, description, rules) = match key {
        "observe" => (
            S::AgentTemplateObserve,
            S::AgentTemplateObserveDescription,
            S::AgentTemplateObserveRules,
        ),
        "explore" => (
            S::AgentTemplateExplore,
            S::AgentTemplateExploreDescription,
            S::AgentTemplateExploreRules,
        ),
        "inventory" => (
            S::AgentTemplateInventory,
            S::AgentTemplateInventoryDescription,
            S::AgentTemplateInventoryRules,
        ),
        _ => return Err(AgentError::Invalid("Unknown goal template.")),
    };
    Ok(AgentGoal {
        id: Uuid::new_v4(),
        text: t(description).into(),
        enabled: false,
        name: t(name).into(),
        rules: t(rules).into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> AgentProfile {
        AgentProfile {
            model: "gemma-12b".into(),
            ..AgentProfile::default()
        }
    }

    #[test]
    fn the_defaults_are_the_c_sharp_ones_and_valid() {
        let p = profile();
        assert_eq!(p.provider, OPENAI_COMPATIBLE);
        assert_eq!(p.endpoint, "http://localhost:1234/v1");
        assert_eq!(p.response_timeout_seconds, 120);
        assert_eq!(p.max_input_characters, 12_000);
        assert_eq!(parse_catalog(&p.commands).unwrap().len(), 5);
        validate(&p, true).unwrap();
        assert!(validate(&AgentProfile::default(), true).is_err());
        validate(&AgentProfile::default(), false).unwrap();
    }

    /// AgentProviderTests.CatalogRejectsUnsafeOrAmbiguousCommands.
    #[test]
    fn the_catalog_rejects_unsafe_or_ambiguous_commands() {
        for catalog in [
            "look | look;quit | Chain",
            "look | look | First\nlook | north | Duplicate",
            "done | quit | Reserved",
            "look | look\tquit | Control",
            "look | {target} | Dynamic",
            "LOOK | look | a\nlook | look | b",
            "look | /quit | Client command",
            "look | #3 north | Repeat",
            "look | look",
            "",
        ] {
            assert!(parse_catalog(catalog).is_err(), "{catalog:?}");
        }
        let many: String = (0..65).map(|i| format!("a{i} | look | x\n")).collect();
        assert!(parse_catalog(&many).is_err());
        let ok: String = (0..64).map(|i| format!("a{i} | look | x\r\n")).collect();
        assert_eq!(parse_catalog(&ok).unwrap().len(), 64);
        let parsed = parse_catalog(" buy_bread | buy bread | Buy food \n\n").unwrap();
        assert_eq!(parsed[0].id, "buy_bread");
        assert_eq!(parsed[0].command, "buy bread");
        assert_eq!(parsed[0].description, "Buy food");
    }

    /// AgentProviderTests.ConfigurationRejectsUnsafeEndpoints.
    #[test]
    fn configuration_rejects_unsafe_endpoints() {
        for endpoint in [
            "file:///tmp/model",
            "http://user:secret@localhost:1234/v1",
            "http://localhost:1234/v1?secret=key",
            "http://localhost:1234/v1#fragment",
            "localhost:1234",
            "",
        ] {
            let p = AgentProfile {
                endpoint: endpoint.into(),
                ..profile()
            };
            assert!(validate(&p, true).is_err(), "{endpoint}");
        }
        assert!(valid_endpoint("https://models.example.net/v1"));
        assert!(valid_endpoint("http://192.168.1.20:1234/api/v1"));
    }

    #[test]
    fn limits_and_goals_are_checked() {
        let p = AgentProfile {
            max_decisions: 0,
            ..profile()
        };
        assert!(validate(&p, true).is_err());
        let p = AgentProfile {
            max_input_characters: 1023,
            ..profile()
        };
        assert!(validate(&p, true).is_err());
        let two = AgentProfile {
            goals: vec![AgentGoal::new("One", true), AgentGoal::new("Two", true)],
            ..profile()
        };
        assert!(validate(&two, true).is_err());
        let blank = AgentProfile {
            goals: vec![AgentGoal::new("  ", false)],
            ..profile()
        };
        assert!(validate(&blank, true).is_err());
        let long = AgentProfile {
            goals: vec![AgentGoal {
                name: "n".repeat(10),
                rules: "r".repeat(2000),
                ..AgentGoal::new(&"d".repeat(2000), false)
            }],
            ..profile()
        };
        assert!(
            validate(&long, true).is_err(),
            "name, description and rules share 4,000"
        );
    }

    /// AgentProfileStoreTests: combining objectives is refused; the Markdown document.
    #[test]
    fn goals_compose_one_markdown_document() {
        let first = AgentGoal {
            name: "Explore".into(),
            rules: "- Do not fight".into(),
            ..AgentGoal::new("Explore carefully", true)
        };
        let second = AgentGoal::new("Observe the room", true);
        assert!(compose(&[first.clone(), second.clone()]).is_err());
        let off = AgentGoal {
            enabled: false,
            ..second.clone()
        };
        assert_eq!(
            compose(&[first.clone(), off.clone()]).unwrap(),
            "# Goal: Explore\n\n## Description\nExplore carefully\n\n## Rules\n- Do not fight"
        );
        assert_eq!(compose(&[off]).unwrap(), "");
        assert_eq!(second.instructions(), "Observe the room");
        assert_eq!(first.display_name(), "Explore");
        assert_eq!(second.display_name(), "Observe the room");
    }

    #[test]
    fn older_profiles_keep_every_goal_and_select_the_first() {
        let goals = single_default(vec![
            AgentGoal::new("One", true),
            AgentGoal::new("Two", true),
            AgentGoal::new("Three", false),
        ]);
        assert_eq!(
            goals.iter().map(|g| g.enabled).collect::<Vec<_>>(),
            [true, false, false]
        );
        let legacy = AgentProfile {
            default_goal: "Explore".into(),
            ..profile()
        };
        let goals = goals_of(&legacy);
        assert_eq!(goals.len(), 1);
        assert_eq!(goals[0].text, "Explore");
        assert!(goals[0].enabled);
        assert_eq!(goals[0].id, legacy.id);
        assert!(goals_of(&profile()).is_empty());
    }

    /// AgentSettingsTests: templates are independent editable copies with Markdown.
    #[test]
    fn templates_are_fresh_copies() {
        let a = template("explore").unwrap();
        let b = template("explore").unwrap();
        assert_ne!(a.id, b.id);
        assert!(!a.enabled);
        assert!(a.text.contains("##"));
        assert!(a.rules.contains("- "));
        assert_eq!(a.name, t(S::AgentTemplateExplore));
        assert!(template("attack").is_err());
        for (key, _) in TEMPLATES {
            let goal = template(key).unwrap();
            let p = AgentProfile {
                goals: vec![goal],
                ..profile()
            };
            validate(&p, true).unwrap();
        }
    }

    #[test]
    fn json_uses_the_c_sharp_names() {
        let p = AgentProfile {
            goals: vec![AgentGoal::new("Explore", true)],
            ..profile()
        };
        let json = serde_json::to_string(&p).unwrap();
        for name in [
            "\"Id\"",
            "\"Provider\"",
            "\"SystemPrompt\"",
            "\"MaxDecisions\"",
            "\"ResponseTimeoutSeconds\"",
            "\"JsonMode\"",
            "\"CredentialId\":null",
            "\"Goals\":[{\"Id\"",
            "\"Text\":\"Explore\"",
        ] {
            assert!(json.contains(name), "{name} in {json}");
        }
        let back: AgentProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        // A C# profile with only some fields reads with the defaults for the rest.
        let partial: AgentProfile = serde_json::from_str(
            r#"{"Id":"0f8fad5b-d9cb-469f-a165-70867728950e","Model":"m","Goals":[{"Id":"7c9e6679-7425-40de-944b-e07fc1f90ae7","Text":"Go"}]}"#,
        )
        .unwrap();
        assert_eq!(partial.max_decisions, 30);
        assert!(partial.goals[0].enabled);
        assert_eq!(partial.goals[0].name, "");
    }
}
