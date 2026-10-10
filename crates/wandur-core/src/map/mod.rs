//! The mapper: rooms and directed exits learned from GMCP, MSDP and room text, saved per world,
//! searched, routed and walked (the C# `Wandur.Core.Mapping` and the SDK's room decoder).
//!
//! - [`model`]: rooms, exits, areas, tombstones and observations, in the C# JSON shape.
//! - [`decode`]: `Room.Info` and MSDP room variables to observations.
//! - [`text`]: room blocks read from plain text, and failed moves.
//! - [`tracker`]: the room tracker and the map editing calls.
//! - [`editing`]: the map editor's operations (connect, move, delete, field edits), each one
//!   undo step over the tracker's editing calls.
//! - [`route`]: the route planner.
//! - [`search`]: room search by observed words.
//! - [`merge`] and [`store`]: saved maps in `wandur.db`, merged across sessions.
//! - [`format`]: the versioned map file and its checks.
//! - [`images`]: label pictures (hashing, checks, scaling down, base64).
//! - [`mudlet`]: Mudlet's JSON map export read into a map to merge (File > Import map).
//! - [`session`]: one session's map, its protocol evidence and verified walking.

pub mod decode;
pub mod editing;
pub mod format;
pub mod images;
pub mod merge;
pub mod model;
pub mod mudlet;
pub mod route;
pub mod search;
pub mod session;
pub mod store;
pub mod text;
pub mod tracker;

#[cfg(test)]
mod tests;

pub use editing::infer_direction;
pub use model::{
    DoorState, MapAreaSettings, MapImage, MapLabel, MapLabelDeletion, MapLink, MapRoom, MapSnapshot, RoomObservation,
    RoomSource, TrackingState, normalize_direction,
};
pub use route::{MapRoute, find_route, find_route_live};
pub use session::{MapSession, OptionState, ProtocolEvidence, WalkGate, WalkStatus};
pub use tracker::RoomMapTracker;
#[cfg(test)]
mod label_tests;
