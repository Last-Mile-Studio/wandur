//! Small drawing helpers shared by the directory, the workspace panel and the side panels: pills,
//! text links, clipped text, and artwork plates (a picture cropped to cover a box, or the world's
//! initials while it loads).

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align2, Color32, CornerRadius, FontId, Rect, Response, RichText, Sense, Stroke, Ui, Vec2, pos2, vec2};
use wandur_core::l10n::{S, t};

use crate::artwork::ArtState;
use crate::theme::Theme;

/// A rounded outline pill with small text.
pub fn pill(ui: &mut Ui, text: &str, theme: &Theme, size: f32) -> Response {
    egui::Frame::new()
        .stroke(Stroke::new(1.0, theme.border))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(egui::Margin::symmetric(8, 1))
        .show(ui, |ui| ui.label(RichText::new(text).size(size).color(theme.text)))
        .response
}

/// The green beginner friendly pill.
pub fn beginner(ui: &mut Ui, theme: &Theme, size: f32) -> Response {
    egui::Frame::new()
        .stroke(Stroke::new(1.0, theme.ok.gamma_multiply(0.7)))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(egui::Margin::symmetric(8, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(t(S::BeginnerFriendly)).size(size).color(theme.ok))
        })
        .response
        .on_hover_text(t(S::BeginnerFriendlyHint))
}

/// "143 online" with a live dot after it.
pub fn live(ui: &mut Ui, text: &str, theme: &Theme, size: f32) {
    ui.label(RichText::new(text).size(size).color(theme.text));
    let d = size * 0.55;
    let (rect, _) = ui.allocate_exact_size(vec2(d, d), Sense::hover());
    ui.painter().circle_filled(rect.center(), d * 0.4, theme.ok);
}

/// A small round dot (online selector, status).
pub fn live_dot(ui: &mut Ui, color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, color);
    response
}

/// A text button without a face: the accent "Explore world" link, or a quiet muted one.
pub fn link(ui: &mut Ui, text: &str, color: Color32, size: f32, strong: bool) -> Response {
    let mut rich = RichText::new(text).size(size).color(color);
    if strong {
        rich = rich.strong();
    }
    let response = ui.add(egui::Label::new(rich).sense(Sense::click()));
    if response.hovered() {
        let r = response.rect;
        ui.painter()
            .hline(r.x_range(), r.bottom() + 1.0, Stroke::new(1.0, color));
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

/// Text laid out to `width`, at most `rows` rows, ending in an ellipsis when cut.
pub fn clipped(
    ui: &Ui,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
    rows: usize,
) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::single_section(text.to_string(), TextFormat::simple(font, color));
    job.wrap = TextWrapping {
        max_width: width.max(1.0),
        max_rows: rows,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    ui.fonts_mut(|f| f.layout_job(job))
}

/// The first of `texts` (the full wording first, then shorter ones) that fits in `width` at
/// `sizes[0]` points, then at the next size, and so on. When none fits, the last text at the
/// last size, and `false`: the caller lets it end with an ellipsis and shows the first text as a
/// tooltip.
pub fn fit_text<'a>(ui: &Ui, texts: &[&'a str], sizes: &[f32], width: f32) -> (&'a str, f32, bool) {
    for text in texts {
        for &size in sizes {
            let w = ui.fonts_mut(|f| {
                f.layout_no_wrap(text.to_string(), FontId::proportional(size), Color32::WHITE)
                    .size()
                    .x
            });
            if w <= width {
                return (text, size, true);
            }
        }
    }
    (
        texts.last().copied().unwrap_or(""),
        sizes.last().copied().unwrap_or(12.0),
        false,
    )
}

/// Up to two letters for a world without artwork: the first letter of its first two words.
pub fn initials(name: &str) -> String {
    let letters: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_uppercase)
        .take(2)
        .collect();
    if letters.is_empty() { "?".into() } else { letters }
}

