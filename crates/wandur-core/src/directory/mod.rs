//! The world directory (wandur.net's `GET /directory`): the listing model, snapshot parsing,
//! search, filters and sorting, an HTTP client, and a background service that caches the snapshot
//! on disk for offline use. Nothing here touches the UI.

pub mod catalog;
pub mod client;
pub mod install;
pub mod listing;
pub mod query;
pub mod service;
pub mod snapshot;
pub mod time;
pub mod world_theme;

pub use catalog::Catalog;
pub use listing::{
    Artwork, WorldAvailability, WorldCommunity, WorldFeatures, WorldListing, WorldPopulation, WorldSource,
};
pub use query::{Connection, Facet, Query, Sort};
pub use service::{DirectoryService, DirectoryStatus};
pub use world_theme::WorldTheme;
