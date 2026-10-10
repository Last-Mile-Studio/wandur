//! One select for the whole app: a single field surface in the theme's field colour showing the
//! value and a chevron, where a click anywhere on it opens the list. Keyboard: it takes focus
//! with Tab; Enter, Space or an arrow opens the list; the arrows, Home and End move through it;
//! Enter or Space chooses; Escape closes. A screen reader finds a combo box named by its label,
//! with the value as its value, and a named list while it is open.
//!
//! Colours come from the egui visuals [`crate::theme::Theme::visuals`] sets, so the select looks
//! right in every preset, light or dark, and in every skin without being handed a theme.

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align2, Color32, CornerRadius, EventFilter, FontId, Id, Key, Modifiers, Popup, PopupCloseBehavior, Rect, RectAlign,
    Response, Sense, Stroke, Ui, Vec2, pos2, vec2,
};

/// The height of a select (and the directory's other controls) unless set.
pub const HEIGHT: f32 = 36.0;
/// One row of the open list.
const ROW: f32 = 30.0;
/// Space for the chevron at the right.
const CHEVRON: f32 = 28.0;
/// Text inset at the left.
const INSET: f32 = 12.0;

/// A select under construction: `Select::new("id", "Label").width(200.0).show_index(...)`.
pub struct Select<'a> {
    id_salt: Id,
    label: &'a str,
    width: Option<f32>,
    height: f32,
    radius: Option<CornerRadius>,
    font_size: f32,
    prefix: Option<&'a str>,
    dot: Option<Color32>,
    max_list_height: f32,
    open_now: bool,
    placeholder: Option<&'a str>,
}

/// The field's colours, from the visuals.
struct Look {
    fill: Color32,
    hover_fill: Color32,
    border: Color32,
    accent: Color32,
    text: Color32,
    muted: Color32,
}

impl Look {
    fn of(ui: &Ui) -> Look {
        let v = ui.visuals();
        let fill = v.text_edit_bg_color();
        Look {
            fill,
            hover_fill: crate::theme::mix(fill, v.text_color(), 0.05),
            border: v.widgets.inactive.bg_stroke.color,
            accent: v.widgets.hovered.bg_stroke.color,
            text: v.text_color(),
            muted: v.weak_text_color(),
        }
    }
}

/// Paint a field surface (a select, the directory's search box and buttons): the field colour
/// with a one point border, lighter on hover, the accent on hover and while open or focused.
pub fn paint_field(ui: &Ui, rect: Rect, radius: CornerRadius, hovered: bool, active: bool, focused: bool) {
    let look = Look::of(ui);
    let fill = if hovered { look.hover_fill } else { look.fill };
    let stroke = if focused {
        Stroke::new(2.0, look.accent)
    } else if active {
        Stroke::new(1.5, look.accent)
    } else if hovered {
        Stroke::new(1.0, crate::theme::mix(look.border, look.accent, 0.7))
    } else {
        Stroke::new(1.0, look.border)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, radius, fill);
    painter.rect_stroke(rect, radius, stroke, egui::StrokeKind::Inside);
}

