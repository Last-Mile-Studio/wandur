//! Local model agents (the C# `Wandur.Core.Agents`): a model chooses one fixed, allowed command
//! at a time for one open connection.
//!
//! - [`profile`]: the per-world agent profile (server, provider, model, instructions, goals,
//!   allowed commands, limits), goals with Markdown descriptions and rules, the command catalog
//!   and the checks every profile passes.
//! - [`decision_codec`]: the prompt sent to the model (instructions, the catalog, the goal, the
//!   memory and the newest observation that fits the input budget) and the strict check of the
//!   decision it returns.
//! - [`providers`]: LM Studio's native API and the OpenAI-compatible API over HTTP, with model
//!   discovery, a response size limit, the response timeout and one request at a time per
//!   server.
//! - [`runner`]: the decision loop of one connection, driven from the UI thread: model calls run
//!   on their own threads and the loop never waits on them.
//! - [`store`]: profiles in `wandur.db` (`world_agent_profiles`), one per world.
//! - [`credentials`]: the optional API key, in the system vault, bound to the profile, its
//!   provider and its endpoint.
//!
//! The model never sends text of its own: it names an action id from the catalog, `wait` or
//! `done`, and the client sends that action's fixed command. Private input, login text and chat
//! never reach it (the app feeds only public output).

pub mod credentials;
pub mod decision_codec;
pub mod profile;
pub mod providers;
pub mod runner;
pub mod store;

mod agent_http;

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub use profile::{AgentCommand, AgentGoal, AgentProfile};
pub use providers::{AgentModelProvider, LmStudioNativeProvider, OpenAiCompatibleProvider, ProviderRegistry};
pub use runner::{AgentGateway, AgentObservation, AgentRunMode, AgentRunner, AgentStatus};
pub use store::{AgentProfileStore, AgentWorld, MemoryAgentProfileStore, SqliteAgentProfileStore};

/// Why an agent call or check failed. The messages are the C# exception messages (English,
/// for logs and tests); people see the runner's status or the settings' own translated text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentError {
    /// A profile, a catalog, a goal or a request outside its limits (C# `ArgumentException`).
    Invalid(&'static str),
    /// The model server answered something that is not one complete, valid decision or model
    /// list (C# `InvalidDataException`).
    InvalidResponse,
    /// The response timeout passed (C# `TimeoutException`).
    Timeout,
    /// The server answered with this HTTP status, or (`None`) could not be reached (C#
    /// `HttpRequestException`).
    Http(Option<u16>),
    /// The call was cancelled (Stop, a manual command, a goal change...).
    Cancelled,
    /// The profile store or the vault failed, with its message.
    Storage(String),
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentError::Invalid(message) => f.write_str(message),
            AgentError::InvalidResponse => f.write_str("Agent provider returned an invalid or incomplete response."),
            AgentError::Timeout => f.write_str("Agent provider request timed out."),
            AgentError::Http(Some(status)) => write!(f, "Agent provider returned HTTP {status}."),
            AgentError::Http(None) => f.write_str("Cannot connect to the agent provider."),
            AgentError::Cancelled => f.write_str("The agent request was cancelled."),
            AgentError::Storage(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for AgentError {}

/// A cancellation flag shared with a call running on another thread. Cancelling never waits:
/// a call that does not look at the flag finishes on its own and its result is dropped.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// Whether two tokens are the same flag.
    pub fn same(&self, other: &CancelToken) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Length in UTF-16 code units: every C# limit counts `string.Length`.
pub fn len16(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}
