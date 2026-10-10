//! Searching, filtering and sorting the directory, as the C# `WorldCatalog.Search` and
//! `WorldBrowserViewModel` do. The searchable text of every world is normalized once when the
//! catalog is built ([`SearchIndex`]), so a query only compares prepared strings.

use super::listing::WorldListing;
use crate::l10n::{S, t, tf};

/// Lower case, every run of characters that are not letters or digits turned into one space.
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = true;
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// One world's searchable text, normalized.
#[derive(Clone, Debug, Default)]
pub struct SearchIndex {
    name: String,
    words: Vec<String>,
    host: String,
    tags: String,
    details: String,
}

impl SearchIndex {
    pub fn new(w: &WorldListing) -> Self {
        let f = &w.features;
        let mut tags = String::new();
        for part in [
            &f.theme,
            &f.kind,
            &f.language,
            &f.location,
            &f.codebase,
            &f.roleplaying,
            &f.player_killing,
            &f.world_size,
            &f.development_status,
            &w.population.reported_range,
        ]
        .into_iter()
        .chain(w.tags.iter())
        {
            tags.push_str(part);
            tags.push(' ');
        }
        let port = |p: Option<u16>| p.map(|p| p.to_string()).unwrap_or_default();
        let name = normalize(&w.name);
        Self {
            words: name.split(' ').filter(|s| !s.is_empty()).map(str::to_string).collect(),
            name,
            host: normalize(&format!("{} {} {}", w.host, port(w.port), port(w.tls_port))),
            tags: normalize(&tags),
            details: normalize(&format!("{} {}", w.summary, w.description)),
        }
    }

    /// The C# relevance score: every term must match somewhere; 0 means no match.
    pub fn score(&self, terms: &[String]) -> u32 {
        let mut score = 0;
        for term in terms {
            score += if self.name == *term {
                100
            } else if self.name.contains(term.as_str()) {
                60
            } else if self.host.contains(term.as_str()) {
                50
            } else if self.tags.contains(term.as_str()) {
                35
            } else if self.details.contains(term.as_str()) {
                10
            } else if term.chars().count() >= 4 && {
                let limit = if term.chars().count() >= 7 { 2 } else { 1 };
                self.words.iter().any(|w| distance(term, w) <= limit)
            } {
                20
            } else {
                return 0;
            };
        }
        score
    }
}

/// Edit distance, giving up (3) when the lengths differ by more than two.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > 2 {
        return 3;
    }
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut diagonal = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let old = row[j];
            row[j] = (row[j] + 1)
                .min(row[j - 1] + 1)
                .min(diagonal + usize::from(a[i - 1] != b[j - 1]));
            diagonal = old;
        }
    }
    row[b.len()]
}

/// Sort orders, in the C# client's order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Sort {
    /// The directory's own rank with nothing typed, relevance with a search.
    #[default]
    BestMatch,
    Name,
    /// Live Wandur counts first, then other counts as history.
    Players,
    Rating,
    RecentlyUpdated,
    Newest,
}

impl Sort {
    pub const ALL: [Sort; 6] = [
        Sort::BestMatch,
        Sort::Name,
        Sort::Players,
        Sort::Rating,
        Sort::RecentlyUpdated,
        Sort::Newest,
    ];

    /// The sort's name in the current language.
    pub fn label(self) -> &'static str {
        t(match self {
            Sort::BestMatch => S::BestMatch,
            Sort::Name => S::NameAZ,
            Sort::Players => S::LastObservedPlayers,
            Sort::Rating => S::HighestRated,
            Sort::RecentlyUpdated => S::RecentlyUpdated,
            Sort::Newest => S::NewestWorlds,
        })
    }
}

/// Which worlds by how they are played.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Connection {
    #[default]
    All,
    /// Worlds a MUD client can connect to.
    MudOnly,
    /// Worlds played in a browser.
    WebOnly,
}

/// A filter on one listed feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Facet {
    Genre,
    Kind,
    Language,
    Roleplaying,
    PlayerKilling,
    Codebase,
    Development,
    WorldSize,
    Location,
    Tag,
}

impl Facet {
    pub const ALL: [Facet; 10] = [
        Facet::Genre,
        Facet::Kind,
        Facet::Language,
        Facet::Roleplaying,
        Facet::PlayerKilling,
        Facet::Codebase,
        Facet::Development,
        Facet::WorldSize,
        Facet::Location,
        Facet::Tag,
    ];