/// The part of a texture that covers `rect` without stretching (centre crop).
pub fn cover_uv(texture: Vec2, rect: Rect) -> Rect {
    let ta = texture.x / texture.y.max(1.0);
    let ra = rect.width() / rect.height().max(1.0);
    if ra < ta {
        let w = ra / ta;
        Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
    } else {
        let h = ta / ra;
        Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
    }
}

/// Paint a plate: the picture covering `rect` when it is ready, else the initials on the plate
/// colour.
pub fn plate(ui: &Ui, rect: Rect, radius: CornerRadius, state: &ArtState, name: &str, theme: &Theme, letters: f32) {
    let painter = ui.painter_at(rect);
    match state {
        ArtState::Ready { texture, size } => {
            let shape = egui::epaint::RectShape::filled(rect, radius, Color32::WHITE)
                .with_texture(*texture, cover_uv(*size, rect));
            painter.add(shape);
        }
        _ => {
            painter.rect_filled(rect, radius, theme.plate);
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                initials(name),
                FontId::proportional(letters),
                theme.border,
            );
        }
    }
}

/// A heading row: bold text.
pub fn heading(ui: &mut Ui, text: &str, size: f32) -> Response {
    ui.label(RichText::new(text).size(size).strong())
}

/// A card frame on the panel colour with a border, as the C# directory's cards.
pub fn card_frame(theme: &Theme) -> egui::Frame {
    egui::Frame::new()
        .fill(theme.panel)
        .stroke(Stroke::new(1.0, theme.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin::symmetric(20, 18))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_follow_the_site() {
        assert_eq!(initials("Lantern & Forest"), "LF");
        assert_eq!(initials("aardwolf"), "A");
        assert_eq!(initials("  "), "?");
        assert_eq!(initials("3 Kingdoms of old"), "3K");
    }

    #[test]
    fn cover_crops_the_long_side() {
        let uv = cover_uv(
            vec2(800.0, 320.0),
            Rect::from_min_size(pos2(0.0, 0.0), vec2(160.0, 160.0)),
        );
        assert!((uv.width() - 0.4).abs() < 1e-4 && (uv.min.x - 0.3).abs() < 1e-4);
        let uv = cover_uv(
            vec2(400.0, 400.0),
            Rect::from_min_size(pos2(0.0, 0.0), vec2(400.0, 100.0)),
        );
        assert!((uv.height() - 0.25).abs() < 1e-4);
    }
}

/// Toolbar icons, drawn with strokes (the UI font has no icon glyphs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// Connect again: a circular arrow.
    Reconnect,
    /// Disconnect: a filled square.
    Stop,
    /// Find: a magnifying glass.
    Search,
    /// Settings: a gear.
    Gear,
    /// Add: a plus.
    Plus,
    /// Delete: a waste bin.
    Trash,
    /// Show or hide a side list: a window with a sidebar.
    Sidebar,
    /// Scripts: three sliders.
    Tune,
    /// Private input: a padlock.
    Lock,
    /// An expander that is closed: a chevron pointing down.
    ChevronDown,
    /// An expander that is open: a chevron pointing up.
    ChevronUp,
    /// Look around: an eye.
    Eye,
    /// Quick commands: four squares.
    Grid,
    /// Copy (and Duplicate): two overlapping sheets.
    Copy,
    /// Script output: a terminal window with a prompt.
    Console,
    /// Help: a question mark in a circle.
    Help,
    /// Map auto-center: a ring with four ticks.
    Crosshair,
    /// Fit the floor: four corner brackets.
    Fit,
    /// Map tools: three lines.
    Menu,
    /// One floor up: an arrow up.
    ArrowUp,
    /// One floor down: an arrow down.
    ArrowDown,
    /// Next match: a chevron pointing right.
    ChevronRight,
    /// Scroll back: a chevron pointing left.
    ChevronLeft,
    /// Run the agent: an outlined triangle pointing right.
    Play,
    /// Save: a floppy disk.
    Save,
    /// Disconnect in the System toolbar: an outlined square (one stroke weight with Play).
    StopOutline,
    /// A panel header's pin (auto hide): a push pin.
    Pin,
    /// A panel header's close: a thin cross.
    Close,
    /// A panel header's options menu: a small chevron pointing down.
    PanelMenu,
    /// Edit the map: a pencil.
    Edit,
    /// Open the full map: a box with an arrow out of its corner.
    Expand,
    /// Play and map side by side: a box split in two columns.
    Columns,
    /// The map editor's Select tool: a pointer arrow.
    Pointer,
    /// The map editor's Add room tool: a square with a plus.
    AddRoom,
    /// The map editor's Connect tool: two rooms joined by a line.
    Connect,
    /// Undo: an arrow curving back to the left.
    Undo,
    /// Redo: an arrow curving on to the right.
    Redo,
    /// Snap to grid: a dotted grid.
    Snap,
    /// The map editor's Add label tool: a frame with a T in it.
    Label,
}

