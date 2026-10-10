//! Map labels on the canvas (the full map and the mini map alike): text labels laid out at the
//! map's zoom, picture labels from cached textures. Labels set to show above the rooms are drawn
//! after them, the rest before the exits.
//!
//! Pictures are decoded once, on a thread of their own, scaled to at most [`MAX_TEXTURE_SIDE`]
//! on a side, and kept as textures ([`LabelImages`], one per window, shared by every map view)
//! within a byte budget; the least recently drawn go first when it is full. Nothing is decoded
//! while drawing: a picture not decoded yet is drawn as a frame until it is.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use egui::{Color32, ColorImage, FontId, Pos2, Rect, Stroke, StrokeKind, TextureHandle, pos2, vec2};
use wandur_core::map::{MapLabel, RoomMapTracker};

use super::MapViewState;

/// Picture textures kept, in bytes of RGBA (64 MiB).
pub const BUDGET: usize = 64 * 1024 * 1024;
/// A picture is scaled to at most this on a side for drawing.
pub const MAX_TEXTURE_SIDE: u32 = 1024;

enum Entry {
    /// Sent to the decoder.
    Pending,
    Ready {
        texture: TextureHandle,
        bytes: usize,
        /// The frame it was last drawn in.
        used: u64,
    },
    /// Could not be decoded (drawn as a frame).
    Failed,
}

type Decoded = (String, Option<ColorImage>);

/// Label picture textures by picture hash, and the thread that decodes them.
pub struct LabelImages {
    entries: HashMap<String, Entry>,
    budget: usize,
    used: usize,
    frame: u64,
    jobs: Option<Sender<(String, Arc<[u8]>)>>,
    results: Option<Receiver<Decoded>>,
    /// Pictures decoded so far (tests: each is decoded once).
    pub decodes: u64,
}

impl Default for LabelImages {
    fn default() -> Self {
        Self::with_budget(BUDGET)
    }
}

impl LabelImages {
    pub fn with_budget(budget: usize) -> Self {
        Self {
            entries: HashMap::new(),
            budget,
            used: 0,
            frame: 0,
            jobs: None,
            results: None,
            decodes: 0,
        }
    }

    /// The window's cache (made on first use).
    pub fn shared(ctx: &egui::Context) -> Arc<Mutex<LabelImages>> {
        ctx.data_mut(|d| {
            d.get_temp_mut_or_insert_with(egui::Id::new("map-label-images"), || {
                Arc::new(Mutex::new(LabelImages::default()))
            })
            .clone()
        })
    }

    /// Bytes of textures held.
    pub fn used(&self) -> usize {
        self.used
    }

    pub fn is_ready(&self, hash: &str) -> bool {
        matches!(self.entries.get(hash), Some(Entry::Ready { .. }))
    }

    /// Pictures still being decoded.
    pub fn pending(&self) -> usize {
        self.entries.values().filter(|e| matches!(e, Entry::Pending)).count()
    }

