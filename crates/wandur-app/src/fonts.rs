//! Fonts: a bundled monospace family with a real bold face, and system fonts added lazily for
//! characters it cannot draw.
//!
//! JetBrains Mono Regular and Bold (SIL Open Font License 1.1, `assets/fonts/JetBrainsMono-OFL.txt`)
//! are compiled in, and Ubuntu Medium (Ubuntu Font Licence 1.0, `assets/fonts/Ubuntu-UFL.txt`) as
//! the interface's bold: the heavier weight of egui's Ubuntu Light, so bold interface text stays in
//! the interface face instead of a second drawing or the terminal's bold. egui's own fonts stay behind them as fallbacks (Hack, Noto Emoji, the emoji
//! icon font, Ubuntu Light). When the terminal shows a character that none of these can draw, the
//! renderer notes it; [`FallbackFonts::poll`] sends the batch to a worker thread that, once per
//! app run, indexes the system fonts with `fontdb` and then picks faces whose character map covers
//! the characters (preferred families first, then any face). The chosen file is memory mapped, not
//! read into the heap, and added to egui's fallback list. Each character is looked up once; ones
//! that no system font covers are remembered and not looked up again.
//!
//! Not handled (egui has no shaping engine): complex scripts that need shaping (Arabic joining,
//! Indic conjuncts), right-to-left reordering, colour emoji (bitmap and layered colour glyphs draw
//! as nothing or monochrome), and emoji sequences joined with ZWJ.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use egui::{FontData, FontDefinitions, FontFamily, FontId};

/// Family name of the bold terminal face.
pub const BOLD_FAMILY: &str = "mono-bold";
/// Family name of the bold interface face.
pub const UI_BOLD_FAMILY: &str = "ui-bold";

const REGULAR: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
const BOLD: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf");
const UI_BOLD: &[u8] = include_bytes!("../assets/fonts/Ubuntu-Medium.ttf");

pub fn bold_family() -> FontFamily {
    FontFamily::Name(BOLD_FAMILY.into())
}

pub fn ui_bold_family() -> FontFamily {
    FontFamily::Name(UI_BOLD_FAMILY.into())
}

/// The bold of a font: the terminal's bold for monospace text, the interface's for the rest.
pub fn bold_of(font: &FontId) -> FontId {
    let family = if font.family == FontFamily::Monospace {
        bold_family()
    } else {
        ui_bold_family()
    };
    FontId::new(font.size, family)
}

/// Install the bundled fonts: JetBrains Mono first for monospace text, its bold face as
/// [`BOLD_FAMILY`], egui's defaults behind both.
pub fn install(ctx: &egui::Context) {
    let mut defs = FontDefinitions::default();
    defs.font_data
        .insert("JetBrainsMono-Regular".into(), FontData::from_static(REGULAR).into());
    defs.font_data
        .insert("JetBrainsMono-Bold".into(), FontData::from_static(BOLD).into());
    let fallbacks: Vec<String> = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let mut mono = vec!["JetBrainsMono-Regular".to_string()];
    mono.extend(fallbacks.iter().cloned());
    let mut bold = vec!["JetBrainsMono-Bold".to_string()];
    bold.extend(fallbacks);
    defs.families.insert(FontFamily::Monospace, mono);
    defs.families.insert(bold_family(), bold);
    // UI text keeps egui's proportional face; JetBrains Mono behind it draws the arrows, check
    // marks and shapes the C# labels use (Send ↵, Explore world →) that the face lacks.
    if let Some(proportional) = defs.families.get_mut(&FontFamily::Proportional) {
        proportional.push("JetBrainsMono-Regular".into());
    }
    // The interface's bold: Ubuntu Medium, then the interface face's own fallbacks.
    defs.font_data
        .insert("Ubuntu-Medium".into(), FontData::from_static(UI_BOLD).into());
    let mut ui_bold = vec!["Ubuntu-Medium".to_string()];
    ui_bold.extend(
        defs.families
            .get(&FontFamily::Proportional)
            .cloned()
            .unwrap_or_default(),
    );
    defs.families.insert(ui_bold_family(), ui_bold);
    ctx.set_fonts(defs);
}