/// Text laid out on one row no wider than `width`, ending in an ellipsis when cut.
fn one_row(ui: &Ui, text: &str, size: f32, color: Color32, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::single_section(text.to_string(), TextFormat::simple(FontId::proportional(size), color));
    job.wrap = TextWrapping {
        max_width: width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.fonts_mut(|f| f.layout_job(job))
}

/// The keyboard highlight of an open list, kept between frames.
fn highlight_id(popup: Id) -> Id {
    popup.with("highlight")
}

impl<'a> Select<'a> {
    /// A select named `label` for screen readers.
    pub fn new(id_salt: impl std::hash::Hash + std::fmt::Debug, label: &'a str) -> Self {
        Select {
            id_salt: Id::new(id_salt),
            label,
            width: None,
            height: HEIGHT,
            radius: None,
            font_size: 14.0,
            prefix: None,
            dot: None,
            max_list_height: 320.0,
            open_now: false,
            placeholder: None,
        }
    }

    /// The field's width (the space left in the row otherwise).
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    /// Corner radius (the visuals' widget radius otherwise).
    pub fn radius(mut self, radius: u8) -> Self {
        self.radius = Some(CornerRadius::same(radius));
        self
    }

    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = size;
        self
    }

    /// Muted text before the value inside the field ("Sort").
    pub fn prefix(mut self, prefix: &'a str) -> Self {
        self.prefix = Some(prefix);
        self
    }

    /// A small dot before the value (the directory's online select).
    pub fn dot(mut self, color: Color32) -> Self {
        self.dot = Some(color);
        self
    }

    /// The tallest the open list grows before it scrolls.
    pub fn max_list_height(mut self, height: f32) -> Self {
        self.max_list_height = height;
        self
    }

    /// Muted text shown while no option is chosen (the index is past the options).
    pub fn placeholder(mut self, text: &'a str) -> Self {
        self.placeholder = Some(text);
        self
    }

    /// Open the list this frame (scenes show a select open).
    pub fn open_now(mut self, open: bool) -> Self {
        self.open_now = open;
        self
    }

    /// The width this select needs to show `text` whole (with its prefix and dot).
    pub fn natural_width(&self, ui: &Ui, text: &str) -> f32 {
        let measure = |s: &str| {
            ui.fonts_mut(|f| {
                f.layout_no_wrap(s.to_string(), FontId::proportional(self.font_size), Color32::WHITE)
                    .size()
                    .x
            })
        };
        let mut w = INSET + measure(text) + CHEVRON;
        if let Some(p) = self.prefix {
            w += measure(p) + 6.0;
        }
        if self.dot.is_some() {
            w += 14.0;
        }
        w.ceil()
    }

    /// Show the select over `count` options named by `name`; `selected` is the chosen index.
    /// The response is changed when the user chose another option.
    pub fn show_index<'n>(
        self,
        ui: &mut Ui,
        selected: &mut usize,
        count: usize,
        name: impl Fn(usize) -> &'n str,
    ) -> Response {
        let width = self.width.unwrap_or_else(|| ui.available_width()).max(CHEVRON + INSET);
        let (rect, _) = ui.allocate_exact_size(vec2(width, self.height), Sense::hover());
        let id = ui.make_persistent_id(self.id_salt);
        let response = ui.interact(rect, id, Sense::click());
        self.show_in(ui, rect, response, selected, count, name)
    }

    /// The same at a rectangle the caller laid out (it must lie inside `ui`'s space).
    pub fn show_index_at<'n>(
        self,
        ui: &mut Ui,
        rect: Rect,
        selected: &mut usize,
        count: usize,
        name: impl Fn(usize) -> &'n str,
    ) -> Response {
        let id = ui.make_persistent_id(self.id_salt);
        let response = ui.interact(rect, id, Sense::click());
        self.show_in(ui, rect, response, selected, count, name)
    }

    /// Show the select over `options` (value and label); `value` is changed to the chosen one.
    pub fn show_value<T: PartialEq + Clone, L: AsRef<str>>(
        self,
        ui: &mut Ui,
        value: &mut T,
        options: &[(T, L)],
    ) -> Response {
        let mut index = options.iter().position(|(v, _)| v == value).unwrap_or(0);
        let response = self.show_index(ui, &mut index, options.len(), |i| options[i].1.as_ref());
        if response.changed()
            && let Some((v, _)) = options.get(index)
        {
            *value = v.clone();
        }
        response
    }

    fn show_in<'n>(
        self,
        ui: &mut Ui,
        rect: Rect,
        mut response: Response,
        selected: &mut usize,
        count: usize,
        name: impl Fn(usize) -> &'n str,
    ) -> Response {
        let ctx = ui.ctx().clone();
        let popup_id = response.id.with("popup");
        let was_open = Popup::is_id_open(&ctx, popup_id);
        let enabled = ui.is_enabled() && count > 0;
        let focused = response.has_focus();
        let chosen = *selected < count;
        let current = if chosen {
            name(*selected)
        } else {
            self.placeholder.unwrap_or("")
        };

        // Keep the arrows, Escape and Tab-free navigation for this field while it has the keyboard.
        if focused {
            ctx.memory_mut(|m| {
                m.set_focus_lock_filter(
                    response.id,
                    EventFilter {
                        vertical_arrows: true,
                        horizontal_arrows: false,
                        tab: false,
                        escape: was_open,
                    },
                )
            });
        }
        let mut highlight: usize = ctx
            .data(|d| d.get_temp(highlight_id(popup_id)))
            .unwrap_or(if *selected < count { *selected } else { 0 })
            .min(count.saturating_sub(1));
        let mut open = was_open;
        let mut choose = None;
        let mut moved = false;
        if enabled && response.clicked() {
            // A click (or Enter or Space on the focused field) opens or closes it; while open,
            // Enter and Space choose the highlighted option.
            if was_open {
                if ctx.input(|i| i.key_pressed(Key::Enter) || i.key_pressed(Key::Space)) {
                    choose = Some(highlight);
                }
                open = false;
            } else {
                open = true;
                highlight = if *selected < count { *selected } else { 0 };
            }
            response.request_focus();
        }
        if enabled && focused {
            let (down, up, home, end, escape) = ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::ArrowDown),
                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                    was_open && i.consume_key(Modifiers::NONE, Key::Home),
                    was_open && i.consume_key(Modifiers::NONE, Key::End),
                    was_open && i.consume_key(Modifiers::NONE, Key::Escape),
                )
            });
            if (down || up) && !open {
                open = true;
                highlight = if *selected < count { *selected } else { 0 };
            } else if open {
                if down {
                    highlight = (highlight + 1).min(count - 1);
                    moved = true;
                }
                if up {
                    highlight = highlight.saturating_sub(1);
                    moved = true;
                }
                if home {
                    highlight = 0;
                    moved = true;
                }
                if end {
                    highlight = count - 1;
                    moved = true;
                }
            }
            if escape {
                open = false;
            }
        }

        // The field.
        let radius = self.radius.unwrap_or(ui.visuals().widgets.inactive.corner_radius);
        let look = Look::of(ui);
        if ui.is_rect_visible(rect) {
            let hovered = enabled && response.hovered();
            paint_field(ui, rect, radius, hovered, open, focused && !open);
            if hovered {
                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let text_color = if enabled && chosen { look.text } else { look.muted };
            let mut x = rect.left() + INSET;
            let cy = rect.center().y;
            if let Some(c) = self.dot {
                ui.painter().circle_filled(pos2(x + 4.0, cy), 3.5, c);
                x += 14.0;
            }
            let right = rect.right() - CHEVRON;
            if let Some(p) = self.prefix {
                let g = one_row(ui, p, self.font_size, look.muted, (right - x).max(1.0));
                let w = g.size().x;
                ui.painter().galley(pos2(x, cy - g.size().y / 2.0), g, look.muted);
                x += w + 6.0;
            }
            if right > x + 4.0 {
                let g = one_row(ui, current, self.font_size, text_color, right - x);
                ui.painter().galley(pos2(x, cy - g.size().y / 2.0), g, text_color);
            }
            let c = pos2(rect.right() - CHEVRON / 2.0 - 2.0, cy);
            let d = if open { -2.5 } else { 2.5 };
            ui.painter().add(egui::Shape::line(
                vec![c + vec2(-4.5, -d), c + vec2(0.0, d), c + vec2(4.5, -d)],
                Stroke::new(1.5, if enabled { look.muted } else { look.border }),
            ));
        }
        let label = self.label;
        response.widget_info(|| {
            let mut info = egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, enabled, label);
            info.current_text_value = Some(current.to_string());
            info
        });
        let expanded = open;
        ctx.accesskit_node_builder(response.id, |node| {
            node.set_label(label);
            node.set_value(current);
            node.set_expanded(expanded);
        });

        // The list.
        let list_width = if open && count > 0 {
            let widest = (0..count)
                .map(|i| {
                    ui.fonts_mut(|f| {
                        f.layout_no_wrap(
                            name(i).to_string(),
                            FontId::proportional(self.font_size),
                            Color32::WHITE,
                        )
                        .size()
                        .x
                    })
                })
                .fold(0.0f32, f32::max);
            (widest + 2.0 * INSET + 22.0).clamp(rect.width(), rect.width().max(420.0))
        } else {
            rect.width()
        };
        if self.open_now && !was_open {
            open = true;
        }
        let set = if open != was_open { Some(open) } else { None };
        let frame = egui::Frame::new()
            .fill(ui.visuals().window_fill)
            .stroke(ui.visuals().window_stroke)
            .corner_radius(radius)
            .inner_margin(egui::Margin::same(4))
            .shadow(ui.visuals().popup_shadow);
        let max_h = self.max_list_height;
        let font = self.font_size;
        let current_index = *selected;
        let shown = Popup::from_response(&response)
            .id(popup_id)
            .open_memory(set.map(egui::SetOpenCommand::Bool))
            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
            .align(RectAlign::BOTTOM_START)
            .align_alternatives(&[RectAlign::TOP_START])
            .gap(4.0)
            .width(list_width)
            .frame(frame)
            .show(|ui| {
                ui.set_min_width(list_width - 8.0);
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
                    for i in 0..count {
                        let (row, r) = ui.allocate_exact_size(vec2(list_width - 8.0, ROW), Sense::click());
                        if r.hovered() && ui.input(|inp| inp.pointer.delta() != Vec2::ZERO) {
                            highlight = i;
                        }
                        let lit = i == highlight;
                        if lit {
                            ui.painter()
                                .rect_filled(row, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
                        }
                        if i == current_index {
                            ui.painter().rect_filled(
                                Rect::from_min_size(row.min + vec2(2.0, 7.0), vec2(3.0, ROW - 14.0)),
                                1.5,
                                look.accent,
                            );
                        }
                        let g = one_row(ui, name(i), font, look.text, row.width() - 2.0 * INSET);
                        ui.painter().galley(
                            pos2(row.left() + INSET, row.center().y - g.size().y / 2.0),
                            g,
                            look.text,
                        );
                        r.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::WidgetType::SelectableLabel,
                                true,
                                i == current_index,
                                name(i),
                            )
                        });
                        if lit && moved {
                            r.scroll_to_me(Some(egui::Align::Center));
                        }
                        if r.clicked() {
                            choose = Some(i);
                        }
                    }
                });
            });
        if shown.is_some() {
            crate::a11y::name_shown_popup(&ctx, popup_id, egui::accesskit::Role::ListBox, label);
        }
        if let Some(i) = choose {
            Popup::close_id(&ctx, popup_id);
            response.request_focus();
            if i != *selected && i < count {
                *selected = i;
                response.mark_changed();
            }
        }
        if Popup::is_id_open(&ctx, popup_id) {
            ctx.data_mut(|d| d.insert_temp(highlight_id(popup_id), highlight));
        } else {
            ctx.data_mut(|d| d.remove::<usize>(highlight_id(popup_id)));
        }
        response
    }
}

