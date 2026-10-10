//! The terminal grid's plain text drawn as one mesh per frame, without laying out text.
//!
//! Laying out every visible run with egui (shaping, glyph lists, a mesh per galley) cost about
//! 1.6 ms and 1.3 MB of allocation per frame under a flood, because the rows on screen are new in
//! every frame. A monospace grid needs none of that: each character sits in its cell. So each
//! character's glyph quad is cut once from a one-character galley (cached per character and
//! style), and every frame the visible cells are copied into one reused mesh.
//!
//! The mesh travels as a [`Galley`] (a clone of a real one with its mesh replaced) rather than a
//! plain mesh, because galley meshes carry texel coordinates that egui turns into texture
//! coordinates at tessellation time: the font atlas can grow during a frame, and a mesh made
//! with the earlier size would sample the wrong place. Glyphs land on whole physical pixels.
//!
//! egui rebuilds its font atlas when the text options change or the atlas fills up; then the
//! cached quads are forgotten ([`AtlasWatch`]).

use std::collections::HashMap;
use std::sync::Arc;

use egui::epaint::{Galley, Mesh, Vertex};
use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Pos2, Rect, Ui, Vec2, pos2};

/// Notices when egui rebuilds its font atlas (text options changed, or the atlas filled up), which
/// makes every galley laid out before it unusable: galleys kept across frames (the Channels rows,
/// directory cards, the grid's glyph quads) must be dropped then. It asks for a one-character
/// galley every frame; egui's galley cache returns the same one until it is rebuilt.
#[derive(Debug, Default)]
pub struct AtlasWatch {
    sentinel: Option<Arc<Galley>>,
}

impl AtlasWatch {
    /// True on the first call and whenever the atlas was rebuilt since the last call.
    pub fn changed(&mut self, ui: &Ui) -> bool {
        let sentinel = ui.fonts_mut(|f| {
            f.layout_job(LayoutJob::simple_singleline(
                "M".into(),
                FontId::monospace(10.0),
                Color32::WHITE,
            ))
        });
        let same = self.sentinel.as_ref().is_some_and(|s| Arc::ptr_eq(s, &sentinel));
        self.sentinel = Some(sentinel);
        !same
    }

    pub fn template(&self) -> Option<&Arc<Galley>> {
        self.sentinel.as_ref()
    }
}

/// Regular, bold, italic, bold italic.
const STYLES: usize = 4;

/// One glyph's quad relative to its cell's top left corner (points), texel coordinates, and its
/// two triangles as indices into the four vertices.
#[derive(Clone, Copy, Debug)]
struct Quad([Vertex; 4], [u32; 6]);

#[derive(Debug, Default)]
pub struct GridText {
    atlas: AtlasWatch,
    font_size: f32,
    pixels_per_point: f32,
    /// ASCII quads by style and code; `None` until looked up, `Some(None)` for no ink.
    ascii: Vec<Option<Option<Quad>>>,
    other: HashMap<(char, u8), Option<Quad>>,
    /// The frame's text, reused when egui has let go of the previous frame's copy.
    galley: Option<Arc<Galley>>,
    /// Glyphs drawn in the last frame.
    pub glyphs: usize,
    /// One-character layouts made in the last frame (0 once every character on screen is known).
    pub laid_out: usize,
}

/// The style index for the quad caches.
pub fn style(bold: bool, italic: bool) -> u8 {
    u8::from(bold) | (u8::from(italic) << 1)
}

fn snap(v: f32, ppp: f32) -> f32 {
    (v * ppp).round() / ppp
}

impl GridText {
    /// Start a frame: check the font atlas is the one the cached quads came from, and empty the
    /// reused mesh.
    pub fn begin(&mut self, ui: &Ui, regular: &FontId) {
        let ppp = ui.ctx().pixels_per_point();
        let same = !self.atlas.changed(ui) && self.font_size == regular.size && self.pixels_per_point == ppp;
        if !same {
            self.ascii.clear();
            self.ascii.resize(STYLES * 128, None);
            self.other.clear();
            self.font_size = regular.size;
            self.pixels_per_point = ppp;
            self.galley = None;
        }
        self.glyphs = 0;
        self.laid_out = 0;
        // Reuse last frame's buffers once egui has dropped its copy (after tessellation).
        if !self.galley.as_mut().is_some_and(|g| Arc::get_mut(g).is_some()) {
            let template = self.atlas.template().expect("checked above");
            self.galley = Some(Arc::new((**template).clone()));
        }
        let mesh = self.mesh_mut();
        // Not `Mesh::clear`, which gives the vertex buffer back.
        mesh.vertices.clear();
        mesh.indices.clear();
        mesh.texture_id = egui::TextureId::default();
    }