    /// The filter's name in the current language.
    pub fn label(self) -> &'static str {
        t(match self {
            Facet::Genre => S::Genre,
            Facet::Kind => S::GameType,
            Facet::Language => S::Language,
            Facet::Roleplaying => S::Roleplaying,
            Facet::PlayerKilling => S::PlayerKilling,
            Facet::Codebase => S::Codebase,
            Facet::Development => S::DevelopmentStage,
            Facet::WorldSize => S::WorldSizeRooms,
            Facet::Location => S::ServerLocation,
            Facet::Tag => S::FeatureTag,
        })
    }

    /// The world's values for this facet.
    pub fn values(self, w: &WorldListing) -> &[String] {
        let f = &w.features;
        std::slice::from_ref(match self {
            Facet::Genre => &f.theme,
            Facet::Kind => &f.kind,
            Facet::Language => &f.language,
            Facet::Roleplaying => &f.roleplaying,
            Facet::PlayerKilling => &f.player_killing,
            Facet::Codebase => &f.codebase,
            Facet::Development => &f.development_status,
            Facet::WorldSize => &f.world_size,
            Facet::Location => &f.location,
            Facet::Tag => return &w.tags,
        })
    }
}

/// What the person asked of the directory.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub search: String,
    pub connection: Connection,
    pub online_only: bool,
    pub sort: Sort,
    /// Selected facet values (one per facet), matched without case.
    pub facets: Vec<(Facet, String)>,
    pub min_players: Option<i64>,
    pub max_players: Option<i64>,
    /// 0 any, 1 three stars, 2 four, 3 four and a half.
    pub rating: u8,
    pub tls_only: bool,
    pub show_adult: bool,
}

impl Query {
    pub fn facet(&self, facet: Facet) -> Option<&str> {
        self.facets.iter().find(|(f, _)| *f == facet).map(|(_, v)| v.as_str())
    }

    pub fn set_facet(&mut self, facet: Facet, value: Option<String>) {
        self.facets.retain(|(f, _)| *f != facet);
        if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
            self.facets.push((facet, v));
        }
    }

    /// The number of further filters set (for the Filters button).
    pub fn advanced_count(&self) -> usize {
        self.facets.len()
            + usize::from(self.min_players.is_some())
            + usize::from(self.max_players.is_some())
            + usize::from(self.rating > 0)
            + usize::from(self.tls_only)
            + usize::from(self.show_adult)
            + usize::from(self.connection != Connection::All)
    }

    pub fn has_filters(&self) -> bool {
        *self != Query::default()
    }

    fn matches(&self, w: &WorldListing) -> bool {
        match self.connection {
            Connection::MudOnly if !w.can_connect() => return false,
            Connection::WebOnly if !w.web_only => return false,
            _ => {}
        }
        if self.online_only && !w.is_online() {
            return false;
        }
        for (facet, selected) in &self.facets {
            if !facet
                .values(w)
                .iter()
                .any(|v| v.trim().eq_ignore_ascii_case(selected.trim()))
            {
                return false;
            }
        }
        if self.tls_only && (!w.can_connect() || w.tls_port.is_none()) {
            return false;
        }
        if let Some(min) = self.min_players
            && w.population.latest_count.is_none_or(|c| c < min)
        {
            return false;
        }
        if let Some(max) = self.max_players
            && w.population.latest_count.is_none_or(|c| c > max)
        {
            return false;
        }
        let minimum = match self.rating {
            1 => 3.0,
            2 => 4.0,
            3 => 4.5,
            _ => return true,
        };
        w.community.rating_count.is_some_and(|n| n > 0) && w.community.rating.is_some_and(|r| r >= minimum)
    }
}

/// Run a query: the indices of matching worlds in display order. `browsable` says whether a world
/// may be listed at all (adult worlds only when asked for, or when saved).
pub fn run(
    worlds: &[WorldListing],
    index: &[SearchIndex],
    query: &Query,
    now: i64,
    browsable: impl Fn(&WorldListing) -> bool,
) -> Vec<usize> {
    let terms: Vec<String> = normalize(&query.search)
        .split(' ')
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    let keep = |i: usize| {
        let w = &worlds[i];
        browsable(w) && query.matches(w)
    };
    let mut results: Vec<usize> = if terms.is_empty() {
        (0..worlds.len()).filter(|&i| keep(i)).collect()
    } else {
        let mut scored: Vec<(u32, usize)> = (0..worlds.len())
            .filter_map(|i| {
                let s = index[i].score(&terms);
                (s > 0 && keep(i)).then_some((s, i))
            })
            .collect();
        // Score, then name (the worlds are in name order already, so the index breaks ties).
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().map(|(_, i)| i).collect()
    };
    let name = |i: &usize| worlds[*i].name.to_lowercase();
    match query.sort {
        Sort::BestMatch => {
            if terms.is_empty() {
                // The directory's rank; worlds without one keep their order after the ranked ones.
                results.sort_by_key(|&i| worlds[i].community.rank.map_or((1, 0), |r| (0, r)));
            }
        }
        Sort::Name => results.sort_by_cached_key(name),
        Sort::Players => results.sort_by_cached_key(|&i| {
            let w = &worlds[i];
            let live = w.live_players(now);
            (
                std::cmp::Reverse(live.is_some()),
                std::cmp::Reverse(live.or(w.population.latest_count).unwrap_or(i64::MIN)),
                w.name.to_lowercase(),
            )
        }),
        Sort::Rating => results.sort_by(|&a, &b| {
            let rating = |i: usize| {
                let c = &worlds[i].community;
                c.rating.filter(|_| c.rating_count.is_some_and(|n| n > 0))
            };
            let count = |i: usize| worlds[i].community.rating_count;
            rating(b)
                .partial_cmp(&rating(a))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(count(b).cmp(&count(a)))
                .then_with(|| name(&a).cmp(&name(&b)))
        }),
        Sort::RecentlyUpdated => results.sort_by_cached_key(|&i| {
            (
                std::cmp::Reverse(worlds[i].source.updated_at),
                worlds[i].name.to_lowercase(),
            )
        }),
        Sort::Newest => results.sort_by_cached_key(|&i| {
            (
                std::cmp::Reverse(worlds[i].established_at),
                worlds[i].name.to_lowercase(),
            )
        }),
    }
    results
}

