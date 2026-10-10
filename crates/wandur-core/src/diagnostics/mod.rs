//! The session's Diagnostics document without its drawing: the Messages history with its kinds
//! and filter ([`messages`]), and the raw text console ([`console`]). Both are bounded and kept in
//! memory for one session only.

pub mod console;
pub mod messages;
pub mod session;

pub use session::{Gate, SessionProtocol};