    fn mesh_mut(&mut self) -> &mut Mesh {
        let galley = Arc::get_mut(self.galley.as_mut().expect("begin first")).expect("the frame's galley is ours");
        let placed = &mut galley.rows[0];
        if Arc::get_mut(&mut placed.row).is_none() {
            placed.row = Arc::new((*placed.row).clone());
        }
        &mut Arc::get_mut(&mut placed.row).expect("unique row").visuals.mesh
    }

    fn quad(&mut self, ui: &Ui, font: &FontId, c: char, style: u8) -> Option<Quad> {
        let slot = if c.is_ascii() {
            let i = usize::from(style) * 128 + c as usize;
            if let Some(q) = self.ascii[i] {
                return q;
            }
            Some(i)
        } else {
            if let Some(q) = self.other.get(&(c, style)) {
                return *q;
            }
            None
        };
        let format = TextFormat {
            font_id: font.clone(),
            color: Color32::WHITE,
            italics: style & 2 != 0,
            ..Default::default()
        };
        let galley = ui.fonts_mut(|f| f.layout_job(LayoutJob::single_section(c.to_string(), format)));
        self.laid_out += 1;
        let quad = galley.rows.first().and_then(|row| {
            let visuals = &row.visuals;
            let range = visuals.glyph_vertex_range.clone();
            let v = &visuals.mesh.vertices[range.clone()];
            let first = range.start as u32;
            let indices = visuals
                .mesh
                .indices
                .get(visuals.glyph_index_start..visuals.glyph_index_start + 6)?;
            if v.len() != 4 || indices.iter().any(|&i| i < first || i >= first + 4) {
                return None;
            }
            let mut q = [v[0], v[1], v[2], v[3]];
            for vertex in &mut q {
                vertex.pos += row.pos.to_vec2();
            }
            let mut tri = [0u32; 6];
            for (t, i) in tri.iter_mut().zip(indices) {
                *t = i - first;
            }
            Some(Quad(q, tri))
        });
        match slot {
            Some(i) => self.ascii[i] = Some(quad),
            None => {
                self.other.insert((c, style), quad);
            }
        }
        quad
    }

    /// Add a run of single-cell characters starting at `col` of `row` (cells of `cell` points).
    #[expect(clippy::too_many_arguments)]
    pub fn push_run(
        &mut self,
        ui: &Ui,
        font: &FontId,
        text: &str,
        col: usize,
        row: usize,
        cell: Vec2,
        style: u8,
        color: Color32,
    ) {
        let ppp = self.pixels_per_point;
        let y = snap(row as f32 * cell.y, ppp);
        for (i, c) in text.chars().enumerate() {
            if c == ' ' {
                continue;
            }
            let Some(Quad(q, tri)) = self.quad(ui, font, c, style) else {
                continue;
            };
            let x = snap((col + i) as f32 * cell.x, ppp);
            let mesh = self.mesh_mut();
            let base = mesh.vertices.len() as u32;
            for v in q {
                mesh.vertices.push(Vertex {
                    pos: pos2(v.pos.x + x, v.pos.y + y),
                    uv: v.uv,
                    color,
                });
            }
            mesh.indices.extend(tri.iter().map(|i| base + i));
            self.glyphs += 1;
        }
    }

    /// Paint the frame's text with its top left corner at `origin`, clipped by `painter`.
    pub fn finish(&mut self, painter: &egui::Painter, origin: Pos2, size: Vec2) {
        let ppp = self.pixels_per_point;
        let bounds = Rect::from_min_size(Pos2::ZERO, size);
        {
            let galley = Arc::get_mut(self.galley.as_mut().expect("begin first")).expect("the frame's galley is ours");
            galley.rows.truncate(1);
            let placed = &mut galley.rows[0];
            placed.pos = Pos2::ZERO;
            placed.ends_with_newline = false;
            let row = Arc::get_mut(&mut placed.row).expect("unique row");
            row.glyphs.clear();
            row.size = size;
            row.visuals.mesh_bounds = bounds;
            row.visuals.glyph_index_start = 0;
            row.visuals.glyph_vertex_range = 0..row.visuals.mesh.vertices.len();
            galley.num_vertices = row.visuals.mesh.vertices.len();
            galley.num_indices = row.visuals.mesh.indices.len();
            galley.rect = bounds;
            galley.mesh_bounds = bounds;
            galley.pixels_per_point = ppp;
            galley.elided = false;
        }
        if self.glyphs > 0
            && let Some(galley) = &self.galley
        {
            painter.galley(origin, Arc::clone(galley), Color32::WHITE);
        }
    }
}