/// Paint a field-styled button (the directory's Filters and Refresh): the same surface as a
/// select, its text centred; `selected` draws it in the accent (Filters while open).
pub fn field_button(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    text: &str,
    selected: bool,
    enabled: bool,
    radius: u8,
) -> Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let response = ui.interact(rect, id, sense);
    let look = Look::of(ui);
    let radius = CornerRadius::same(radius);
    let hovered = enabled && response.hovered();
    paint_field(ui, rect, radius, hovered, selected, response.has_focus());
    if selected {
        ui.painter()
            .rect_filled(rect.shrink(1.5), radius, look.accent.gamma_multiply(0.14));
    }
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let color = if enabled { look.text } else { look.muted };
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::proportional(14.0),
        color,
    );
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, enabled, selected, text));
    response
}

/// The width a [`field_button`] needs for `text`.
pub fn field_button_width(ui: &Ui, text: &str) -> f32 {
    let w = ui.fonts_mut(|f| {
        f.layout_no_wrap(text.to_string(), FontId::proportional(14.0), Color32::WHITE)
            .size()
            .x
    });
    (w + 2.0 * 16.0).ceil()
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, RawInput};

    const OPTIONS: [&str; 4] = ["Any", "Fantasy", "Science fiction", "Horror"];

    struct Rig {
        ctx: egui::Context,
        value: usize,
        id: Id,
        rect: Rect,
    }

    impl Rig {
        fn new() -> Rig {
            let ctx = egui::Context::default();
            crate::theme::Theme::preset("Slate").apply(&ctx);
            Rig {
                ctx,
                value: 0,
                id: Id::NULL,
                rect: Rect::NOTHING,
            }
        }

        fn frame(&mut self, events: Vec<Event>) {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 500.0))),
                events,
                ..Default::default()
            };
            let mut value = self.value;
            let mut id = self.id;
            let mut rect = self.rect;
            let mut out = self.ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let r = Select::new("genre", "Genre")
                        .width(220.0)
                        .show_index(ui, &mut value, OPTIONS.len(), |i| OPTIONS[i]);
                    id = r.id;
                    rect = r.rect;
                });
            });
            out.textures_delta.clear();
            self.value = value;
            self.id = id;
            self.rect = rect;
        }

        fn open(&self) -> bool {
            Popup::is_id_open(&self.ctx, self.id.with("popup"))
        }

        fn click(&mut self, at: egui::Pos2) {
            self.frame(vec![Event::PointerMoved(at)]);
            let button = |pressed| Event::PointerButton {
                pos: at,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            self.frame(vec![button(true)]);
            self.frame(vec![button(false)]);
            self.frame(vec![]);
        }

        fn key(&mut self, key: Key) {
            self.frame(vec![Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }]);
            self.frame(vec![]);
        }
    }

    #[test]
    fn a_click_anywhere_on_the_field_opens_the_list() {
        for fraction in [0.03, 0.5, 0.97] {
            let mut rig = Rig::new();
            rig.frame(vec![]);
            let at = pos2(rig.rect.left() + rig.rect.width() * fraction, rig.rect.center().y);
            rig.click(at);
            assert!(rig.open(), "a click at {fraction} of the width opens it");
            // A second click on the field closes it again.
            rig.click(at);
            assert!(!rig.open(), "a second click closes it");
        }
    }

    #[test]
    fn clicking_an_option_chooses_it_and_closes() {
        let mut rig = Rig::new();
        rig.frame(vec![]);
        rig.click(rig.rect.center());
        assert!(rig.open());
        // The list opens under the field: rows of 30 points after a 4 point gap and margin.
        let third = pos2(rig.rect.left() + 40.0, rig.rect.bottom() + 4.0 + 4.0 + 2.5 * ROW);
        rig.click(third);
        assert_eq!(rig.value, 2, "Science fiction chosen");
        assert!(!rig.open());
    }

    #[test]
    fn the_keyboard_opens_moves_chooses_and_closes() {
        let mut rig = Rig::new();
        rig.frame(vec![]);
        rig.key(Key::Tab);
        assert!(rig.ctx.memory(|m| m.has_focus(rig.id)), "Tab focuses the select");
        rig.key(Key::Enter);
        assert!(rig.open(), "Enter opens it");
        rig.key(Key::ArrowDown);
        rig.key(Key::ArrowDown);
        rig.key(Key::Enter);
        assert!(!rig.open(), "Enter chooses and closes");
        assert_eq!(rig.value, 2);
        rig.key(Key::Space);
        assert!(rig.open(), "Space opens it");
        rig.key(Key::ArrowUp);
        rig.key(Key::Escape);
        assert!(!rig.open(), "Escape closes it");
        assert_eq!(rig.value, 2, "Escape keeps the value");
        assert!(rig.ctx.memory(|m| m.has_focus(rig.id)), "and the keyboard stays on it");
        rig.key(Key::ArrowDown);
        assert!(rig.open(), "an arrow opens it too");
        rig.key(Key::End);
        rig.key(Key::Space);
        assert_eq!(rig.value, 3, "End then Space chooses the last");
    }

    #[test]
    fn a_screen_reader_finds_a_named_combo_box_with_its_value() {
        let mut rig = Rig::new();
        rig.ctx.enable_accesskit();
        rig.value = 1;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 500.0))),
            ..Default::default()
        };
        let mut value = 1;
        let mut out = rig.ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                Select::new("genre", "Genre").show_index(ui, &mut value, OPTIONS.len(), |i| OPTIONS[i]);
            });
        });
        out.textures_delta.clear();
        let update = out.platform_output.accesskit_update.take().expect("a tree");
        let combo = update
            .nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.role() == egui::accesskit::Role::ComboBox)
            .expect("a combo box");
        assert_eq!(combo.label(), Some("Genre"));
        assert_eq!(combo.value(), Some("Fantasy"));
        assert!(crate::a11y::unnamed(&update).is_empty());
    }
}