/// A check box square (the C# flyout's): filled with the accent and a white tick when on.
pub fn paint_check(ui: &Ui, rect: Rect, on: bool, enabled: bool, theme: &Theme) {
    let p = ui.painter();
    let fade = |c: Color32| if enabled { c } else { c.gamma_multiply(0.5) };
    if on {
        p.rect_filled(rect, 3.0, fade(theme.accent));
        let c = rect.center();
        p.add(egui::Shape::line(
            vec![
                c + egui::vec2(-4.5, 0.2),
                c + egui::vec2(-1.3, 3.4),
                c + egui::vec2(4.6, -3.6),
            ],
            egui::Stroke::new(1.8, fade(Color32::WHITE)),
        ));
    } else {
        p.rect_filled(rect, 3.0, theme.panel);
        p.rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.2, fade(theme.muted)),
            egui::StrokeKind::Inside,
        );
    }
}

/// Paint `icon` centred in `rect`.
pub fn paint_icon(ui: &Ui, icon: Icon, rect: Rect, color: Color32) {
    paint_icon_with(ui.painter(), ui.visuals().panel_fill, icon, rect, color);
}

/// [`paint_icon`] with a painter (clipped to a strip, say) instead of a `Ui` (the gear's centre
/// hole is left unpainted).
pub fn paint_icon_in(p: &egui::Painter, icon: Icon, rect: Rect, color: Color32) {
    paint_icon_with(p, Color32::TRANSPARENT, icon, rect, color);
}

