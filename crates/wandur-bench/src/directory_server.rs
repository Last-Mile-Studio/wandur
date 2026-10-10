//! A loopback-only stand-in for the wandur.net directory, like the C# bench's `DirectoryServer`:
//! `GET /directory` lists generated worlds, and every world's art (any size asked for) is the same
//! large generated picture, JPEG for most worlds and PNG for every fifth, so thumbnail decoding sees
//! full-size sources. `--varied` gives the worlds different genres, languages, codebases, tags,
//! counts and ratings (for trying filters and for screenshots); without it every world copies one
//! template, as the C# harness does. `--fixture` serves the C# reference captures' four fictional
//! worlds instead (Starfall, Emberwild, The Last Harbor, Moss & Myth) with drawn landscapes, so
//! scenes can be compared with the C# screenshots.
//!
//! `GET /client/latest` answers what [`DirectoryServer::set_latest`] (or `--latest VERSION`)
//! set, 404 otherwise. Every request's head (request line and headers) is kept
//! ([`DirectoryServer::requests`]) and, with `--log FILE`, appended to a file, so tests and runs
//! can show what reached the server and which requests carried the install id.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use image::{Rgba, RgbaImage};

/// What the server answers besides `/directory`.
enum Content {
    /// Every world's art is one of two large generated pictures.
    Generated { jpeg: Vec<u8>, png: Vec<u8> },
    /// Fixed paths (without the leading slash) to a content type and body; anything else is 404.
    Fixture(std::collections::HashMap<String, (&'static str, Vec<u8>)>),
}

/// A fixed answer for one path: status line, content type, body and an optional `Location`.
#[derive(Clone)]
pub struct Route {
    pub status: &'static str,
    pub content_type: &'static str,
    pub body: Vec<u8>,
    pub location: Option<String>,
}

/// Fixed answers by path (without the leading slash), checked before anything else.
type Routes = Arc<RwLock<std::collections::HashMap<String, Route>>>;

/// The request heads received, oldest first, and an optional file they are appended to.
#[derive(Clone, Default)]
pub struct RequestLog {
    heads: Arc<Mutex<Vec<String>>>,
    file: Arc<Mutex<Option<std::path::PathBuf>>>,
}

impl RequestLog {
    fn push(&self, head: String) {
        let file = self.file.lock().ok().and_then(|f| f.clone());
        if let Some(path) = file
            && let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path)
        {
            let _ = writeln!(f, "{}", head.trim_end());
            let _ = writeln!(f);
        }
        if let Ok(mut heads) = self.heads.lock() {
            heads.push(head);
        }
    }
}

pub struct DirectoryServer {
    pub port: u16,
    stop: Arc<AtomicBool>,
    art_requests: Arc<AtomicU64>,
    /// What `GET /directory` answers; [`DirectoryServer::set_directory`] changes it.
    #[cfg_attr(not(test), allow(dead_code))]
    directory: Arc<RwLock<Arc<Vec<u8>>>>,
    routes: Routes,
    log: RequestLog,
}

/// What `/client/latest` answers for a release `version`, in the site's shape.
pub fn latest_json(version: &str) -> String {
    serde_json::json!({
        "version": version,
        "published_at": "2026-10-03T12:48:44Z",
        "page": "https://www.wandur.net/client/downloads",
        "notes": format!("https://github.com/Last-Mile-Studio/wandur/releases/tag/v{version}"),
        "files": [],
    })
    .to_string()
}

/// Large, original test artwork: a gradient, translucent circles and fine texture, so the codecs
/// have real work (the C# bench's `Artwork.Generate`, redrawn here).
pub fn generate_art(width: u32, height: u32, seed: u64) -> RgbaImage {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let stops = [(20.0, 30.0, 70.0), (140.0, 70.0, 160.0), (240.0, 180.0, 90.0)];
    let mut img = RgbaImage::from_fn(width, height, |x, y| {
        let t = (x as f32 / width as f32 + y as f32 / height as f32) / 2.0;
        let (a, b, u) = if t < 0.5 {
            (stops[0], stops[1], t * 2.0)
        } else {
            (stops[1], stops[2], (t - 0.5) * 2.0)
        };
        let mix = |p: f32, q: f32| (p + (q - p) * u) as u8;
        Rgba([mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2), 255])
    });
    let blend = |img: &mut RgbaImage, x: u32, y: u32, c: [u8; 3], alpha: u32| {
        let p = img.get_pixel_mut(x, y);
        for i in 0..3 {
            p[i] = ((u32::from(p[i]) * (255 - alpha) + u32::from(c[i]) * alpha) / 255) as u8;
        }
    };
    for _ in 0..400 {
        let c = [rng.next() as u8, rng.next() as u8, rng.next() as u8];
        let alpha = 40 + (rng.next() % 160) as u32;
        let (cx, cy) = (
            (rng.next() % u64::from(width)) as i64,
            (rng.next() % u64::from(height)) as i64,
        );
        let r = 10 + (rng.next() % u64::from((width / 10).max(11) - 10)) as i64;
        for y in (cy - r).max(0)..(cy + r).min(i64::from(height)) {
            let dy = y - cy;
            let half = ((r * r - dy * dy) as f64).sqrt() as i64;
            for x in (cx - half).max(0)..(cx + half).min(i64::from(width)) {
                blend(&mut img, x as u32, y as u32, c, alpha);
            }
        }
    }
    for _ in 0..(width * height / 400) {
        let c = [rng.next() as u8, rng.next() as u8, rng.next() as u8];
        let (x0, y0) = (
            (rng.next() % u64::from(width)) as u32,
            (rng.next() % u64::from(height)) as u32,
        );
        for y in y0..(y0 + 3).min(height) {
            for x in x0..(x0 + 3).min(width) {
                blend(&mut img, x, y, c, 60);
            }
        }
    }
    img
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// The JPEG and PNG pictures every world serves, made once and kept in `cache_dir`.
pub fn artwork(cache_dir: &Path, width: u32, height: u32) -> (Vec<u8>, Vec<u8>) {
    let _ = std::fs::create_dir_all(cache_dir);
    let jpg = cache_dir.join(format!("large-{width}x{height}.jpg"));
    let png = cache_dir.join(format!("large-{width}x{height}.png"));
    if let (Ok(j), Ok(p)) = (std::fs::read(&jpg), std::fs::read(&png)) {
        return (j, p);
    }
    let img = generate_art(width, height, 7);
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    let mut j = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut j, 88)
        .encode_image(&rgb)
        .expect("JPEG encoding");
    let mut p = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut p),
        rgb.as_raw(),
        width,
        height,
        image::ExtendedColorType::Rgb8,
    )
    .expect("PNG encoding");
    let _ = std::fs::write(&jpg, &j);
    let _ = std::fs::write(&png, &p);
    (j, p)
}

