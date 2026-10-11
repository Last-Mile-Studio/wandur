//! Sheet tabs, a modern take on a spreadsheet's tabs along the bottom of its sheet: a strip of
//! tabs hanging from the edge of the content above them. The shown tab takes the content's colour
//! and joins it with no line between, a thin accent line along its free edge; the others sit back
//! a little, shorter and in a muted grey, tucked under the line. Tabs overlap slightly so the
//! shown one is in front.
//!
//! Two strips use it: a session's views under its command box (Play, Map, Diagnostics in
//! `terminal_view.rs`) and the panels sharing a dock leaf (along the leaf's bottom, `shell.rs`).
//! Each caller paints its strip's ground and the line along its top; this module paints and
//! senses the tabs.

use egui::epaint::{Mesh, PathShape, PathStroke};
use egui::{Color32, FontId, Id, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};

use crate::theme::{Theme, mix};

/// The tabs' shape: both sides softly slanted with rounded free corners, overlapping, the shown
/// tab flaring into the line.
pub const GEOMETRY: Geometry = Geometry {
    slant_left: 5.0,
    slant_right: 5.0,
    overlap: 6.0,
    radius: 5.0,
    pad: 11.0,
    flare: 5.0,
};

/// A tab's shape, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    /// How far each side leans in from the joined edge to the free edge.
    pub slant_left: f32,
    pub slant_right: f32,
    /// How much a tab overlaps the next (negative: a gap between them).
    pub overlap: f32,
    /// The free corners' radius.
    pub radius: f32,
    /// Room between a side and the label.
    pub pad: f32,
    /// The shown tab's small outward curves where it meets the line (0: none).
    pub flare: f32,
}

/// How far the shown tab's free edge reaches past the others' (they sit back by this much).
pub const RECESS: f32 = 2.0;
/// The accent line along the shown tab's free edge.
pub const ACCENT: f32 = 2.0;

/// A strip's colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// The content above the strip, which the shown tab takes, and its text.
    pub content: Color32,
    pub content_text: Color32,
    /// The other tabs, under the pointer, and their text.
    pub face: Color32,
    pub face_hover: Color32,
    pub face_text: Color32,
    /// The tabs' outline and the line along the strip's top.
    pub edge: Color32,
    pub line: Color32,
    pub accent: Color32,
}

impl Palette {
    /// Colours for a strip on `ground` (its surface) under content of colour `content`.
    /// `ground_text` is the text colour the ground was made for.
    pub fn new(theme: &Theme, ground: Color32, ground_text: Color32, content: Color32, content_text: Color32) -> Self {
        // The others sit back: a step darker than the ground on a light one; on a dark one,
        // where the shown tab and the content above are as dark as the ground, a muted grey a
        // step toward the text, so the shown one stands out.
        let light = crate::theme::is_light(ground);
        let face = if light {
            mix(ground, Color32::BLACK, 0.08)
        } else {
            mix(ground, ground_text, 0.13)
        };
        Palette {
            content,
            content_text,
            face,
            face_hover: mix(face, ground_text, 0.08),
            face_text: mix(ground_text, face, 0.25),
            edge: edge(theme.border, face, ground_text),
            line: theme.border,
            // Lifted toward the content's text where the accent would not show on the content
            // (Hull's teal on its dark transcript).
            accent: if crate::theme::contrast(theme.accent, content) < 2.2 {
                mix(theme.accent, content_text, 0.4)
            } else {
                theme.accent
            },
        }
    }
}

/// The tabs' outline: the theme's line softened toward the face, or where that line is too faint
/// to show on it (light themes), a touch of the text.
fn edge(border: Color32, face: Color32, text: Color32) -> Color32 {
    let soft = mix(border, face, 0.3);
    if crate::theme::contrast(soft, face) < 1.3 {
        mix(face, text, 0.22)
    } else {
        soft
    }
}