fn paint_icon_with(p: &egui::Painter, cut: Color32, icon: Icon, rect: Rect, color: Color32) {
    let c = rect.center();
    let stroke = egui::Stroke::new(1.8, color);
    match icon {
        Icon::Reconnect => {
            let r = 7.5;
            let points: Vec<egui::Pos2> = (0..=24)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_4 + i as f32 / 24.0 * std::f32::consts::PI * 1.6;
                    c + egui::vec2(a.cos(), -a.sin()) * r
                })
                .collect();
            let tip = points[0];
            p.add(egui::Shape::line(points, stroke));
            p.line_segment([tip, tip + egui::vec2(-1.0, -5.5)], stroke);
            p.line_segment([tip, tip + egui::vec2(-5.5, 0.5)], stroke);
        }
        Icon::Stop => {
            p.rect_filled(Rect::from_center_size(c, egui::vec2(13.0, 13.0)), 1.0, color);
        }
        Icon::StopOutline => {
            p.rect_stroke(
                Rect::from_center_size(c, egui::vec2(14.0, 14.0)),
                1.0,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        Icon::Search => {
            p.circle_stroke(c + egui::vec2(-2.0, -2.0), 6.0, stroke);
            p.line_segment(
                [c + egui::vec2(2.5, 2.5), c + egui::vec2(7.5, 7.5)],
                egui::Stroke::new(2.2, color),
            );
        }
        Icon::Gear => {
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let dir = egui::vec2(a.cos(), a.sin());
                p.line_segment([c + dir * 5.0, c + dir * 8.5], egui::Stroke::new(3.2, color));
            }
            p.circle_filled(c, 6.2, color);
            p.circle_filled(c, 2.6, cut);
        }
        Icon::Plus => {
            p.line_segment([c + egui::vec2(0.0, -7.0), c + egui::vec2(0.0, 7.0)], stroke);
            p.line_segment([c + egui::vec2(-7.0, 0.0), c + egui::vec2(7.0, 0.0)], stroke);
        }
        Icon::Trash => {
            let s = egui::Stroke::new(1.5, color);
            p.line_segment([c + egui::vec2(-7.0, -5.0), c + egui::vec2(7.0, -5.0)], s);
            p.line_segment([c + egui::vec2(-2.0, -5.0), c + egui::vec2(-2.0, -7.0)], s);
            p.line_segment([c + egui::vec2(-2.0, -7.0), c + egui::vec2(2.0, -7.0)], s);
            p.line_segment([c + egui::vec2(2.0, -7.0), c + egui::vec2(2.0, -5.0)], s);
            let body = [
                c + egui::vec2(-5.0, -5.0),
                c + egui::vec2(-4.0, 7.0),
                c + egui::vec2(4.0, 7.0),
                c + egui::vec2(5.0, -5.0),
            ];
            p.add(egui::Shape::line(body.to_vec(), s));
        }
        Icon::Sidebar => {
            let s = egui::Stroke::new(1.4, color);
            let r = Rect::from_center_size(c, egui::vec2(16.0, 14.0));
            p.rect_stroke(r, 1.5, s, egui::StrokeKind::Inside);
            p.line_segment(
                [
                    egui::pos2(r.left() + 5.0, r.top()),
                    egui::pos2(r.left() + 5.0, r.bottom()),
                ],
                s,
            );
        }
        Icon::Tune => {
            let s = egui::Stroke::new(1.4, color);
            for (i, knob) in [(-5.0f32, -3.0f32), (0.0, 3.0), (5.0, -1.0)] {
                p.line_segment([c + egui::vec2(-7.0, i), c + egui::vec2(7.0, i)], s);
                p.line_segment(
                    [c + egui::vec2(knob, i - 2.5), c + egui::vec2(knob, i + 2.5)],
                    egui::Stroke::new(2.4, color),
                );
            }
        }
        Icon::Lock => {
            p.rect_filled(
                Rect::from_center_size(c + egui::vec2(0.0, 2.5), egui::vec2(11.0, 8.0)),
                1.5,
                color,
            );
            let points: Vec<egui::Pos2> = (0..=12)
                .map(|i| {
                    let a = std::f32::consts::PI * i as f32 / 12.0;
                    c + egui::vec2(-3.5 * a.cos(), -1.5 - 3.5 * a.sin())
                })
                .collect();
            p.add(egui::Shape::line(points, egui::Stroke::new(1.6, color)));
        }
        Icon::Eye => {
            let s = egui::Stroke::new(1.5, color);
            let upper: Vec<egui::Pos2> = (0..=16)
                .map(|i| {
                    let x = -7.5 + 15.0 * i as f32 / 16.0;
                    c + egui::vec2(x, -5.2 * (1.0 - (x / 7.5).powi(2)))
                })
                .collect();
            let lower: Vec<egui::Pos2> = upper.iter().map(|q| egui::pos2(q.x, 2.0 * c.y - q.y)).collect();
            p.add(egui::Shape::line(upper, s));
            p.add(egui::Shape::line(lower, s));
            p.circle_stroke(c, 2.1, s);
        }
        Icon::Grid => {
            for (dx, dy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let center = c + egui::vec2(dx * 3.6, dy * 3.6);
                p.rect_stroke(
                    Rect::from_center_size(center, egui::vec2(5.0, 5.0)),
                    0.5,
                    egui::Stroke::new(1.4, color),
                    egui::StrokeKind::Inside,
                );
            }
        }
        Icon::Copy => {
            let s = egui::Stroke::new(1.4, color);
            p.rect_stroke(
                Rect::from_center_size(c + egui::vec2(1.8, 1.8), egui::vec2(9.0, 9.0)),
                1.0,
                s,
                egui::StrokeKind::Inside,
            );
            p.add(egui::Shape::line(
                vec![
                    c + egui::vec2(-1.8, -6.3),
                    c + egui::vec2(-6.3, -6.3),
                    c + egui::vec2(-6.3, 3.3),
                    c + egui::vec2(-3.0, 3.3),
                ],
                s,
            ));
        }
        Icon::Console => {
            // The C# path "M 2,3 H 14 V 13 H 2 Z M 5,6 L 7,8 L 5,10 M 9,10 H 12" on a 16 grid.
            let s = egui::Stroke::new(1.4, color);
            let at = |x: f32, y: f32| c + egui::vec2(x - 8.0, y - 8.0);
            p.rect_stroke(
                Rect::from_min_max(at(2.0, 3.0), at(14.0, 13.0)),
                1.0,
                s,
                egui::StrokeKind::Middle,
            );
            p.add(egui::Shape::line(vec![at(5.0, 6.0), at(7.0, 8.0), at(5.0, 10.0)], s));
            p.line_segment([at(9.0, 10.0), at(12.0, 10.0)], s);
        }
        Icon::Help => {
            let s = egui::Stroke::new(1.4, color);
            p.circle_stroke(c, 7.0, s);
            p.text(
                c + egui::vec2(0.0, 0.5),
                egui::Align2::CENTER_CENTER,
                "?",
                egui::FontId::proportional(11.0),
                color,
            );
        }
        Icon::Pin | Icon::Close | Icon::PanelMenu => {
            // The C# Fleet header paths (`Fleet.axaml`, 16 units) at 12 points.
            let k = 12.0 / 16.0;
            let at = |x: f32, y: f32| c + egui::vec2((x - 8.0) * k, (y - 8.0) * k);
            let thin = egui::Stroke::new(1.3, color);
            match icon {
                Icon::Pin => {
                    let outline = [
                        (10.0, 1.0),
                        (15.0, 6.0),
                        (13.0, 7.0),
                        (10.0, 10.0),
                        (10.0, 13.0),
                        (3.0, 6.0),
                        (6.0, 6.0),
                        (9.0, 3.0),
                    ];
                    p.add(egui::Shape::closed_line(
                        outline.iter().map(|&(x, y)| at(x, y)).collect(),
                        thin,
                    ));
                    p.line_segment([at(6.5, 9.5), at(1.5, 14.5)], thin);
                }
                Icon::Close => {
                    p.line_segment([at(2.0, 2.0), at(14.0, 14.0)], thin);
                    p.line_segment([at(14.0, 2.0), at(2.0, 14.0)], thin);
                }
                _ => {
                    p.add(egui::Shape::line(
                        vec![at(2.0, 5.0), at(8.0, 11.0), at(14.0, 5.0)],
                        thin,
                    ));
                }
            }
        }
        Icon::Play => {
            // The C# path M 4,2 L 13,8 L 4,14 Z on a 16 point grid.
            let at = |x: f32, y: f32| c + egui::vec2(x - 8.5, y - 8.0) * 1.1;
            p.add(egui::Shape::closed_line(
                vec![at(4.0, 2.0), at(13.0, 8.0), at(4.0, 14.0)],
                egui::Stroke::new(1.4, color),
            ));
        }
        Icon::Save => {
            let at = |x: f32, y: f32| c + egui::vec2(x - 8.0, y - 8.0) * 1.1;
            let s = egui::Stroke::new(1.4, color);
            p.add(egui::Shape::closed_line(
                vec![
                    at(2.0, 2.0),
                    at(11.0, 2.0),
                    at(14.0, 5.0),
                    at(14.0, 14.0),
                    at(2.0, 14.0),
                ],
                s,
            ));
            p.add(egui::Shape::line(
                vec![at(5.0, 2.0), at(5.0, 6.0), at(10.0, 6.0), at(10.0, 2.0)],
                s,
            ));
            p.add(egui::Shape::line(
                vec![at(5.0, 14.0), at(5.0, 9.0), at(11.0, 9.0), at(11.0, 14.0)],
                s,
            ));
        }
        Icon::Crosshair
        | Icon::Fit
        | Icon::Menu
        | Icon::ArrowUp
        | Icon::ArrowDown
        | Icon::ChevronRight
        | Icon::ChevronLeft
        | Icon::Edit
        | Icon::Expand
        | Icon::Columns
        | Icon::Pointer
        | Icon::AddRoom
        | Icon::Connect
        | Icon::Undo
        | Icon::Redo
        | Icon::Snap
        | Icon::Label => {
            // The C# toolbar paths, on a 16 point grid.
            let s = egui::Stroke::new(1.4, color);
            let at = |x: f32, y: f32| c + egui::vec2(x - 8.0, y - 8.0) * 1.1;
            let line = |points: &[(f32, f32)]| {
                p.add(egui::Shape::line(points.iter().map(|&(x, y)| at(x, y)).collect(), s));
            };
            match icon {
                Icon::Crosshair => {
                    for points in [
                        [(8.0, 2.0), (8.0, 5.0)],
                        [(8.0, 11.0), (8.0, 14.0)],
                        [(2.0, 8.0), (5.0, 8.0)],
                        [(11.0, 8.0), (14.0, 8.0)],
                    ] {
                        line(&points);
                    }
                    p.circle_stroke(at(8.0, 8.0), 2.2, s);
                }
                Icon::Fit => {
                    line(&[(1.0, 6.0), (1.0, 1.0), (6.0, 1.0)]);
                    line(&[(10.0, 1.0), (15.0, 1.0), (15.0, 6.0)]);
                    line(&[(15.0, 10.0), (15.0, 15.0), (10.0, 15.0)]);
                    line(&[(6.0, 15.0), (1.0, 15.0), (1.0, 10.0)]);
                }
                Icon::Menu => {
                    for y in [4.0, 8.0, 12.0] {
                        line(&[(2.0, y), (14.0, y)]);
                    }
                }
                Icon::ArrowUp => {
                    line(&[(3.0, 8.0), (8.0, 3.0), (13.0, 8.0)]);
                    line(&[(8.0, 3.0), (8.0, 14.0)]);
                }
                Icon::ArrowDown => {
                    line(&[(3.0, 8.0), (8.0, 13.0), (13.0, 8.0)]);
                    line(&[(8.0, 13.0), (8.0, 2.0)]);
                }
                Icon::Edit => {
                    line(&[
                        (3.0, 13.0),
                        (3.5, 10.0),
                        (11.0, 2.5),
                        (13.5, 5.0),
                        (6.0, 12.5),
                        (3.0, 13.0),
                    ]);
                    line(&[(9.5, 4.0), (12.0, 6.5)]);
                }
                Icon::Expand => {
                    line(&[(7.0, 3.0), (2.0, 3.0), (2.0, 14.0), (13.0, 14.0), (13.0, 9.0)]);
                    line(&[(10.0, 2.0), (14.0, 2.0), (14.0, 6.0)]);
                    line(&[(14.0, 2.0), (8.0, 8.0)]);
                }
                Icon::Columns => {
                    line(&[(2.0, 3.0), (14.0, 3.0), (14.0, 13.0), (2.0, 13.0), (2.0, 3.0)]);
                    line(&[(8.0, 3.0), (8.0, 13.0)]);
                }
                Icon::Pointer => {
                    line(&[
                        (4.0, 2.0),
                        (4.0, 13.0),
                        (7.0, 10.5),
                        (9.5, 15.0),
                        (11.0, 14.2),
                        (8.7, 9.8),
                        (12.5, 9.5),
                        (4.0, 2.0),
                    ]);
                }
                Icon::AddRoom => {
                    line(&[(2.0, 2.0), (12.0, 2.0), (12.0, 7.0)]);
                    line(&[(7.0, 12.0), (2.0, 12.0), (2.0, 2.0)]);
                    line(&[(12.0, 9.5), (12.0, 15.0)]);
                    line(&[(9.3, 12.2), (14.7, 12.2)]);
                }
                Icon::Connect => {
                    line(&[(1.5, 9.5), (6.5, 9.5), (6.5, 14.5), (1.5, 14.5), (1.5, 9.5)]);
                    line(&[(9.5, 1.5), (14.5, 1.5), (14.5, 6.5), (9.5, 6.5), (9.5, 1.5)]);
                    line(&[(6.5, 9.5), (9.5, 6.5)]);
                }
                Icon::Undo | Icon::Redo => {
                    let flip = |x: f32| if icon == Icon::Redo { 16.0 - x } else { x };
                    let arc: Vec<(f32, f32)> = (0..=10)
                        .map(|i| {
                            let a = std::f32::consts::PI * (0.5 - i as f32 / 10.0);
                            (flip(9.0 + 4.25 * a.cos()), 8.75 - 4.25 * a.sin())
                        })
                        .collect();
                    let mut path = vec![(flip(4.0), 4.5)];
                    path.extend(arc);
                    path.push((flip(6.0), 13.0));
                    line(&path);
                    line(&[(flip(6.5), 2.0), (flip(4.0), 4.5), (flip(6.5), 7.0)]);
                }
                Icon::Snap => {
                    for x in [3.0, 8.0, 13.0] {
                        for y in [3.0, 8.0, 13.0] {
                            p.circle_filled(at(x, y), 1.1, color);
                        }
                    }
                    line(&[(3.0, 13.0), (13.0, 3.0)]);
                }
                Icon::Label => {
                    line(&[(1.5, 3.0), (14.5, 3.0), (14.5, 13.0), (1.5, 13.0), (1.5, 3.0)]);
                    line(&[(5.0, 6.0), (11.0, 6.0)]);
                    line(&[(8.0, 6.0), (8.0, 10.5)]);
                }
                Icon::ChevronLeft => line(&[(11.0, 3.0), (6.0, 8.0), (11.0, 13.0)]),
                _ => line(&[(5.0, 3.0), (10.0, 8.0), (5.0, 13.0)]),
            }
        }
        Icon::ChevronDown | Icon::ChevronUp => {
            let dy = if icon == Icon::ChevronDown { 2.5 } else { -2.5 };
            let s = egui::Stroke::new(1.4, color);
            p.add(egui::Shape::line(
                vec![
                    c + egui::vec2(-5.0, -dy),
                    c + egui::vec2(0.0, dy),
                    c + egui::vec2(5.0, -dy),
                ],
                s,
            ));
        }
    }
}