const GENRES: [&str; 6] = [
    "Fantasy",
    "Science Fiction",
    "Horror",
    "Post-apocalyptic",
    "Historical",
    "Superhero",
];
const LANGUAGES: [&str; 4] = ["English", "English", "German", "Portuguese"];
const CODEBASES: [&str; 6] = ["Custom", "ROM", "LPMud", "DikuMUD", "Evennia", "CoffeeMUD"];
const TAGS: [&str; 10] = [
    "Exploration",
    "PvP",
    "Quests",
    "Crafting",
    "Roleplay",
    "Clans",
    "Remort",
    "Pets",
    "Housing",
    "Puzzles",
];
const FIRST: [&str; 12] = [
    "Ashen",
    "Silver",
    "Hollow",
    "Crimson",
    "Northern",
    "Sunken",
    "Iron",
    "Whispering",
    "Gilded",
    "Shattered",
    "Amber",
    "Lantern",
];
const SECOND: [&str; 10] = [
    "Vale", "Reach", "Kingdoms", "Expanse", "Marches", "Spire", "Frontier", "Isles", "Depths", "Forest",
];

/// The directory JSON for `count` worlds.
pub fn listing_json(count: usize, varied: bool, now_rfc3339: &str) -> String {
    let mut worlds = Vec::with_capacity(count);
    for i in 0..count {
        let mut w = serde_json::json!({
            "id": format!("bench-world-{i}"),
            "name": format!("Bench World {i:03}"),
            "summary": format!("Generated directory entry {i} for the performance harness"),
            "description": "A wizard's forest.\n\nVisit & explore.\nStay awhile.",
            "host": format!("bench{i}.invalid"),
            "port": 4000 + i,
            "tls_port": null,
            "web_only": false,
            "established_at": null,
            "community": {"rating": null, "rating_count": null, "review_count": null, "rank": null, "monthly_votes": 123},
            "source": {"provider": "mudverse", "name": "MUDVerse", "record_id": "7",
                       "listing_url": "https://www.mudverse.com/game/7", "updated_at": "2026-09-14T20:00:00+00:00",
                       "listed_at": "2009-10-22T19:56:31+00:00"},
            "availability": {"online": true, "archived": false, "archive_reason": "",
                             "checked_at": "2026-09-15T20:00:00+00:00", "last_online_at": "2026-09-15T20:00:00+00:00"},
            "population": {"latest_count": 0, "observed_at": "2026-09-14T12:00:00+00:00", "average_count": null,
                           "reported_range": "75-100"},
            "features": {"theme": "Fantasy", "kind": "MUD", "language": "English", "location": "USA", "codebase": "Custom",
                         "roleplaying": "Suggested", "player_killing": "Restricted", "world_size": "10000+",
                         "development_status": "Beta"},
            "tags": ["Exploration"],
            "website_url": "",
            "discord_url": "https://discord.gg/example",
            "play_url": "https://example.org/play",
            "banner_url": "",
            "generated_artwork_path": format!("worlds/bench-world-{i}/art"),
        });
        if varied {
            let name = format!(
                "{} {}",
                FIRST[i % FIRST.len()],
                SECOND[(i / FIRST.len() + i) % SECOND.len()]
            );
            let genre = GENRES[i % GENRES.len()];
            let tags: Vec<&str> = (0..(1 + i % 4)).map(|k| TAGS[(i * 3 + k * 7) % TAGS.len()]).collect();
            let round = i / (FIRST.len() * SECOND.len());
            w["name"] = if round == 0 {
                name
            } else {
                format!("{name} {}", round + 1)
            }
            .into();
            w["summary"] = format!(
                "A {} world of {} where {}.",
                genre.to_lowercase(),
                [
                    "old roads and older secrets",
                    "drifting stations",
                    "haunted manors",
                    "ruined cities",
                    "great houses",
                    "masked heroes"
                ][i % 6],
                [
                    "every choice is remembered",
                    "clans rise and fall",
                    "the map is yours to draw",
                    "the night is long"
                ][i % 4]
            )
            .into();
            w["description"] = format!(
                "Founded by a small team of builders, this world has grown over many years.\n\nExpect {} and a \
                 welcoming community. New players start in a guided area with a mentor channel.",
                tags.join(", ").to_lowercase()
            )
            .into();
            w["features"]["theme"] = genre.into();
            w["features"]["language"] = LANGUAGES[i % LANGUAGES.len()].into();
            w["features"]["codebase"] = CODEBASES[i % CODEBASES.len()].into();
            w["features"]["roleplaying"] = ["Suggested", "Required", "Allowed"][i % 3].into();
            w["tags"] = serde_json::json!(tags);
            w["availability"]["online"] = (i % 7 != 3).into();
            w["beginner_friendly"] = (i % 5 == 0).into();
            w["adult_content"] = (i % 23 == 11).into();
            w["tls_port"] = if i % 3 == 0 {
                (5000 + i).into()
            } else {
                serde_json::Value::Null
            };
            w["community"]["rank"] = if i % 4 == 0 {
                (i / 4 + 1).into()
            } else {
                serde_json::Value::Null
            };
            w["community"]["rating"] = if i % 2 == 0 {
                (3.0 + (i % 21) as f64 / 10.0).into()
            } else {
                serde_json::Value::Null
            };
            w["community"]["rating_count"] = if i % 2 == 0 {
                (5 + i % 40).into()
            } else {
                serde_json::Value::Null
            };
            w["population"]["latest_count"] = ((i * 37) % 180).into();
            if i % 2 == 1 {
                w["population"]["source"] = "wandur".into();
                w["population"]["observed_at"] = now_rfc3339.into();
            }
            w["established_at"] = format!("{}-06-01T00:00:00+00:00", 1992 + i % 33).into();
        }
        worlds.push(w);
    }
    serde_json::json!({
        "format": "wandur.directory",
        "schema_version": 2,
        "fetched_at": now_rfc3339,
        "worlds": worlds,
    })
    .to_string()
}

