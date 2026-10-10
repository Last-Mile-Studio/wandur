//! Logging in to a world: the saved password in the operating system's vault, the text
//! handshake that answers the username and password prompts once, and GMCP `Char.Login`.
//! No UI and no threads here; the app decides when to read the vault and what to send.
//!
//! - [`vault`]: where passwords live, the vault key, password rules.
//! - [`credentials`]: saving and forgetting a world's password in the C# order.
//! - [`sequence`]: the two-minute text handshake and the prompt line it reads.
//! - [`gmcp`]: `Char.Login` offers, results and the credentials message.

pub mod credentials;
pub mod gmcp;
pub mod sequence;
pub mod vault;

pub use sequence::{AutoLoginSequence, DEFAULT_PASSWORD_PROMPT, DEFAULT_USERNAME_PROMPT, LoginStep, PromptLine};
pub use vault::{MemoryVault, PasswordVault, VaultError};

/// How long a GMCP login may wait for `Char.Login.Result` before automation stops.
pub const GMCP_RESULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