/// One tab to draw.
#[derive(Clone, Debug)]
pub struct Sheet<'a> {
    pub label: &'a str,
    /// Shown (two can be: Play and Map side by side).
    pub selected: bool,
    /// Its label in this colour instead (a panel with unread messages).
    pub tint: Option<Color32>,
}

/// The tabs' widths for their labels.
pub fn widths(ui: &Ui, sheets: &[Sheet<'_>], font: &FontId, geometry: Geometry) -> Vec<f32> {
    ui.fonts_mut(|f| {
        sheets
            .iter()
            .map(|s| {
                let text: f32 = s.label.chars().map(|c| f.glyph_width(font, c)).sum();
                (text + 2.0 * geometry.pad + geometry.slant_left + geometry.slant_right).ceil()
            })
            .collect()
    })
}

/// The narrowest a tab gets when the strip is short of room (its label ellipsized).
pub const NARROWEST: f32 = 44.0;

/// The tabs' widths in `room` points: as wanted when they fit, else the widest shrink first,
/// evenly, to a common cap no narrower than [`NARROWEST`] (past that they are cut at the
/// strip's end).
pub fn fit(wanted: &[f32], room: f32, geometry: Geometry) -> Vec<f32> {
    if total(wanted, geometry) <= room {
        return wanted.to_vec();
    }
    let within = |cap: f32| {
        let capped: Vec<f32> = wanted.iter().map(|w| w.min(cap)).collect();
        total(&capped, geometry) <= room
    };
    let (mut low, mut high) = (NARROWEST, wanted.iter().copied().fold(NARROWEST, f32::max));
    for _ in 0..24 {
        let cap = (low + high) / 2.0;
        if within(cap) {
            low = cap;
        } else {
            high = cap;
        }
    }
    wanted.iter().map(|w| w.min(low).floor()).collect()
}

/// The tabs' slots from `left`, each overlapping the one before by the overlap.
pub fn place(left: f32, widths: &[f32], geometry: Geometry) -> Vec<(f32, f32)> {
    let mut x = left;
    widths
        .iter()
        .map(|w| {
            let slot = (x, x + w);
            x += w - geometry.overlap;
            slot
        })
        .collect()
}

/// The width the tabs take side by side.
pub fn total(widths: &[f32], geometry: Geometry) -> f32 {
    let sum: f32 = widths.iter().sum();
    sum - geometry.overlap * widths.len().saturating_sub(1) as f32
}

/// Where each tab takes the pointer: its slot, with an overlap shared at its middle, or all of it
/// going to a shown tab (which is drawn in front). No two overlap.
pub fn hit_spans(slots: &[(f32, f32)], selected: &[bool]) -> Vec<(f32, f32)> {
    let mut spans = slots.to_vec();
    for i in 1..slots.len() {
        let (left_end, right_start) = (slots[i - 1].1, slots[i].0);
        if left_end <= right_start {
            continue;
        }
        let cut = match (selected[i - 1], selected[i]) {
            (true, false) => left_end,
            (false, true) => right_start,
            _ => (left_end + right_start) / 2.0,
        };
        spans[i - 1].1 = cut;
        spans[i].0 = cut;
    }
    spans
}

/// A tab's outline hanging from `top` to `bottom` in the slot `x0..x1`, from the joined edge's
/// left end round to its right end: the joined corners sharp, the free ones rounded.
pub fn outline(x0: f32, x1: f32, top: f32, bottom: f32, geometry: Geometry) -> Vec<Pos2> {
    let corners = [
        pos2(x0, top),
        pos2(x0 + geometry.slant_left, bottom),
        pos2(x1 - geometry.slant_right, bottom),
        pos2(x1, top),
    ];
    let mut points = vec![corners[0]];
    for i in 1..3 {
        rounded_corner(&mut points, corners[i - 1], corners[i], corners[i + 1], geometry.radius);
    }
    points.push(corners[3]);
    points
}

/// The corner at `at` between the edges from `from` and to `to`, rounded with radius `r` (a
/// quadratic curve between the points where the rounding meets each edge).
fn rounded_corner(points: &mut Vec<Pos2>, from: Pos2, at: Pos2, to: Pos2, r: f32) {
    let (a, b) = (from - at, to - at);
    let (la, lb) = (a.length(), b.length());
    if r <= 0.0 || la < 0.01 || lb < 0.01 {
        points.push(at);
        return;
    }
    let t = r.min(la / 2.0).min(lb / 2.0);
    let (p, q) = (at + a / la * t, at + b / lb * t);
    const STEPS: usize = 6;
    for k in 0..=STEPS {
        let s = k as f32 / STEPS as f32;
        let u = 1.0 - s;
        points.push(pos2(
            u * u * p.x + 2.0 * u * s * at.x + s * s * q.x,
            u * u * p.y + 2.0 * u * s * at.y + s * s * q.y,
        ));
    }
}

/// Paint one tab. `shown`: the selected look, joined to the content; `hot`: under the pointer.
#[allow(clippy::too_many_arguments)]
pub fn paint(
    painter: &egui::Painter,
    slot: (f32, f32),
    strip: Rect,
    geometry: Geometry,
    palette: &Palette,
    shown: bool,
    hot: bool,
) {
    let (x0, x1) = slot;
    let bottom = (strip.bottom() - 2.0).round();
    if shown {
        let top = strip.top();
        let points = outline(x0, x1, top, bottom, geometry);
        painter.add(Shape::convex_polygon(points.clone(), palette.content, Stroke::NONE));
        flares(painter, x0, x1, top, bottom, geometry, palette.content);
        // The sides and the free edge, not the joined one, flowing out of the line through the
        // flares.
        painter.add(PathShape::line(
            flared_outline(points, top, bottom, geometry),
            PathStroke::new(1.0, palette.edge),
        ));
        // The accent along the free edge, inside it.
        let inset = geometry.radius.max(3.0);
        let y = bottom - ACCENT / 2.0 - 0.5;
        let left = x0 + geometry.slant_left + inset;
        let right = x1 - geometry.slant_right - inset;
        if right > left {
            painter.line_segment([pos2(left, y), pos2(right, y)], Stroke::new(ACCENT, palette.accent));
        }
    } else {
        // Under the line along the strip's top, and a little shorter.
        let top = strip.top() + 1.0;
        let points = outline(x0, x1, top, bottom - RECESS, geometry);
        let fill = if hot { palette.face_hover } else { palette.face };
        painter.add(Shape::convex_polygon(points.clone(), fill, Stroke::NONE));
        painter.add(PathShape::line(points, PathStroke::new(1.0, palette.edge)));
    }
}

/// The shown tab's outline with its flares: from the line, round into the left side, the free
/// edge, and out of the right side back to the line.
fn flared_outline(mut points: Vec<Pos2>, top: f32, bottom: f32, geometry: Geometry) -> Vec<Pos2> {
    let r = geometry.flare;
    let (Some(&first), Some(&last)) = (points.first(), points.last()) else {
        return points;
    };
    if r <= 0.0 {
        return points;
    }
    let height = bottom - top;
    let left = flare_curve(first, geometry.slant_left * r / height, -1.0, r);
    let right = flare_curve(last, -geometry.slant_right * r / height, 1.0, r);
    points.remove(0);
    points.pop();
    // Drop the side's points within the flares (the curves take them over).
    points.retain(|p| p.y > top + r);
    // The curves end on the strip's line (drawn through the middle of its pixel row).
    left.into_iter()
        .rev()
        .chain(points)
        .chain(right)
        .map(|p| if p.y <= top { pos2(p.x, top + 0.5) } else { p })
        .collect()
}

/// A flare's curve at a top `corner`, from the side `r` down (leaning `lean` across) to the line
/// `r` out (`out` is -1 to the left, 1 to the right): bowed toward the corner.
fn flare_curve(corner: Pos2, lean: f32, out: f32, r: f32) -> Vec<Pos2> {
    let down = corner + vec2(lean, r);
    let along = corner + vec2(out * r, 0.0);
    const STEPS: usize = 6;
    (0..=STEPS)
        .map(|k| {
            let s = k as f32 / STEPS as f32;
            let u = 1.0 - s;
            pos2(
                u * u * down.x + 2.0 * u * s * corner.x + s * s * along.x,
                u * u * down.y + 2.0 * u * s * corner.y + s * s * along.y,
            )
        })
        .collect()
}

/// The shown tab's small outward curves where its sides meet the strip's top, so it flows into
/// the content above. A fan from each meeting point fills the curved corner.
fn flares(painter: &egui::Painter, x0: f32, x1: f32, top: f32, bottom: f32, geometry: Geometry, color: Color32) {
    let r = geometry.flare;
    if r <= 0.0 {
        return;
    }
    let height = bottom - top;
    for (corner, lean, out) in [
        (pos2(x0, top), geometry.slant_left, -1.0),
        (pos2(x1, top), -geometry.slant_right, 1.0),
    ] {
        // Down the side by r, out along the top by r, a curve bowed toward the corner between.
        let mut mesh = Mesh::default();
        mesh.colored_vertex(corner, color);
        for (k, p) in flare_curve(corner, lean * r / height, out, r).into_iter().enumerate() {
            mesh.colored_vertex(p, color);
            if k > 0 {
                let n = mesh.vertices.len() as u32;
                mesh.add_triangle(0, n - 2, n - 1);
            }
        }
        painter.add(Shape::mesh(mesh));
    }
}

/// The id of tab `index` of the strip with this id (it takes the keyboard focus).
pub fn tab_id(strip: Id, index: usize) -> Id {
    strip.with(("sheet", index))
}

/// Where the strip with this id took the pointer for each tab last frame (tests point at them).
pub fn tab_rects(ctx: &egui::Context, id: Id) -> Vec<Rect> {
    ctx.data(|d| d.get_temp::<Vec<Rect>>(id)).unwrap_or_default()
}

/// What the person did to the strip's tabs this frame (one response per tab, in order).
pub struct Shown {
    pub responses: Vec<egui::Response>,
    /// The width the tabs took from the strip's left.
    pub width: f32,
}

/// Draw `sheets` as tabs from `left` in `strip` (its top is the edge they hang from) and sense
/// them: each a named tab for screen readers, focusable from the keyboard.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    id: Id,
    strip: Rect,
    left: f32,
    sheets: &[Sheet<'_>],
    font: &FontId,
    palette: &Palette,
) -> Shown {
    let geometry = GEOMETRY;
    let widths = fit(
        &widths(ui, sheets, font, geometry),
        strip.right() - left - 4.0,
        geometry,
    );
    let slots = place(left, &widths, geometry);
    let selected: Vec<bool> = sheets.iter().map(|s| s.selected).collect();
    let spans = hit_spans(&slots, &selected);
    let mut responses = Vec::with_capacity(sheets.len());
    let mut hits = Vec::with_capacity(sheets.len());
    // The strip may reach into the dock's margin under a panel's body, past the body's clip: the
    // tabs are sensed and painted to the strip's own edges.
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(strip).id_salt(id));
    ui.set_clip_rect(strip);
    for (i, sheet) in sheets.iter().enumerate() {
        let hit = Rect::from_min_max(pos2(spans[i].0, strip.top()), pos2(spans[i].1, strip.bottom()));
        let response = ui.interact(hit, tab_id(id, i), Sense::click_and_drag());
        crate::a11y::toggle(&response, egui::accesskit::Role::Tab, sheet.label, sheet.selected);
        responses.push(response);
        hits.push(hit);
    }
    ui.data_mut(|d| d.insert_temp(id, hits));
    // The others from the right, so each one's slanted side lies over the next; the shown ones
    // last, in front.
    let painter = ui.painter().clone();
    let order: Vec<usize> = (0..sheets.len())
        .rev()
        .filter(|&i| !selected[i])
        .chain((0..sheets.len()).filter(|&i| selected[i]))
        .collect();
    let pointer_down = ui.input(|i| i.pointer.any_down());
    for i in order {
        let sheet = &sheets[i];
        let response = &responses[i];
        let hot = response.hovered() && !pointer_down;
        paint(&painter, slots[i], strip, geometry, palette, sheet.selected, hot);
        let (x0, x1) = slots[i];
        let bottom = strip.bottom() - 2.0 - if sheet.selected { 0.0 } else { RECESS };
        let body = Rect::from_min_max(
            pos2(x0 + geometry.slant_left, strip.top() + 2.0),
            pos2(
                x1 - geometry.slant_right,
                bottom - if sheet.selected { ACCENT } else { 0.0 },
            ),
        );
        let color = match sheet.tint {
            Some(tint) if !sheet.selected => tint,
            _ if sheet.selected => palette.content_text,
            _ => palette.face_text,
        };
        let mut job = egui::text::LayoutJob::simple_singleline(sheet.label.to_string(), font.clone(), color);
        job.wrap = egui::text::TextWrapping::truncate_at_width((body.width() - 2.0 * geometry.pad + 4.0).max(8.0));
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        painter.galley(body.center() - galley.size() / 2.0, galley, color);
        if response.has_focus() {
            let ring = outline(x0, x1, strip.top() + 1.0, bottom, geometry);
            let inset: Vec<Pos2> = ring
                .iter()
                .map(|p| {
                    pos2(
                        p.x.clamp(x0 + 2.0, x1 - 2.0),
                        p.y.min(bottom - 2.0).max(strip.top() + 3.0),
                    )
                })
                .collect();
            painter.add(PathShape::closed_line(inset, PathStroke::new(1.0, palette.accent)));
        }
    }
    Shown {
        responses,
        width: total(&widths, geometry),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_overlap_by_the_slant_or_keep_a_gap() {
        let a = GEOMETRY;
        let slots = place(10.0, &[60.0, 80.0, 50.0], a);
        assert_eq!(slots[0], (10.0, 70.0));
        assert_eq!(slots[1], (70.0 - a.overlap, 150.0 - a.overlap));
        assert_eq!(total(&[60.0, 80.0, 50.0], a), 190.0 - 2.0 * a.overlap);
        assert_eq!(slots[2].1 - slots[0].0, total(&[60.0, 80.0, 50.0], a));
        let c = Geometry {
            overlap: -2.0,
            ..GEOMETRY
        };
        let slots = place(0.0, &[60.0, 60.0], c);
        assert!(slots[1].0 > slots[0].1, "a negative overlap keeps a gap: {slots:?}");
    }

    #[test]
    fn short_of_room_the_widest_tabs_shrink_first_to_a_floor() {
        let g = GEOMETRY;
        let wanted = [60.0, 120.0, 90.0];
        assert_eq!(fit(&wanted, 500.0, g), wanted, "room for all");
        let room = 200.0;
        let w = fit(&wanted, room, g);
        assert!(total(&w, g) <= room, "{w:?}");
        assert!(total(&w, g) > room - 4.0, "uses the room: {w:?}");
        assert_eq!(w[0], 60.0, "the short one keeps its width");
        assert!((w[1] - w[2]).abs() <= 1.0, "the wide ones share: {w:?}");
        let w = fit(&wanted, 60.0, g);
        assert!(w.iter().all(|&x| x >= NARROWEST - 1.0), "never under the floor: {w:?}");
    }

    #[test]
    fn the_pointer_finds_one_tab_and_the_shown_one_takes_the_overlap() {
        let slots = [(0.0, 60.0), (54.0, 120.0), (114.0, 170.0)];
        // None shown: overlaps split at their middles, no gaps, no overlaps.
        let spans = hit_spans(&slots, &[false, false, false]);
        assert_eq!(spans, [(0.0, 57.0), (57.0, 117.0), (117.0, 170.0)]);
        // The middle one shown: it keeps both overlaps.
        let spans = hit_spans(&slots, &[false, true, false]);
        assert_eq!(spans, [(0.0, 54.0), (54.0, 120.0), (120.0, 170.0)]);
        // The first shown: it keeps its right overlap.
        let spans = hit_spans(&slots, &[true, false, false]);
        assert_eq!(spans[0], (0.0, 60.0));
        assert_eq!(spans[1].0, 60.0);
        // A gap (C) is left alone.
        let spans = hit_spans(&[(0.0, 50.0), (52.0, 100.0)], &[false, true]);
        assert_eq!(spans, [(0.0, 50.0), (52.0, 100.0)]);
    }

    #[test]
    fn the_outline_hangs_from_the_joined_edge_with_rounded_free_corners() {
        for (variant, g) in [
            ("shown", GEOMETRY),
            (
                "upright",
                Geometry {
                    slant_left: 0.0,
                    slant_right: 0.0,
                    ..GEOMETRY
                },
            ),
        ] {
            let points = outline(0.0, 80.0, 10.0, 34.0, g);
            assert_eq!(points.first(), Some(&pos2(0.0, 10.0)), "{variant}");
            assert_eq!(points.last(), Some(&pos2(80.0, 10.0)), "{variant}");
            // Inside the slot and between the edges.
            assert!(
                points
                    .iter()
                    .all(|p| (0.0..=80.0).contains(&p.x) && (10.0..=34.0).contains(&p.y))
            );
            // The free edge's ends lean in by the slants; its corners are rounded, so no point
            // sits on the sharp corner itself.
            let bottom: Vec<&Pos2> = points.iter().filter(|p| (p.y - 34.0).abs() < 0.01).collect();
            assert!(!bottom.is_empty(), "{variant}");
            let min = bottom.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
            let max = bottom.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max);
            assert!(min >= g.slant_left + g.radius - 0.01, "{variant} {min}");
            assert!(max <= 80.0 - g.slant_right - g.radius + 0.01, "{variant} {max}");
            // Convex: every turn goes the same way (the fill is drawn as a convex polygon).
            let mut sign = 0.0f32;
            for w in points.windows(3) {
                let cross = (w[1] - w[0]).x * (w[2] - w[1]).y - (w[1] - w[0]).y * (w[2] - w[1]).x;
                if cross.abs() > 1e-4 {
                    assert!(sign == 0.0 || sign == cross.signum(), "{variant} not convex");
                    sign = cross.signum();
                }
            }
        }
    }

    #[test]
    fn the_others_sit_back_from_the_ground_in_light_and_dark_themes() {
        for name in crate::theme::Theme::names() {
            let theme = crate::theme::Theme::preset(name);
            let p = Palette::new(&theme, theme.panel, theme.text, theme.terminal, theme.terminal_text);
            let lum = |c: Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
            if theme.light {
                assert!(
                    lum(p.face) < lum(theme.panel),
                    "{name}: the others are darker than the ground"
                );
            } else {
                assert!(lum(p.face) > lum(theme.panel), "{name}: the others are a lighter grey");
                assert!(
                    crate::theme::contrast(p.face, p.content) >= 1.3,
                    "{name}: the shown tab stands out from the others ({})",
                    crate::theme::contrast(p.face, p.content)
                );
            }
            assert!(
                crate::theme::contrast(p.face_text, p.face) >= 4.5,
                "{name}: the others' labels read ({})",
                crate::theme::contrast(p.face_text, p.face)
            );
            assert!(
                crate::theme::contrast(p.content_text, p.content) >= 4.5,
                "{name}: the shown label reads"
            );
        }
    }
}
