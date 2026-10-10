//! A parsed directory ready to query: the worlds in name order and their search index, shared
//! behind an `Arc` so the UI and workers read one copy.

use std::sync::Arc;

use super::listing::WorldListing;
use super::query::{self, Query, SearchIndex};
use super::snapshot::Snapshot;
use crate::settings::same_host;

#[derive(Debug, Default)]
pub struct Catalog {
    pub worlds: Vec<WorldListing>,
    index: Vec<SearchIndex>,
    /// When the directory built the snapshot (seconds since 1970).
    pub fetched_at: Option<i64>,
}

impl Catalog {
    pub fn new(snapshot: Snapshot) -> Self {
        let index = snapshot.worlds.iter().map(SearchIndex::new).collect();
        Self {
            worlds: snapshot.worlds,
            index,
            fetched_at: snapshot.fetched_at,
        }
    }

    pub fn len(&self) -> usize {
        self.worlds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.worlds.is_empty()
    }

    /// Matching worlds (indices into `worlds`) in display order.
    pub fn query(&self, query: &Query, now: i64, browsable: impl Fn(&WorldListing) -> bool) -> Vec<usize> {
        query::run(&self.worlds, &self.index, query, now, browsable)
    }

    pub fn by_id(&self, id: &str) -> Option<&WorldListing> {
        self.worlds.iter().find(|w| w.id == id)
    }

    /// The one listing at this address, if exactly one is (the C# `FindEndpoint`).
    pub fn find_endpoint(&self, host: &str, port: u16, tls: bool) -> Option<&WorldListing> {
        let mut matches = self.worlds.iter().filter(|w| {
            !w.web_only
                && same_host(&w.host, host)
                && if tls {
                    w.tls_port == Some(port)
                } else {
                    w.port == Some(port)
                }
        });
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }
}

pub type SharedCatalog = Arc<Catalog>;