/// Art of the fixture worlds: id, size, (sky, ground, sun) as in the C# capture's `ArtHandler`.
const FIXTURE_ART: [(&str, (u32, u32), [u32; 3]); 4] = [
    ("starfall", (1024, 683), [0x2B4C8C, 0x56607A, 0xF4E9C8]),
    ("emberwild", (1024, 683), [0xC9814A, 0x2F5A3A, 0xFFE2A6]),
    ("harbor", (1024, 683), [0x7A5A7E, 0x34465A, 0xFFC98A]),
    ("starfall-banner", (960, 540), [0x1B2A4A, 0x8A6A3A, 0x9FD4FF]),
];

fn rgb(c: u32) -> [f32; 3] {
    [(c >> 16 & 0xFF) as f32, (c >> 8 & 0xFF) as f32, (c & 0xFF) as f32]
}

/// A simple drawn landscape: a sky fading down, a sun, a mountain ridge and rolling ground.
pub fn landscape(width: u32, height: u32, [sky, ground, sun]: [u32; 3]) -> RgbaImage {
    let (sky, ground, sun) = (rgb(sky), rgb(ground), rgb(sun));
    let mix = |a: [f32; 3], b: [f32; 3], t: f32| {
        Rgba([
            (a[0] + (b[0] - a[0]) * t) as u8,
            (a[1] + (b[1] - a[1]) * t) as u8,
            (a[2] + (b[2] - a[2]) * t) as u8,
            255,
        ])
    };
    let (w, h) = (width as f32, height as f32);
    let (sx, sy, sr) = (w * 0.66, h * 0.33, h * 0.14);
    RgbaImage::from_fn(width, height, |x, y| {
        let (fx, fy) = (x as f32, y as f32);
        let u = fx / w;
        let ridge = h * (0.48 + 0.10 * ((u * 7.0).sin() * 0.6 + (u * 2.3 + 1.0).cos() * 0.4));
        let hills = h * (0.78 + 0.05 * (u * 4.0 + 0.5).sin());
        if fy > hills {
            mix(
                ground,
                [0.0, 0.0, 0.0],
                0.45 + 0.2 * (fy - hills) / (h - hills).max(1.0),
            )
        } else if fy > ridge {
            mix(ground, sky, 0.25)
        } else if (fx - sx).powi(2) + (fy - sy).powi(2) < sr * sr {
            mix(sun, [255.0, 255.0, 255.0], 0.15)
        } else {
            mix(sky, [0.0, 0.0, 0.0], 0.35 * (1.0 - fy / ridge.max(1.0)))
        }
    })
}