    /// This frame (egui's pass number): take what the decoder finished. Called by every view
    /// that draws labels; the second call in one pass only looks for finished pictures.
    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        self.frame = self.frame.max(ctx.cumulative_pass_nr() + 1);
        self.poll(ctx);
    }

    /// The next frame, by hand (tests).
    #[cfg(test)]
    fn tick(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        self.poll(ctx);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(results) = &self.results else { return };
        let finished: Vec<Decoded> = results.try_iter().collect();
        for (hash, image) in finished {
            if !matches!(self.entries.get(&hash), Some(Entry::Pending)) {
                continue;
            }
            match image {
                Some(image) => {
                    let bytes = image.pixels.len() * 4;
                    let texture = ctx.load_texture(format!("map-label-{hash}"), image, egui::TextureOptions::LINEAR);
                    self.used += bytes;
                    self.entries.insert(
                        hash,
                        Entry::Ready {
                            texture,
                            bytes,
                            used: self.frame,
                        },
                    );
                }
                None => {
                    self.entries.insert(hash, Entry::Failed);
                }
            }
        }
        self.evict();
    }

    /// Drop the least recently drawn textures until within the budget (never one drawn in
    /// this frame).
    fn evict(&mut self) {
        while self.used > self.budget {
            let oldest = self
                .entries
                .iter()
                .filter_map(|(h, e)| match e {
                    Entry::Ready { used, .. } if *used < self.frame => Some((*used, h.clone())),
                    _ => None,
                })
                .min();
            let Some((_, hash)) = oldest else { break };
            if let Some(Entry::Ready { bytes, .. }) = self.entries.remove(&hash) {
                self.used -= bytes;
            }
        }
    }

    /// The texture for a picture, or `None` while it is decoded (asked for on first sight).
    pub fn texture(&mut self, ctx: &egui::Context, hash: &str, data: &Arc<[u8]>) -> Option<egui::TextureId> {
        match self.entries.get_mut(hash) {
            Some(Entry::Ready { texture, used, .. }) => {
                *used = self.frame;
                return Some(texture.id());
            }
            Some(Entry::Pending | Entry::Failed) => return None,
            None => {}
        }
        if self.jobs.is_none() {
            self.spawn(ctx.clone());
        }
        if let Some(jobs) = &self.jobs
            && jobs.send((hash.to_string(), Arc::clone(data))).is_ok()
        {
            self.entries.insert(hash.to_string(), Entry::Pending);
            self.decodes += 1;
        }
        None
    }

    fn spawn(&mut self, ctx: egui::Context) {
        let (jobs, inbox) = channel::<(String, Arc<[u8]>)>();
        let (send, results) = channel();
        let spawned = std::thread::Builder::new()
            .name("wandur-map-labels".into())
            .spawn(move || {
                for (hash, data) in inbox {
                    let image = decode(&data);
                    if send.send((hash, image)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        if spawned.is_ok() {
            self.jobs = Some(jobs);
            self.results = Some(results);
        }
    }

    /// Wait until every picture asked for is decoded (tests and captures).
    pub fn settle(&mut self, ctx: &egui::Context) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while self.pending() > 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.poll(ctx);
        }
    }
}

/// Decode a picture, scaled to at most [`MAX_TEXTURE_SIDE`] on a side.
pub fn decode(data: &[u8]) -> Option<ColorImage> {
    let image = image::load_from_memory(data).ok()?;
    let image = if image.width() > MAX_TEXTURE_SIDE || image.height() > MAX_TEXTURE_SIDE {
        image.resize(
            MAX_TEXTURE_SIDE,
            MAX_TEXTURE_SIDE,
            image::imageops::FilterType::Triangle,
        )
    } else {
        image
    };
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// A label's rectangle on screen.
pub fn label_rect(state: &MapViewState, canvas: Rect, label: &MapLabel) -> Rect {
    Rect::from_two_pos(
        state.project(canvas, label.x, label.y),
        state.project(canvas, label.x + label.width, label.y - label.height),
    )
}

/// `#RRGGBB` as a colour.
pub fn color(hex: Option<&str>) -> Option<Color32> {
    wandur_core::settings::parse_color(hex?).map(|(r, g, b)| Color32::from_rgb(r, g, b))
}

/// Draw the labels of the shown area and floor that go above (or under) the rooms. Their
/// rectangles are added to `hits` (the editor picks labels by them). `offset` moves (and
/// `resize` sizes) the label being dragged in the editor.
#[allow(clippy::too_many_arguments)]
pub fn paint(
    painter: &egui::Painter,
    canvas: Rect,
    tracker: &RoomMapTracker,
    state: &MapViewState,
    above: bool,
    text_color: Color32,
    preview: Option<(&str, MapLabel)>,
    hits: &mut Vec<(String, Rect)>,
) {
    let ctx = painter.ctx().clone();
    let images = LabelImages::shared(&ctx);
    let mut images = images.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    images.begin_frame(&ctx);
    for stored in tracker.labels() {
        let label = match &preview {
            Some((id, moved)) if *id == stored.id => moved,
            _ => stored,
        };
        if label.above_rooms != above || label.area_key() != state.area || label.z != state.floor {
            continue;
        }
        let rect = label_rect(state, canvas, label);
        hits.push((label.id.clone(), rect));
        if !canvas.intersects(rect) || rect.width() < 1.0 || rect.height() < 1.0 {
            continue;
        }
        let alpha = label.opacity.clamp(0.05, 1.0) as f32;
        if let Some(hash) = label.image.as_deref() {
            let texture = tracker
                .image(hash)
                .and_then(|image| images.texture(&ctx, hash, &image.data));
            match texture {
                Some(id) => {
                    painter.image(
                        id,
                        rect,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE.gamma_multiply(alpha),
                    );
                }
                None => {
                    painter.rect_stroke(
                        rect,
                        2.0,
                        Stroke::new(1.0, text_color.gamma_multiply(0.4 * alpha)),
                        StrokeKind::Inside,
                    );
                }
            }
            continue;
        }
        if let Some(fill) = color(label.background.as_deref()) {
            painter.rect_filled(rect, 2.0, fill.gamma_multiply(alpha));
        }
        let size = (label.font_size * state.zoom) as f32;
        if size < 3.0 {
            continue;
        }
        let ink = color(label.color.as_deref())
            .unwrap_or(text_color)
            .gamma_multiply(alpha);
        let galley = painter.layout(
            label.text.clone(),
            FontId::proportional(size.min(400.0)),
            ink,
            (rect.width() - 4.0).max(size),
        );
        let at = Pos2::new(rect.center().x, rect.center().y - galley.size().y / 2.0);
        let clip = rect.intersect(canvas);
        let mut job_painter = painter.clone();
        job_painter.set_clip_rect(clip);
        job_painter.galley(pos2(at.x - galley.size().x / 2.0, at.y), galley, ink);
    }
}

/// The selection frame of a label and its resize grip (the editor's overlay).
pub fn paint_selection(painter: &egui::Painter, rect: Rect, accent: Color32, panel: Color32) {
    painter.rect_stroke(rect.expand(2.0), 2.0, Stroke::new(2.0, accent), StrokeKind::Outside);
    let grip = grip_rect(rect);
    painter.rect(grip, 1.0, panel, Stroke::new(1.5, accent), StrokeKind::Inside);
}

/// The resize grip at a label's bottom right corner.
pub fn grip_rect(rect: Rect) -> Rect {
    Rect::from_center_size(rect.right_bottom() + vec2(2.0, 2.0), vec2(9.0, 9.0))
}

/// Text for a label too small to read, as a tooltip names it.
pub fn describe(label: &MapLabel) -> String {
    if label.image.is_some() {
        wandur_core::l10n::t(wandur_core::l10n::S::MapLabelPicture).to_string()
    } else {
        label.text.lines().next().unwrap_or_default().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Arc<[u8]> {
        let img = image::RgbaImage::from_fn(w, h, |x, _| image::Rgba([x as u8, 0, 0, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        Arc::from(out)
    }

    #[test]
    fn a_picture_is_decoded_once_then_drawn_from_its_texture() {
        let ctx = egui::Context::default();
        let mut images = LabelImages::default();
        let data = png(20, 10);
        images.tick(&ctx);
        assert!(images.texture(&ctx, "a", &data).is_none(), "first sight: decoding");
        assert!(
            images.texture(&ctx, "a", &data).is_none(),
            "still pending, not asked again"
        );
        images.settle(&ctx);
        assert!(images.is_ready("a"));
        for _ in 0..5 {
            images.tick(&ctx);
            assert!(images.texture(&ctx, "a", &data).is_some());
        }
        assert_eq!(images.decodes, 1);
        assert_eq!(images.used(), 20 * 10 * 4);
        // Junk fails once and is not asked again.
        let junk: Arc<[u8]> = Arc::from(&b"\x89PNG\r\n\x1a\nnot really"[..]);
        assert!(images.texture(&ctx, "b", &junk).is_none());
        images.settle(&ctx);
        assert!(images.texture(&ctx, "b", &junk).is_none());
        assert_eq!(images.decodes, 2);
    }

    #[test]
    fn the_budget_lets_the_least_recently_drawn_go() {
        let ctx = egui::Context::default();
        // Room for two 16 by 16 pictures.
        let mut images = LabelImages::with_budget(2 * 16 * 16 * 4);
        let pictures: Vec<Arc<[u8]>> = (0..3).map(|_| png(16, 16)).collect();
        images.tick(&ctx);
        images.texture(&ctx, "0", &pictures[0]);
        images.texture(&ctx, "1", &pictures[1]);
        images.settle(&ctx);
        images.tick(&ctx);
        assert!(images.texture(&ctx, "1", &pictures[1]).is_some());
        images.tick(&ctx);
        assert!(images.texture(&ctx, "1", &pictures[1]).is_some());
        images.texture(&ctx, "2", &pictures[2]);
        images.settle(&ctx);
        images.tick(&ctx);
        assert!(images.used() <= 2 * 16 * 16 * 4);
        assert!(!images.is_ready("0"), "drawn longest ago");
        assert!(images.is_ready("1") && images.is_ready("2"));
        // A picture that went is decoded again when drawn again.
        assert!(images.texture(&ctx, "0", &pictures[0]).is_none());
        assert_eq!(images.decodes, 4);
    }

    #[test]
    fn large_pictures_are_scaled_for_drawing() {
        let image = decode(&png(2048, 512)).unwrap();
        assert_eq!(image.size, [1024, 256]);
        assert!(decode(b"nope").is_none());
    }
}
