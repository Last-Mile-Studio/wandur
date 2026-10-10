//! One world in the directory, in the wire format of the wandur.net API (`GET /directory`,
//! `format: "wandur.directory"`, `schema_version: 2`, snake_case fields). The shape follows the
//! C# client's `WorldListing`; fields this prototype does not use yet (world themes) are ignored.
//! A script in the world's pack (`scripts`) that does not have the expected shape is dropped. A protocol mapping that does not validate is dropped and the listing kept. JSON `null` is accepted wherever the C# client accepts it: strings
//! become empty, numbers and flags stay unknown.

use crate::l10n::{S, t, tf};
use serde::{Deserialize, Deserializer};

use super::time::parse_rfc3339;

fn null_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

fn null_strings<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let items = Option::<Vec<Option<String>>>::deserialize(d)?.unwrap_or_default();
    Ok(items.into_iter().flatten().collect())
}

/// A timestamp field: kept as seconds since 1970; unparseable or null values are unknown.
fn timestamp<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(Option::<String>::deserialize(d)?.as_deref().and_then(parse_rfc3339))
}

fn null_default<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(d: D) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A port as sent: kept only when it is between 1 and 65535.
fn port<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u16>, D::Error> {
    let value = Option::<f64>::deserialize(d)?;
    Ok(value
        .filter(|v| v.fract() == 0.0 && *v >= 1.0 && *v <= 65_535.0)
        .map(|v| v as u16))
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct WorldListing {
    #[serde(deserialize_with = "null_string")]
    pub id: String,
    #[serde(deserialize_with = "null_string")]
    pub name: String,
    #[serde(deserialize_with = "null_string")]
    pub summary: String,
    #[serde(deserialize_with = "null_string")]
    pub description: String,
    #[serde(deserialize_with = "null_string")]
    pub host: String,
    #[serde(deserialize_with = "port")]
    pub port: Option<u16>,
    #[serde(deserialize_with = "port")]
    pub tls_port: Option<u16>,
    #[serde(deserialize_with = "null_default")]
    pub web_only: bool,
    pub beginner_friendly: Option<bool>,
    pub adult_content: Option<bool>,
    pub banner_by_owner: Option<bool>,
    #[serde(deserialize_with = "null_default")]
    pub source: WorldSource,
    #[serde(deserialize_with = "null_default")]
    pub availability: WorldAvailability,
    #[serde(deserialize_with = "null_default")]
    pub population: WorldPopulation,
    #[serde(deserialize_with = "null_default")]
    pub features: WorldFeatures,
    #[serde(deserialize_with = "null_default")]
    pub community: WorldCommunity,
    #[serde(deserialize_with = "timestamp")]
    pub established_at: Option<i64>,
    #[serde(deserialize_with = "null_strings")]
    pub tags: Vec<String>,
    #[serde(deserialize_with = "null_string")]
    pub website_url: String,
    #[serde(deserialize_with = "null_string")]
    pub discord_url: String,
    #[serde(deserialize_with = "null_string")]
    pub play_url: String,
    #[serde(deserialize_with = "null_string")]
    pub banner_url: String,
    /// Relative to the directory's address.
    #[serde(deserialize_with = "null_string")]
    pub generated_artwork_path: String,
    /// The world's protocol mapping (vitals and the opponent card), when the directory has a valid
    /// one. It applies only to the endpoint it names; see [`WorldListing::mapping_for`].
    #[serde(deserialize_with = "crate::protocol::mapping::lenient")]
    pub protocol_mapping: Option<crate::protocol::mapping::WorldMapping>,
    /// The world's own appearance, when the directory lists a valid one (see
    /// [`crate::directory::world_theme`]).
    #[serde(deserialize_with = "crate::directory::world_theme::lenient")]
    pub theme: Option<crate::directory::WorldTheme>,
    /// The world's script pack, as listed (see [`WorldListing::supported_scripts`]).
    #[serde(deserialize_with = "lenient_scripts")]
    pub scripts: Vec<WorldScriptListing>,
}

/// One script a directory supplies for a world (the C# `WorldScriptListing`). The directory
/// client never runs it; the session does, under the pack send policy.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct WorldScriptListing {
    #[serde(deserialize_with = "null_string")]
    pub id: String,
    #[serde(deserialize_with = "null_string")]
    pub name: String,
    #[serde(deserialize_with = "null_string")]
    pub description: String,
    #[serde(deserialize_with = "null_string")]
    pub source: String,
    /// `generated` or `reviewed`; other provenances are not attached.
    #[serde(deserialize_with = "null_string")]
    pub provenance: String,
    pub version: i64,
}

/// The provenances this client attaches (the C# `ScriptPackInfo.IsSupported`).
pub fn is_supported_provenance(provenance: &str) -> bool {
    matches!(provenance, "generated" | "reviewed")
}