fn landscape_png(width: u32, height: u32, palette: [u32; 3]) -> Vec<u8> {
    let rgb = image::DynamicImage::ImageRgba8(landscape(width, height, palette)).to_rgb8();
    let mut out = Vec::new();
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut out),
        rgb.as_raw(),
        width,
        height,
        image::ExtendedColorType::Rgb8,
    )
    .expect("PNG encoding");
    out
}

/// The C# reference captures' fictional catalog (`DirectorySiteLookCaptureTests.FictionalCatalog`),
/// with the banner served from `address`.
pub fn fixture_json(address: &str, now_rfc3339: &str) -> String {
    let source = serde_json::json!({"name": "Wayfarer Atlas"});
    let worlds = serde_json::json!([
        {
            "id": "starfall", "name": "Starfall", "host": "starfall.example.org", "port": 4000, "tls_port": 4443,
            "summary": "A living galaxy. Your story to tell.",
            "description": "Beyond the settled systems, the galaxy is still being written. Become a pilot, trader, explorer, or diplomat in a persistent world shaped by its players.\n\nChart forgotten routes, forge alliances, and make a home among the stars. Your choices become part of a shared history.",
            "beginner_friendly": true, "availability": {"online": true},
            "established_at": "2018-05-01T00:00:00+00:00",
            "population": {"latest_count": 142, "source": "wandur", "observed_at": now_rfc3339},
            "features": {"theme": "Science fiction", "kind": "MUD", "language": "English", "codebase": "Custom",
                         "roleplaying": "Encouraged", "player_killing": "Restricted", "world_size": "5000+",
                         "location": "Canada", "development_status": "Operational"},
            "tags": ["Roleplay", "Exploration", "Trading"],
            "generated_artwork_path": "worlds/starfall/art",
            "banner_url": format!("{address}/art/starfall-banner.png"),
            "website_url": "https://starfall.example.org",
            "source": {"name": "Wayfarer Atlas", "listing_url": "https://listing.example.org/starfall"},
            "community": {"rating": 4.6, "rating_count": 12, "rank": 3, "monthly_votes": 90}
        },
        {
            "id": "emberwild", "name": "Emberwild", "host": "emberwild.example.org", "port": 4000,
            "summary": "Ancient forests, uneasy kingdoms, and a world changed by the people who call it home.",
            "description": "Ancient forests and uneasy kingdoms.", "availability": {"online": true},
            "population": {"latest_count": 86, "source": "mudverse"},
            "features": {"theme": "Fantasy", "language": "English", "roleplaying": "Suggested"},
            "tags": ["Fantasy", "Exploration", "Crafting", "Clans"],
            "generated_artwork_path": "worlds/emberwild/art", "source": source
        },
        {
            "id": "harbor", "name": "The Last Harbor", "host": "harbor.example.org", "port": 5000,
            "beginner_friendly": true,
            "summary": "A windswept port of secrets, sea voyages, and stories waiting beyond the shoreline.",
            "availability": {"online": true},
            "population": {"latest_count": 53, "source": "wandur", "observed_at": now_rfc3339},
            "features": {"theme": "Adventure"}, "tags": ["Story-rich"],
            "generated_artwork_path": "worlds/harbor/art", "source": source
        },
        {
            "id": "moss", "name": "Moss & Myth", "host": "moss.example.org", "port": 6000,
            "summary": "Small adventures and lasting friendships in an ever-growing woodland world.",
            "availability": {"online": false}, "features": {"theme": "Fantasy"},
            "tags": ["Social", "Cozy", "Gardening"], "source": source
        }
    ]);
    serde_json::json!({
        "format": "wandur.directory",
        "schema_version": 2,
        "fetched_at": now_rfc3339,
        "worlds": worlds,
    })
    .to_string()
}