/// A flat toolbar button: an icon, then optional text, highlighted on hover.
pub fn tool_button(ui: &mut Ui, icon: Icon, text: Option<&str>, theme: &Theme, enabled: bool) -> Response {
    let font = egui::FontId::proportional(14.0);
    let galley = text.map(|t| ui.painter().layout_no_wrap(t.to_string(), font, theme.text));
    let width = 30.0 + galley.as_ref().map_or(0.0, |g| g.size().x + 8.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 30.0), sense);
    // A short fade on hover (100 ms, once per change, never continuous; C# UI review, item 19).
    let hover = ui
        .ctx()
        .animate_bool_with_time(response.id.with("hover"), enabled && response.hovered(), 0.1);
    if hover > 0.0 {
        ui.painter()
            .rect_filled(rect, 4.0, theme.hover_fill().gamma_multiply(hover));
    }
    let color = if enabled { theme.text } else { theme.disabled_text() };
    let icon_rect = Rect::from_min_size(rect.min, egui::vec2(30.0, 30.0));
    paint_icon(ui, icon, icon_rect, color);
    if let Some(galley) = galley {
        let pos = egui::pos2(icon_rect.right(), rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(pos, galley, color);
    }
    let label = text.unwrap_or_else(|| {
        t(match icon {
            Icon::Reconnect => S::ConnectToSelectedWorld,
            Icon::Stop | Icon::StopOutline => S::Disconnect2,
            Icon::Search => S::FindAMUD,
            Icon::Gear => S::SettingsTitle,
            Icon::Plus => S::MacroNew,
            Icon::Trash => S::ScriptDelete,
            Icon::Sidebar => S::ProfileToggleSections,
            Icon::Tune => S::ScriptsButton,
            Icon::Lock => S::PrivateInput2,
            Icon::ChevronDown | Icon::ChevronUp => S::CustomLoginPrompts,
            Icon::Eye => S::LookAround,
            Icon::Grid => S::Controls,
            Icon::Copy => S::ConsoleCopy,
            Icon::Console => S::ScriptOutputLabel,
            Icon::Help => S::ScriptHelpTitle,
            Icon::Crosshair => S::MapAutoCenter,
            Icon::Fit => S::MapFitFloor,
            Icon::Menu => S::MapToolsToggle,
            Icon::ArrowUp => S::MapFloorUp,
            Icon::ArrowDown => S::MapFloorDown,
            Icon::ChevronRight => S::MapRoomSearchNext,
            Icon::ChevronLeft => S::SessionTabsScrollLeft,
            Icon::Play => S::AgentPlay,
            Icon::Save => S::AgentSaveSettings,
            Icon::Pin => S::AutoHide,
            Icon::Close => S::DockClosePanel,
            Icon::PanelMenu => S::DockPanelMenu,
            Icon::Edit => S::MapEditMode,
            Icon::Expand => S::MapOpenFull,
            Icon::Columns => S::SessionSplitView,
            Icon::Pointer => S::MapToolSelect,
            Icon::AddRoom => S::MapAddRoom,
            Icon::Connect => S::MapToolConnect,
            Icon::Undo => S::Undo,
            Icon::Redo => S::Redo,
            Icon::Snap => S::MapToolSnap,
            Icon::Label => S::MapToolLabel,
        })
    });
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    response
}