impl WorldScriptListing {
    /// The C# `IsValid`: an id and a name of 1 to 120 characters without control characters, a
    /// description of at most 1,024 (tabs and new lines allowed), a source within 256 KiB, a
    /// supported provenance and a version of 0 or more that fits the C# int.
    pub fn is_valid(&self) -> bool {
        let units = |s: &str| s.encode_utf16().count();
        let name = self.name.trim();
        (1..=120).contains(&units(&self.id))
            && !self.id.chars().any(char::is_control)
            && (1..=120).contains(&units(name))
            && !self.name.chars().any(char::is_control)
            && units(&self.description) <= 1024
            && !self
                .description
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t')
            && !self.source.is_empty()
            && self.source.len() <= crate::db::scripts::MAX_SOURCE_BYTES
            && is_supported_provenance(&self.provenance)
            && (0..=i64::from(i32::MAX)).contains(&self.version)
    }
}

/// The pack's scripts one by one: an entry of the wrong shape is dropped, not the listing.
fn lenient_scripts<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<WorldScriptListing>, D::Error> {
    let items = Option::<Vec<serde_json::Value>>::deserialize(d)?.unwrap_or_default();
    Ok(items
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect())
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct WorldSource {
    #[serde(deserialize_with = "null_string")]
    pub provider: String,
    #[serde(deserialize_with = "null_string")]
    pub name: String,
    #[serde(deserialize_with = "null_string")]
    pub record_id: String,
    #[serde(deserialize_with = "null_string")]
    pub listing_url: String,
    #[serde(deserialize_with = "timestamp")]
    pub updated_at: Option<i64>,
    #[serde(deserialize_with = "timestamp")]
    pub listed_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct WorldAvailability {
    pub online: Option<bool>,
    pub archived: Option<bool>,
    #[serde(deserialize_with = "null_string")]
    pub archive_reason: String,
    #[serde(deserialize_with = "timestamp")]
    pub checked_at: Option<i64>,
    #[serde(deserialize_with = "timestamp")]
    pub last_online_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct WorldPopulation {
    pub latest_count: Option<i64>,
    #[serde(deserialize_with = "timestamp")]
    pub observed_at: Option<i64>,
    pub average_count: Option<f64>,
    #[serde(deserialize_with = "null_string")]
    pub reported_range: String,
    /// Who counted: "wandur" for the directory's own probe.
    pub source: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct WorldFeatures {
    /// The genre (fantasy, science fiction...).
    #[serde(deserialize_with = "null_string")]
    pub theme: String,
    #[serde(deserialize_with = "null_string")]
    pub kind: String,
    #[serde(deserialize_with = "null_string")]
    pub language: String,
    #[serde(deserialize_with = "null_string")]
    pub location: String,
    #[serde(deserialize_with = "null_string")]
    pub codebase: String,
    #[serde(deserialize_with = "null_string")]
    pub roleplaying: String,
    #[serde(deserialize_with = "null_string")]
    pub player_killing: String,
    #[serde(deserialize_with = "null_string")]
    pub world_size: String,
    #[serde(deserialize_with = "null_string")]
    pub development_status: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct WorldCommunity {
    pub rating: Option<f64>,
    pub rating_count: Option<i64>,
    pub review_count: Option<i64>,
    pub rank: Option<i64>,
    pub monthly_votes: Option<i64>,
}

/// Which picture of a world to fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Artwork {
    /// The illustration the directory generated from the listing.
    Generated,
    /// The banner the listing supplied.
    Supplied,
}

/// How old a Wandur player count may be and still be shown as live (the C# client's two hours).
pub const LIVE_WINDOW_SECS: i64 = 2 * 3600;

impl WorldListing {
    /// The listing's protocol mapping when it is for this exact address.
    pub fn mapping_for(&self, host: &str, port: u16, tls: bool) -> Option<&crate::protocol::mapping::WorldMapping> {
        self.protocol_mapping
            .as_ref()
            .filter(|m| m.endpoint.is(host, port, tls))
    }

    /// The supplied scripts this client is willing to attach, in listing order: valid, the
    /// first of each id, at most 64.
    pub fn supported_scripts(&self) -> Vec<WorldScriptListing> {
        let mut seen = std::collections::HashSet::new();
        self.scripts
            .iter()
            .filter(|s| s.is_valid() && seen.insert(s.id.clone()))
            .take(crate::db::scripts::MAX_ENTRIES)
            .cloned()
            .collect()
    }

    /// The listing can be connected to with a MUD client (not a browser-only world).
    pub fn can_connect(&self) -> bool {
        !self.web_only && valid_host(&self.host) && self.port.or(self.tls_port).is_some()
    }

    pub fn is_adult(&self) -> bool {
        self.adult_content == Some(true)
    }

    /// Online by the directory's latest report, and not archived.
    pub fn is_online(&self) -> bool {
        self.availability.online == Some(true) && self.availability.archived != Some(true)
    }

    /// A player count Wandur measured itself within two hours of `now` (a few minutes into the
    /// future is tolerated as clock skew).
    pub fn live_players(&self, now: i64) -> Option<i64> {
        let p = &self.population;
        let ours = p.source.as_deref().is_some_and(|s| s.eq_ignore_ascii_case("wandur"));
        match (p.latest_count, p.observed_at) {
            (Some(count), Some(at)) if ours && now - at <= LIVE_WINDOW_SECS && at - now <= 300 => Some(count),
            _ => None,
        }
    }

    /// "143 online" for a fresh Wandur count, "Online" for any other world reported online.
    pub fn online_text(&self, now: i64) -> Option<String> {
        if !self.is_online() {
            return None;
        }
        Some(match self.live_players(now) {
            Some(n) => tf(S::OnlineCount, &[&n]),
            None => t(S::StatusOnline).into(),
        })
    }

    /// The address people type: `host:port`, or why there is none.
    pub fn address(&self) -> String {
        if self.web_only {
            return t(S::BrowserBasedWorld).into();
        }
        if self.host.is_empty() {
            return t(S::ConnectionNotListed).into();
        }
        match self.port.or(self.tls_port) {
            Some(port) => format_host_port(&self.host, port),
            None => self.host.clone(),
        }
    }

    pub fn has_supplied_artwork(&self) -> bool {
        !self.banner_url.is_empty()
    }

    pub fn has_generated_artwork(&self) -> bool {
        !self.generated_artwork_path.trim().is_empty()
    }

    pub fn has_artwork(&self) -> bool {
        self.has_supplied_artwork() || self.has_generated_artwork()
    }

    /// The picture that stands for the world: an owner's banner, else the generated
    /// illustration, else whatever banner the listing supplied.
    pub fn preferred_artwork(&self) -> Option<Artwork> {
        if self.banner_by_owner == Some(true) && self.has_supplied_artwork() {
            Some(Artwork::Supplied)
        } else if self.has_generated_artwork() {
            Some(Artwork::Generated)
        } else if self.has_supplied_artwork() {
            Some(Artwork::Supplied)
        } else {
            None
        }
    }

    /// A cache key for the world's picture: what it was made from, so a changed description
    /// (and a regenerated picture) is fetched afresh.
    pub fn art_key(&self) -> String {
        let subject = match self.preferred_artwork() {
            Some(Artwork::Supplied) => format!("{}\nsupplied\n{}", self.id, self.banner_url),
            _ => format!("{}\n{}\n{}\n{}", self.id, self.name, self.summary, self.description),
        };
        format!("{:016x}", fnv1a(subject.as_bytes()))
    }

    /// The genre, then up to two other tags; and one further tag for the row's bottom pill
    /// unless the world is beginner friendly (the C# `DirectoryLook.RowPills`).
    pub fn row_pills(&self) -> (Vec<&str>, Option<&str>) {
        let genre = self.features.theme.trim();
        let mut tags: Vec<&str> = Vec::new();
        for tag in self.tags.iter().map(|t| t.trim()) {
            if !tag.is_empty() && !tag.eq_ignore_ascii_case(genre) && !tags.iter().any(|t| t.eq_ignore_ascii_case(tag))
            {
                tags.push(tag);
            }
        }
        let mut top = Vec::new();
        if !genre.is_empty() {
            top.push(genre);
        }
        top.extend(tags.iter().take(2));
        let bottom = if self.beginner_friendly == Some(true) {
            None
        } else {
            tags.get(2).copied()
        };
        (top, bottom)
    }

    /// The listing's text for a row: the summary, else the description, on one line.
    pub fn blurb(&self) -> String {
        let text = if self.summary.trim().is_empty() {
            &self.description
        } else {
            &self.summary
        };
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Who counted the players, for the history line.
    pub fn population_counted_by(&self) -> String {
        let source = self.population.source.as_deref().map(str::trim).unwrap_or("");
        match source.to_ascii_lowercase().as_str() {
            "wandur" => "Wandur".into(),
            "mudverse" => "MUDVerse".into(),
            "mudconnector" => "The Mud Connector".into(),
            "" if !self.source.name.is_empty() => self.source.name.clone(),
            "" => t(S::Directory).into(),
            _ if source.eq_ignore_ascii_case(&self.source.provider) && !self.source.name.is_empty() => {
                self.source.name.clone()
            }
            _ => source.to_string(),
        }
    }

    /// The last count as history: "MUDVerse counted 86, 3 days ago".
    pub fn population_history(&self, now: i64) -> Option<String> {
        let count = self.population.latest_count?;
        let who = self.population_counted_by();
        Some(match self.population.observed_at {
            Some(at) => tf(S::CountedAgo, &[&who, &count, &super::time::ago(at, now)]),
            None => tf(S::Counted, &[&who, &count]),
        })
    }

    /// The rating line for the details.
    pub fn rating_summary(&self) -> String {
        match (self.community.rating, self.community.rating_count) {
            (Some(r), Some(n)) if n > 0 => tf(
                if n == 1 { S::RatingOne } else { S::RatingMany },
                &[&format!("{r:.1}"), &n],
            ),
            (_, Some(0)) => t(S::NoRatingsYet).into(),
            _ => t(S::RatingNotSupplied).into(),
        }
    }
}

/// `host:port`, with brackets around an IPv6 address.
pub fn format_host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn valid_host(host: &str) -> bool {
    let host = host.trim();
    !host.is_empty()
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_'))
}

/// A small stable hash for cache keys (not for security).
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}