/// System families tried first, in order, before any other face. Missing ones are skipped.
const PREFERRED: &[&str] = &[
    // macOS
    "Menlo",
    "Hiragino Sans",
    "Hiragino Sans GB",
    "Apple SD Gothic Neo",
    "Apple Symbols",
    "STIX Two Math",
    "Arial Unicode MS",
    // Linux
    "DejaVu Sans Mono",
    "Noto Sans Mono CJK SC",
    "Noto Sans CJK SC",
    "WenQuanYi Micro Hei",
    "Noto Sans Symbols",
    "Noto Sans Symbols 2",
    "DejaVu Sans",
    // Windows
    "Consolas",
    "MS Gothic",
    "Microsoft YaHei",
    "Malgun Gothic",
    "Segoe UI Symbol",
];

/// Faces never used: colour-only or placeholder fonts egui cannot draw usefully.
fn skipped(family: &str) -> bool {
    family.contains("Emoji") || family.contains("LastResort") || family.starts_with('.')
}

/// What the worker found for one face.
struct Found {
    name: String,
    path: PathBuf,
    data: &'static [u8],
    index: u32,
    chars: Vec<char>,
}

struct Reply {
    found: Vec<Found>,
    unsupported: Vec<char>,
    index_ms: Option<f64>,
    search_ms: f64,
}

/// Timings and sizes, for the log and the measurements.
#[derive(Clone, Debug, Default)]
pub struct FallbackStats {
    /// Time to index the system fonts (once).
    pub index_ms: Option<f64>,
    /// Total time spent searching for faces.
    pub search_ms: f64,
    /// Faces added: name, path, mapped bytes, characters it was added for.
    pub added: Vec<(String, PathBuf, usize, usize)>,
    /// Characters no system font covers.
    pub unsupported: usize,
    /// Frame time of the frame after the last font was added (the atlas rebuild).
    pub pause_ms: Option<f64>,
}

/// Lazy system font fallback; one per app.
#[derive(Default)]
pub struct FallbackFonts {
    /// Characters known to draw, or already looked up.
    known: HashSet<char>,
    noted: Vec<char>,
    worker: Option<Sender<Vec<char>>>,
    replies: Option<Receiver<Reply>>,
    in_flight: bool,
    measure_pause: Option<Instant>,
    pub stats: FallbackStats,
    /// Print what happens to stderr (`WANDUR_FONT_LOG=1`).
    pub log: bool,
}

impl FallbackFonts {
    pub fn new() -> Self {
        Self {
            log: std::env::var_os("WANDUR_FONT_LOG").is_some(),
            ..Self::default()
        }
    }

    /// Note a character the terminal is drawing. ASCII is always covered and skipped by callers.
    #[inline]
    pub fn note(&mut self, c: char) {
        if !self.known.contains(&c) {
            self.known.insert(c);
            self.noted.push(c);
        }
    }