/// Now as RFC 3339 (UTC).
pub fn now_rfc3339() -> String {
    let secs = wandur_core::settings::unix_now() as i64;
    let (y, m, d) = wandur_core::directory::time::civil_from_days(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}+00:00",
        t / 3600,
        t % 3600 / 60,
        t % 60
    )
}

impl DirectoryServer {
    /// Listen on 127.0.0.1:`port` (0 picks one).
    pub fn start(port: u16, directory: String, jpeg: Vec<u8>, png: Vec<u8>) -> std::io::Result<DirectoryServer> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        Self::serve_on(listener, directory, Content::Generated { jpeg, png })
    }

    /// The four fictional worlds of the C# reference captures, with their drawn art and
    /// Starfall's banner, all served from this loopback address.
    pub fn start_fixture(port: u16) -> std::io::Result<DirectoryServer> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let address = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
        let mut files = std::collections::HashMap::new();
        for (id, (w, h), palette) in FIXTURE_ART {
            let path = if id.ends_with("-banner") {
                format!("art/{id}.png")
            } else {
                format!("worlds/{id}/art")
            };
            files.insert(path, ("image/png", landscape_png(w, h, palette)));
        }
        Self::serve_on(
            listener,
            fixture_json(&address, &now_rfc3339()),
            Content::Fixture(files),
        )
    }

    fn serve_on(listener: TcpListener, directory: String, content: Content) -> std::io::Result<DirectoryServer> {
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let art_requests = Arc::new(AtomicU64::new(0));
        let directory = Arc::new(RwLock::new(Arc::new(directory.into_bytes())));
        let served = Arc::clone(&directory);
        let content = Arc::new(content);
        let (s, counter) = (Arc::clone(&stop), Arc::clone(&art_requests));
        let routes: Routes = Arc::default();
        let log = RequestLog::default();
        let (r, l) = (Arc::clone(&routes), log.clone());
        thread::Builder::new().name("directory-accept".into()).spawn(move || {
            for stream in listener.incoming() {
                if s.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let d = served.read().map(|d| Arc::clone(&d)).unwrap_or_default();
                let (content, c) = (Arc::clone(&content), Arc::clone(&counter));
                let (routes, log) = (Arc::clone(&r), l.clone());
                let _ = thread::Builder::new()
                    .name("directory-client".into())
                    .spawn(move || serve(stream, &d, &content, &c, &routes, &log));
            }
        })?;
        Ok(DirectoryServer {
            port,
            stop,
            art_requests,
            directory,
            routes,
            log,
        })
    }

    /// Append every request head to this file too (the `--log` option).
    pub fn log_to(&self, file: std::path::PathBuf) {
        if let Ok(mut f) = self.log.file.lock() {
            *f = Some(file);
        }
    }

    /// Answer `path` (without the leading slash) with this from now on.
    pub fn set_route(&self, path: &str, route: Route) {
        if let Ok(mut routes) = self.routes.write() {
            routes.insert(path.trim_matches('/').to_string(), route);
        }
    }

    /// `/client/latest` answers this release (status 200), or with `status` and `body` as given.
    pub fn set_latest(&self, status: &'static str, body: String) {
        self.set_route(
            "client/latest",
            Route {
                status,
                content_type: "application/json",
                body: body.into_bytes(),
                location: None,
            },
        );
    }

    /// The request heads received so far, oldest first.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn requests(&self) -> Vec<String> {
        self.log.heads.lock().map(|h| h.clone()).unwrap_or_default()
    }

    pub fn address(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Serve another directory from now on (a regenerated script pack, a changed listing).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_directory(&self, json: String) {
        if let Ok(mut d) = self.directory.write() {
            *d = Arc::new(json.into_bytes());
        }
    }

    pub fn art_requests(&self) -> u64 {
        self.art_requests.load(Ordering::Relaxed)
    }
}

