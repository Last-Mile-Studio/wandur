//! No-code macros: triggers, aliases, timers and function-key shortcuts, as the C# client's
//! Macros section defines them, run natively (no script engine) over a session's complete lines
//! and its send path.
//!
//! - [`definition`]: the saved data, validation and the C# JavaScript form.
//! - [`rules`]: compiled matching (one Aho-Corasick pass per line for every trigger).
//! - [`runtime`]: one session's macros: when they run, timers, the session switch, rate limits.
//! - [`worker`]: trigger matching on a thread of its own, so a flood costs the UI thread little.
//!
//! Macros are stored per world in `wandur.db` with the scripts ([`crate::db::scripts`]).

pub mod definition;
pub mod rules;
pub mod runtime;
pub mod worker;

pub use definition::{MacroDefinition, MacroError, MacroKind, MacroMatch, SHORTCUT_KEYS};
pub use rules::RuleSet;
pub use runtime::{Gate, MacroRuntime, MacroStatus, SavedMacro};