    /// Check the noted characters against the loaded fonts, start a lookup for the missing ones,
    /// and add the faces a finished lookup found. Call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context, font: &FontId) {
        if let Some(started) = self.measure_pause.take() {
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            self.stats.pause_ms = Some(ms);
            if self.log {
                eprintln!("wandur fonts: frame after adding fonts took {ms:.1} ms");
            }
        }
        if let Some(replies) = &self.replies
            && let Ok(reply) = replies.try_recv()
        {
            self.in_flight = false;
            self.install(ctx, reply);
        }
        if self.noted.is_empty() {
            return;
        }
        let noted = std::mem::take(&mut self.noted);
        let missing: Vec<char> = ctx.fonts_mut(|f| {
            noted
                .into_iter()
                .filter(|&c| !c.is_control() && !f.has_glyph(font, c))
                .collect()
        });
        if missing.is_empty() {
            return;
        }
        if self.log {
            eprintln!("wandur fonts: looking up {} characters: {:?}", missing.len(), missing);
        }
        let worker = self.worker.get_or_insert_with(|| {
            let (tx, rx) = channel::<Vec<char>>();
            let (reply_tx, reply_rx) = channel();
            let repaint = ctx.clone();
            let _ = std::thread::Builder::new()
                .name("wandur-fonts".into())
                .spawn(move || worker_main(rx, reply_tx, repaint));
            self.replies = Some(reply_rx);
            tx
        });
        if worker.send(missing).is_ok() {
            self.in_flight = true;
        }
    }

    /// Whether a lookup is running (for tests and the log).
    pub fn busy(&self) -> bool {
        self.in_flight
    }

    fn install(&mut self, ctx: &egui::Context, reply: Reply) {
        if let Some(ms) = reply.index_ms {
            self.stats.index_ms = Some(ms);
        }
        self.stats.search_ms += reply.search_ms;
        self.stats.unsupported += reply.unsupported.len();
        if self.log {
            if let Some(ms) = reply.index_ms {
                eprintln!("wandur fonts: indexed system fonts in {ms:.1} ms");
            }
            eprintln!(
                "wandur fonts: search took {:.1} ms; not covered by any font: {:?}",
                reply.search_ms, reply.unsupported
            );
        }
        for found in reply.found {
            if self.log {
                eprintln!(
                    "wandur fonts: adding {} ({}, face {}, {:.1} MB mapped) for {} characters",
                    found.name,
                    found.path.display(),
                    found.index,
                    found.data.len() as f64 / 1048576.0,
                    found.chars.len()
                );
            }
            let mut data = FontData::from_static(found.data);
            data.index = found.index;
            let families = [
                FontFamily::Monospace,
                bold_family(),
                FontFamily::Proportional,
                ui_bold_family(),
            ]
            .into_iter()
            .map(|family| InsertFontFamily {
                family,
                priority: FontPriority::Lowest,
            })
            .collect();
            ctx.add_font(FontInsert {
                name: found.name.clone(),
                data,
                families,
            });
            self.stats
                .added
                .push((found.name, found.path, found.data.len(), found.chars.len()));
            self.measure_pause = Some(Instant::now());
        }
        ctx.request_repaint();
    }
}

fn worker_main(requests: Receiver<Vec<char>>, replies: Sender<Reply>, repaint: egui::Context) {
    let mut db: Option<fontdb::Database> = None;
    // Faces already handed to egui; a second request never adds the same face twice.
    let mut used: HashSet<(PathBuf, u32)> = HashSet::new();
    let mut maps: HashMap<PathBuf, &'static [u8]> = HashMap::new();
    while let Ok(mut chars) = requests.recv() {
        // Coalesce requests that queued up meanwhile.
        while let Ok(more) = requests.try_recv() {
            chars.extend(more);
        }
        let mut index_ms = None;
        let db = db.get_or_insert_with(|| {
            let t = Instant::now();
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            index_ms = Some(t.elapsed().as_secs_f64() * 1000.0);
            db
        });
        let t = Instant::now();
        let (found, unsupported) = search(db, &chars, &mut used, &mut maps);
        let reply = Reply {
            found,
            unsupported,
            index_ms,
            search_ms: t.elapsed().as_secs_f64() * 1000.0,
        };
        if replies.send(reply).is_err() {
            return;
        }
        repaint.request_repaint_after(Duration::ZERO);
    }
}