impl Drop for DirectoryServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn serve(stream: TcpStream, directory: &[u8], content: &Content, art: &AtomicU64, routes: &Routes, log: &RequestLog) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut stream = stream;
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    // Keep the headers for the log.
    let mut head = line.clone();
    let mut header = String::new();
    while reader.read_line(&mut header).is_ok_and(|n| n > 2) {
        head.push_str(&header);
        header.clear();
    }
    log.push(head);
    let path = line.split_whitespace().nth(1).unwrap_or("/");
    let path = path.split('?').next().unwrap_or("").trim_matches('/');
    let route = routes.read().ok().and_then(|r| r.get(path).cloned());
    if let Some(route) = route {
        let location = route
            .location
            .as_deref()
            .map(|l| format!("Location: {l}\r\n"))
            .unwrap_or_default();
        let head = format!(
            "HTTP/1.1 {}\r\nContent-Type: {}\r\n{location}Content-Length: {}\r\nConnection: close\r\n\r\n",
            route.status,
            route.content_type,
            route.body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(&route.body);
        return;
    }
    let (status, kind, body): (&str, &str, &[u8]) = if path == "directory" {
        ("200 OK", "application/json", directory)
    } else if let Content::Fixture(files) = content {
        match files.get(path) {
            Some((kind, body)) => {
                art.fetch_add(1, Ordering::Relaxed);
                ("200 OK", *kind, body.as_slice())
            }
            None => ("404 Not Found", "text/plain", b"not found".as_slice()),
        }
    } else if let (Content::Generated { jpeg, png }, Some(rest)) = (
        content,
        path.strip_prefix("worlds/").and_then(|r| r.strip_suffix("/art")),
    ) {
        art.fetch_add(1, Ordering::Relaxed);
        let index: usize = rest.trim_start_matches("bench-world-").parse().unwrap_or(0);
        if index % 5 == 4 {
            ("200 OK", "image/png", png.as_slice())
        } else {
            ("200 OK", "image/jpeg", jpeg.as_slice())
        }
    } else {
        ("404 Not Found", "text/plain", b"not found")
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use wandur_core::directory::client::{Fetcher, HttpFetcher};

    #[test]
    fn serves_a_directory_the_client_reads_and_art_with_the_right_type() {
        let json = listing_json(25, true, &now_rfc3339());
        let snapshot = wandur_core::directory::snapshot::parse(&json).unwrap();
        assert_eq!(snapshot.worlds.len(), 25);
        assert!(snapshot.worlds.iter().any(|w| w.features.theme == "Horror"));
        let small = generate_art(64, 48, 1);
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode_image(&image::DynamicImage::ImageRgba8(small).to_rgb8())
            .unwrap();
        let server = DirectoryServer::start(0, json, jpeg.clone(), b"\x89PNG fake".to_vec()).unwrap();
        let http = HttpFetcher::new();
        let r = http.get(&format!("{}/directory", server.address()), 1 << 20).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(
            wandur_core::directory::snapshot::parse(std::str::from_utf8(&r.body).unwrap())
                .unwrap()
                .worlds
                .len(),
            25
        );
        let r = http
            .get(
                &format!("{}/worlds/bench-world-3/art?size=400", server.address()),
                1 << 20,
            )
            .unwrap();
        assert_eq!(r.content_type.as_deref(), Some("image/jpeg"));
        assert_eq!(r.body, jpeg);
        let r = http
            .get(&format!("{}/worlds/bench-world-4/art", server.address()), 1 << 20)
            .unwrap();
        assert_eq!(r.content_type.as_deref(), Some("image/png"));
        assert_eq!(
            http.get(&format!("{}/nope", server.address()), 1 << 20).unwrap().status,
            404
        );
        assert_eq!(server.art_requests(), 2);
    }

    #[test]
    fn the_fixture_catalog_is_the_four_csharp_worlds_with_loopback_art() {
        let server = DirectoryServer::start_fixture(0).unwrap();
        let http = HttpFetcher::new();
        let r = http.get(&format!("{}/directory", server.address()), 1 << 20).unwrap();
        let snapshot = wandur_core::directory::snapshot::parse(std::str::from_utf8(&r.body).unwrap()).unwrap();
        let mut names: Vec<&str> = snapshot.worlds.iter().map(|w| w.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["Emberwild", "Moss & Myth", "Starfall", "The Last Harbor"]);
        let starfall = snapshot.worlds.iter().find(|w| w.id == "starfall").unwrap();
        assert!(starfall.banner_url.starts_with(&server.address()));
        let r = http
            .get(&format!("{}/worlds/emberwild/art", server.address()), 4 << 20)
            .unwrap();
        assert_eq!(r.content_type.as_deref(), Some("image/png"));
        assert!(image::load_from_memory(&r.body).is_ok());
        assert_eq!(
            http.get(&format!("{}/worlds/moss/art", server.address()), 1 << 20)
                .unwrap()
                .status,
            404,
            "Moss & Myth has no art (its card shows initials)"
        );
    }
}
