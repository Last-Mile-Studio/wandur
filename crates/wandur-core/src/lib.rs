//! Core of the Wandur MUD client: everything that works without a window or a terminal grid.
//! The protocol pieces are pure over bytes (no I/O, no clock); the session drives them over a
//! [`transport::Transport`].
//!
//! - [`endpoint`]: parse what a person types as an address (`tls://` for TLS).
//! - [`telnet`]: IAC handling and option negotiation (ECHO, SGA, EOR, NAWS, TTYPE/MTTS, GMCP, MSSP).
//! - [`protocol`]: GMCP, MSDP and MSSP decoding, diagnostics formatting, protocol mappings and
//!   the game state they give, the latest-value cache.
//! - [`diagnostics`]: the Diagnostics document's Messages history and console.
//! - [`utf8`] and [`charset`]: incremental decoding (UTF-8 or Latin-1) and command encoding.
//! - [`prompt`]: prompt detection from GA/EOR marks or a quiet unterminated line.
//! - [`transport`]: TCP and TLS links behind one trait.
//! - [`session`]: a connection with its own threads and a bounded inbox.
//! - [`connection`]: the lifecycle across sessions, manual and automatic reconnect.
//! - [`settings`]: settings and saved worlds in the data directory.
//! - [`db`]: the client database (`wandur.db`): world identity, usage, one writer thread.
//! - [`directory`]: the world directory (fetch, offline cache, search, filters, sorting).
//! - [`channels`]: channel traffic (GMCP channels, rule sets per codebase family, rules taught
//!   per world) mirrored per channel, bounded, with replies.
//! - [`map`]: the room map: tracking, saved maps, routes, walking, editing, map files.
//! - [`classify`]: room terrain inference (a local classifier, behind the `classifier` feature).
//! - [`mudcolor`]: SMAUG colour codes and SGR in panel text, as styled runs.
//! - [`agent`]: local model agents (per-world profiles, providers, the decision runner), behind
//!   the `agent` feature.
//! - [`login`]: saved passwords in the system vault, auto-login over text prompts and GMCP.
//! - [`macros`]: no-code triggers, aliases, timers and function-key shortcuts.
//! - [`scripting`]: world scripts in JavaScript (QuickJS), one script thread per session.
//! - [`history`]: session history (recorder, `wandur.db` tables, full-text search).
//! - [`import`]: bringing another client's data over (the C# Wandur client's `wandur.db`).
//! - [`completion`]: inline completion of commands from words seen and sent.
//! - [`l10n`]: UI text in five languages, generated from the `.resx` tables.
//! - [`demo`]: the offline five-room demo world.
//! - [`weblinks`] and [`site`]: which links may be opened, and the wandur.net pages.
//! - [`updates`]: the daily update check against the directory's `/client/latest`.
//!
//! ANSI escape sequences are interpreted by the terminal grid (`wandur-term`), the one model of
//! a session's text.

#[cfg(feature = "agent")]
pub mod agent;
pub mod channels;
pub mod charset;
pub mod classify;
pub mod client_commands;
pub mod command_line;
pub mod completion;
pub mod connection;
pub mod db;
pub mod demo;
pub mod diagnostics;
pub mod directory;
pub mod endpoint;
pub mod history;
pub mod import;
pub mod l10n;
pub mod login;
pub mod macros;
pub mod map;
pub mod mudcolor;
pub mod mudlet;
pub mod prompt;
pub mod protocol;
pub mod scripting;
pub mod session;
pub mod settings;
pub mod site;
pub mod telnet;
pub mod transport;
pub mod updates;
pub mod utf8;
pub mod weblinks;

pub use charset::Charset;
pub use connection::{Connection, ConnectionState, Notice};
pub use endpoint::Endpoint;
pub use session::{Drained, Session, SessionConfig, SessionEvent, Waker};