/// Pick faces for `chars`: preferred families first, then every other face, greedily taking the
/// face that covers the most remaining characters.
fn search(
    db: &fontdb::Database,
    chars: &[char],
    used: &mut HashSet<(PathBuf, u32)>,
    maps: &mut HashMap<PathBuf, &'static [u8]>,
) -> (Vec<Found>, Vec<char>) {
    let mut remaining: Vec<char> = chars.to_vec();
    let mut found = Vec::new();
    let faces: Vec<&fontdb::FaceInfo> = {
        let mut preferred: Vec<&fontdb::FaceInfo> = Vec::new();
        for name in PREFERRED {
            preferred.extend(
                db.faces()
                    .filter(|f| f.families.iter().any(|(fam, _)| fam == name) && regular(f)),
            );
        }
        let rest = db
            .faces()
            .filter(|f| !preferred.iter().any(|p| p.id == f.id) && !f.families.iter().any(|(fam, _)| skipped(fam)));
        let mut rest: Vec<&fontdb::FaceInfo> = rest.collect();
        // Regular, upright faces first among the rest.
        rest.sort_by_key(|f| !regular(f));
        preferred.into_iter().chain(rest).collect()
    };
    while !remaining.is_empty() {
        let mut best: Option<(&fontdb::FaceInfo, Vec<char>)> = None;
        for face in &faces {
            let fontdb::Source::File(path) = &face.source else {
                continue;
            };
            if used.contains(&(path.clone(), face.index)) {
                continue;
            }
            let covered = db
                .with_face_data(face.id, |data, index| {
                    let Ok(font) = skrifa::FontRef::from_index(data, index) else {
                        return Vec::new();
                    };
                    let map = skrifa::MetadataProvider::charmap(&font);
                    remaining.iter().copied().filter(|&c| map.map(c).is_some()).collect()
                })
                .unwrap_or_default();
            let better = best.as_ref().is_none_or(|(_, b)| covered.len() > b.len());
            if !covered.is_empty() && better {
                let complete = covered.len() == remaining.len();
                best = Some((face, covered));
                if complete {
                    break;
                }
            }
        }
        let Some((face, covered)) = best else { break };
        let fontdb::Source::File(path) = &face.source else {
            break;
        };
        let Some(data) = map_file(path, maps) else {
            used.insert((path.clone(), face.index));
            continue;
        };
        used.insert((path.clone(), face.index));
        remaining.retain(|c| !covered.contains(c));
        let family = face.families.first().map_or("system font", |(f, _)| f.as_str());
        found.push(Found {
            name: format!("{family} ({})", face.post_script_name),
            path: path.clone(),
            data,
            index: face.index,
            chars: covered,
        });
    }
    (found, remaining)
}

fn regular(face: &fontdb::FaceInfo) -> bool {
    face.style == fontdb::Style::Normal && face.weight == fontdb::Weight::NORMAL
}

/// Map a font file into memory for the rest of the run. Mapped pages are shared with the file
/// cache and only the pages egui touches become resident, so a large CJK collection costs little.
fn map_file(path: &PathBuf, maps: &mut HashMap<PathBuf, &'static [u8]>) -> Option<&'static [u8]> {
    if let Some(data) = maps.get(path) {
        return Some(data);
    }
    let file = std::fs::File::open(path).ok()?;
    // SAFETY: the mapping is read only. System font files are not modified while the app runs;
    // if one were truncated underneath, reading it could fault, the same risk fontdb itself takes.
    let map = unsafe { memmap2::Mmap::map(&file) }.ok()?;
    let data: &'static [u8] = Box::leak(Box::new(map));
    maps.insert(path.clone(), data);
    Some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_faces_cover_ascii_box_drawing_and_latin() {
        let ctx = egui::Context::default();
        install(&ctx);
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();
        let font = FontId::monospace(13.0);
        let bold = FontId::new(13.0, bold_family());
        ctx.fonts_mut(|f| {
            for c in "Az09─│┌┐└┘├┤┬┴┼═║╔╗╚╝█▓▒░éñüßøÅçœ€".chars() {
                assert!(f.has_glyph(&font, c), "regular lacks {c}");
                assert!(f.has_glyph(&bold, c), "bold lacks {c}");
            }
            // Bold and regular advance alike, so bold text stays on the grid.
            let a = f.glyph_width(&font, 'M');
            let b = f.glyph_width(&bold, 'M');
            assert!((a - b).abs() < 0.01, "{a} vs {b}");
            assert!(!f.has_glyph(&font, '漢'), "CJK comes from the system, not the bundle");
        });
    }

    #[test]
    fn noting_is_once_per_character() {
        let mut fb = FallbackFonts::new();
        fb.note('漢');
        fb.note('漢');
        fb.note('字');
        assert_eq!(fb.noted, ['漢', '字']);
    }
}
