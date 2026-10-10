//! Mudlet compatibility: the pattern conversions the Lua layer and the importer share, and
//! (with the `mudlet-import` feature) File > Import from Mudlet, ported from the C# client's
//! `feature/mudlet-import` branch:
//!
//! - [`source`] finds and reads a profile folder, a profile or package `.xml`, or an
//!   `.mpackage` ([`zip`]), and [`parser`] reads the untrusted MudletPackage XML into [`model`].
//! - [`converter`] maps it onto a world's library: items that only send commands
//!   ([`plain_send`]) become JavaScript, one script per top-level folder, F1 to F12 keys become
//!   key macros, and everything else is kept, switched off, as needing conversion.
//! - [`importer`] picks or creates the world and merges the scripts into its library, keeping
//!   edits and on and off choices; [`summary`] says what happened.
//! - [`lua_wrapper`] turns kept items into a Mudlet-compatible Lua script (Run as Lua).

pub mod regex;

#[cfg(feature = "mudlet-import")]
pub mod converter;
#[cfg(feature = "mudlet-import")]
pub mod importer;
#[cfg(feature = "mudlet-import")]
pub mod lua_wrapper;
#[cfg(feature = "mudlet-import")]
pub mod model;
#[cfg(feature = "mudlet-import")]
pub mod parser;
#[cfg(feature = "mudlet-import")]
pub mod plain_send;
#[cfg(feature = "mudlet-import")]
pub mod source;
#[cfg(feature = "mudlet-import")]
pub mod summary;
#[cfg(any(feature = "mudlet-import", feature = "classifier"))]
pub mod zip;

#[cfg(all(test, feature = "mudlet-import"))]
mod tests;