/// A bordered box with a header that opens and closes it (the C# `Expander`): the title on the
/// left, a chevron on the right, the body under a line while open. Returns the header's
/// response.
pub fn expander(
    ui: &mut Ui,
    open: &mut bool,
    title: &str,
    width: f32,
    theme: &Theme,
    body: impl FnOnce(&mut Ui),
) -> Response {
    let mut header_response = None;
    let fill = ui.visuals().extreme_bg_color;
    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1.0, theme.border))
        .corner_radius(4)
        .show(ui, |ui| {
            ui.set_width(width - 2.0);
            let (rect, header) = ui.allocate_exact_size(egui::vec2(width - 2.0, 46.0), egui::Sense::click());
            ui.painter().text(
                egui::pos2(rect.left() + 16.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                title,
                egui::FontId::proportional(14.0),
                theme.text,
            );
            let chevron =
                Rect::from_center_size(egui::pos2(rect.right() - 24.0, rect.center().y), egui::vec2(12.0, 12.0));
            paint_icon(
                ui,
                if *open { Icon::ChevronUp } else { Icon::ChevronDown },
                chevron,
                theme.muted,
            );
            header.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::CollapsingHeader, true, *open, title));
            if header.clicked() {
                *open = !*open;
            }
            if *open {
                ui.painter()
                    .hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1.0, theme.border));
                egui::Frame::new().inner_margin(egui::Margin::same(16)).show(ui, |ui| {
                    ui.set_width(width - 34.0);
                    body(ui);
                });
            }
            header_response = Some(header);
        });
    header_response.expect("the header was drawn")
}

/// Give a widget drawn without a text label (an icon, a ×) its accessible name, the name a
/// screen reader announces.
pub fn name(response: &egui::Response, typ: egui::WidgetType, label: &str) {
    let enabled = response.enabled();
    response.widget_info(|| egui::WidgetInfo::labeled(typ, enabled, label));
}
