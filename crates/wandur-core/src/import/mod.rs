//! Bringing another client's data over. [`csharp`] reads the C# Wandur client's data directory
//! (its `wandur.db`) strictly read-only and merges it into this client's data directory.

pub mod csharp;