/// The distinct values of a facet among `worlds` (without case), sorted, for the filter menus.
pub fn facet_options<'a>(facet: Facet, worlds: impl Iterator<Item = &'a WorldListing>) -> Vec<String> {
    let mut seen = std::collections::BTreeMap::new();
    for w in worlds {
        for v in facet.values(w) {
            let v = v.trim();
            if !v.is_empty() {
                seen.entry(v.to_lowercase()).or_insert_with(|| v.to_string());
            }
        }
    }
    seen.into_values().collect()
}

/// "300 worlds to explore", or "12 of 300 worlds to explore" when filtered.
pub fn count_text(shown: usize, listed: usize) -> String {
    if shown != listed {
        tf(S::WorldsToExploreFiltered, &[&shown, &listed])
    } else if shown == 1 {
        tf(S::WorldsToExploreOne, &[&shown])
    } else {
        tf(S::WorldsToExplore, &[&shown])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory::{WorldAvailability, WorldCommunity, WorldFeatures, WorldPopulation};

    fn world(name: &str) -> WorldListing {
        WorldListing {
            id: name.to_lowercase(),
            name: name.into(),
            host: format!("{}.example.org", name.to_lowercase().replace(' ', "")),
            port: Some(4000),
            ..Default::default()
        }
    }

    fn sample() -> Vec<WorldListing> {
        let mut a = world("Aardwolf");
        a.features = WorldFeatures {
            theme: "Fantasy".into(),
            language: "English".into(),
            codebase: "ROM".into(),
            ..Default::default()
        };
        a.availability = WorldAvailability {
            online: Some(true),
            ..Default::default()
        };
        a.community = WorldCommunity {
            rank: Some(2),
            rating: Some(4.6),
            rating_count: Some(20),
            ..Default::default()
        };
        a.population = WorldPopulation {
            latest_count: Some(200),
            observed_at: Some(1000),
            source: Some("wandur".into()),
            ..Default::default()
        };
        a.source.updated_at = Some(50);
        let mut b = world("Batmud");
        b.features.theme = "Fantasy".into();
        b.features.language = "Finnish".into();
        b.community.rank = Some(1);
        b.population.latest_count = Some(300);
        b.tls_port = Some(4001);
        b.established_at = Some(100);
        b.summary = "Huge world with dragons".into();
        let mut c = world("Cyberpunk Nights");
        c.features.theme = "Science Fiction".into();
        c.availability.online = Some(true);
        c.availability.archived = Some(true);
        c.web_only = true;
        c.adult_content = Some(true);
        c.tags = vec!["Cyberpunk".into()];
        c.source.updated_at = Some(900);
        let mut d = world("Dragon Realms");
        d.features.theme = "fantasy".into();
        d.community.rating = Some(3.5);
        d.community.rating_count = Some(4);
        vec![a, b, c, d]
    }

    fn names(worlds: &[WorldListing], q: &Query) -> Vec<String> {
        let index: Vec<SearchIndex> = worlds.iter().map(SearchIndex::new).collect();
        run(worlds, &index, q, 1000, |w| q.show_adult || !w.is_adult())
            .into_iter()
            .map(|i| worlds[i].name.clone())
            .collect()
    }

    #[test]
    fn search_scores_names_hosts_tags_and_near_misses() {
        let w = sample();
        let q = |s: &str| Query {
            search: s.into(),
            show_adult: true,
            ..Query::default()
        };
        assert_eq!(
            names(&w, &q("dragon")),
            ["Dragon Realms", "Batmud"],
            "name beats details"
        );
        assert_eq!(names(&w, &q("cyberpunk")), ["Cyberpunk Nights"]);
        assert_eq!(names(&w, &q("finnish")), ["Batmud"], "tags and features");
        assert_eq!(names(&w, &q("batmud.example")), ["Batmud"], "host");
        assert_eq!(names(&w, &q("aardwlf")), ["Aardwolf"], "one edit away");
        assert!(names(&w, &q("zzz")).is_empty());
        assert_eq!(
            names(&w, &q("fantasy dragon")),
            ["Dragon Realms", "Batmud"],
            "all terms must match"
        );
    }

    #[test]
    fn filters_combine() {
        let w = sample();
        let mut q = Query::default();
        assert_eq!(names(&w, &q).len(), 3, "adult worlds are hidden by default");
        q.show_adult = true;
        assert_eq!(names(&w, &q).len(), 4);
        q.online_only = true;
        assert_eq!(names(&w, &q), ["Aardwolf"], "archived worlds are not online");
        q.online_only = false;
        q.set_facet(Facet::Genre, Some("FANTASY".into()));
        assert_eq!(
            names(&w, &q),
            ["Batmud", "Aardwolf", "Dragon Realms"],
            "rank first, then unranked"
        );
        q.set_facet(Facet::Language, Some("English".into()));
        assert_eq!(names(&w, &q), ["Aardwolf"]);
        q.facets.clear();
        q.tls_only = true;
        assert_eq!(names(&w, &q), ["Batmud"]);
        q.tls_only = false;
        q.min_players = Some(250);
        assert_eq!(names(&w, &q), ["Batmud"], "unknown counts never match a number");
        q.min_players = None;
        q.rating = 2;
        assert_eq!(names(&w, &q), ["Aardwolf"]);
        q.rating = 0;
        q.connection = Connection::WebOnly;
        assert_eq!(names(&w, &q), ["Cyberpunk Nights"]);
        assert_eq!(q.advanced_count(), 2);
    }

    #[test]
    fn sorts_follow_the_csharp_client() {
        let w = sample();
        let q = |sort| Query {
            sort,
            show_adult: true,
            ..Query::default()
        };
        assert_eq!(names(&w, &q(Sort::Name))[0], "Aardwolf");
        assert_eq!(
            names(&w, &q(Sort::Players)),
            ["Aardwolf", "Batmud", "Cyberpunk Nights", "Dragon Realms"],
            "a live count beats a larger historic one"
        );
        assert_eq!(names(&w, &q(Sort::Rating))[..2], ["Aardwolf", "Dragon Realms"]);
        assert_eq!(
            names(&w, &q(Sort::RecentlyUpdated))[..2],
            ["Cyberpunk Nights", "Aardwolf"]
        );
        assert_eq!(names(&w, &q(Sort::Newest))[0], "Batmud");
    }

    #[test]
    fn facet_options_are_distinct_without_case() {
        let w = sample();
        assert_eq!(facet_options(Facet::Genre, w.iter()), ["Fantasy", "Science Fiction"]);
        assert_eq!(count_text(3, 3), "3 worlds to explore");
        assert_eq!(count_text(1, 1), "1 world to explore");
        assert_eq!(count_text(2, 3), "2 of 3 worlds to explore");
    }

    #[test]
    fn directory_text_follows_the_language() {
        use crate::l10n::{Language, S, override_thread, text_in};
        override_thread(Some(Language::De));
        let mut w = sample().remove(0);
        let counts = [count_text(3, 3), count_text(1, 1), count_text(2, 3)];
        w.web_only = true;
        let web = w.address();
        w.web_only = false;
        w.host.clear();
        let unlisted = w.address();
        let format = crate::directory::snapshot::parse(r#"{"format":"other"}"#).unwrap_err();
        let unreadable = crate::directory::snapshot::parse("not json").unwrap_err();
        override_thread(None);
        assert_eq!(
            counts,
            [
                "3 Welten zu entdecken",
                "1 Welt zu entdecken",
                "2 von 3 Welten zu entdecken"
            ]
        );
        assert_eq!(web, text_in(Language::De, S::BrowserBasedWorld));
        assert_eq!(unlisted, text_in(Language::De, S::ConnectionNotListed));
        assert_eq!(format, text_in(Language::De, S::DirectoryFormatUnsupported));
        assert!(
            unreadable.starts_with("Das Verzeichnis konnte nicht gelesen werden: "),
            "{unreadable}"
        );
    }

    #[test]
    fn normalize_collapses_punctuation() {
        assert_eq!(normalize("  Lantern & Forest!! "), "lantern forest");
        assert_eq!(normalize("ÄBC-déf"), "äbc déf");
    }
}
