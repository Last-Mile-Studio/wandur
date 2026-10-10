//! A large dialog in a window of its own, as the C# client's dialogs are (the world editor's
//! `ProfileDialog`, the settings' `OptionsDialog`): it can be moved anywhere and resized, and
//! its content reflows.
//!
//! On the desktop it is a second native window (an egui immediate viewport) with the C# title,
//! default and minimum size. It opens centred over the main window the first time and where it
//! was left, at the size it was left, after that (for the run). The main window stays dimmed
//! under a card that brings the dialog back to the front, so the dialog stays modal.
//!
//! Where viewports are embedded (the headless scenes and tests, or a platform without a second
//! window) it falls back to a movable, resizable window inside the main one, over the dimmed
//! window, which is the modal layer: nothing under it takes input meanwhile.

use egui::{Id, Pos2, RichText, Ui, Vec2};
use wandur_core::l10n::{S, t, tf};

use crate::theme::Theme;

/// What a dialog window is.
#[derive(Clone, Copy, Debug)]
pub struct Spec<'a> {
    /// Its id (also its viewport's).
    pub id: &'a str,
    pub title: &'a str,
    pub default_size: Vec2,
    pub min_size: Vec2,
}

/// How the person asked to close the dialog this frame, if they did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Close {
    /// Still open.
    No,
    /// The window's close button.
    Button,
    /// Escape, with no popup open.
    Escape,
}

impl Close {
    pub fn requested(self) -> bool {
        self != Close::No
    }
}

/// Where the native window opens: its outer position (or the system's choice) and inner size.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Geometry {
    position: Option<Pos2>,
    size: Vec2,
}

/// The viewport of the dialog `id`.
pub fn viewport_id(id: &str) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("dialog-window", id))
}

/// Whether the dialog's own window has the keyboard (its content is not in the main window's
/// AccessKit tree then).
pub fn is_native(ctx: &egui::Context) -> bool {
    !ctx.embed_viewports()
}

/// Draw the dialog for this frame with `add` filling it (the whole inner rect, no margin).
pub fn show(ctx: &egui::Context, spec: Spec<'_>, theme: &Theme, add: impl FnOnce(&mut Ui)) -> Close {
    if ctx.embed_viewports() {
        embedded(ctx, spec, theme, add)
    } else {
        native(ctx, spec, theme, add)
    }
}

/// The dialog was not drawn last frame: it is opening now.
fn opening(ctx: &egui::Context, id: &str) -> bool {
    let key = Id::new(("dialog-window-last", id));
    let pass = ctx.cumulative_pass_nr();
    let previous = ctx.data_mut(|d| {
        let previous = d.get_temp::<u64>(key);
        d.insert_temp(key, pass);
        previous
    });
    previous.is_none_or(|p| p + 1 < pass)
}

fn embedded(ctx: &egui::Context, spec: Spec<'_>, theme: &Theme, add: impl FnOnce(&mut Ui)) -> Close {
    let screen = ctx.content_rect();
    let room = (screen.size() - egui::vec2(24.0, 24.0)).max(egui::vec2(200.0, 160.0));
    // Under the window, over everything else: the dimmed main window.
    ctx.layer_painter(egui::LayerId::new(
        egui::Order::Middle,
        Id::new(("dialog-backdrop", spec.id)),
    ))
    .rect_filled(screen, 0.0, theme.dim());
    let mut open = true;
    let shown = egui::Window::new(RichText::new(spec.title).size(13.0).color(theme.text))
        .id(Id::new(("dialog-window", spec.id)))
        .order(egui::Order::Foreground)
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_size(spec.default_size.min(room))
        .min_size(spec.min_size.min(room))
        // Centred by its top left corner, so the corner under the pointer follows a resize.
        .default_pos(screen.center() - spec.default_size.min(room) / 2.0)
        .constrain(true)
        .frame(
            egui::Frame::window(&ctx.global_style())
                .fill(theme.panel)
                .stroke(egui::Stroke::new(1.0, theme.border))
                .corner_radius(6)
                .inner_margin(egui::Margin::ZERO),
        )
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
            let mut content = ui.new_child(egui::UiBuilder::new().max_rect(rect));
            content.set_clip_rect(rect.intersect(ui.clip_rect()));
            add(&mut content);
        });
    name_title_bar(ctx, Id::new(("dialog-window", spec.id)), spec.title);
    if let Some(shown) = &shown {
        // The dialog is the modal layer: the main window under it takes no input.
        ctx.memory_mut(|m| m.set_modal_layer(shown.response.layer_id));
    }
    if !open {
        Close::Button
    } else if escape(ctx) {
        Close::Escape
    } else {
        Close::No
    }
}

/// An egui window's title bar (the strip that moves it) gets the window's title as its name.
pub fn name_title_bar(ctx: &egui::Context, window: Id, title: &str) {
    ctx.accesskit_node_builder(window.with("__title_click"), |node| node.set_label(title));
}

/// Escape closes the dialog unless a popup (a combo box's list) is open: that takes it first.
fn escape(ctx: &egui::Context) -> bool {
    !egui::Popup::is_any_open(ctx) && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
}

fn native(ctx: &egui::Context, spec: Spec<'_>, theme: &Theme, add: impl FnOnce(&mut Ui)) -> Close {
    let viewport = viewport_id(spec.id);
    let remembered = Id::new(("dialog-window-geometry", spec.id));
    let start_key = Id::new(("dialog-window-start", spec.id));
    if opening(ctx, spec.id) || ctx.data(|d| d.get_temp::<Geometry>(start_key)).is_none() {
        // Where it was left this run, else centred over the main window at the C# size.
        let start = ctx.data(|d| d.get_temp::<Geometry>(remembered)).unwrap_or_else(|| {
            let (outer, monitor) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().monitor_size));
            let size = match monitor {
                Some(m) => spec.default_size.min(m - egui::vec2(80.0, 120.0)).max(spec.min_size),
                None => spec.default_size,
            };
            Geometry {
                position: outer.map(|o| o.center() - size / 2.0),
                size,
            }
        });
        ctx.data_mut(|d| d.insert_temp(start_key, start));
        ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Focus);
    }
    let start: Geometry = ctx.data(|d| d.get_temp(start_key)).unwrap_or(Geometry {
        position: None,
        size: spec.default_size,
    });
    // The main window: dimmed, with a way back to the dialog should it go behind.
    blocker(ctx, viewport, spec.title, theme);
    // The builder stays the same while the dialog is open (a changed one would move or resize
    // the window back).
    let mut builder = egui::ViewportBuilder::default()
        .with_title(spec.title)
        .with_inner_size(start.size)
        .with_min_inner_size(spec.min_size);
    if let Some(position) = start.position {
        builder = builder.with_position(position);
    }
    let mut add = Some(add);
    ctx.show_viewport_immediate(viewport, builder, |ui, _class| {
        let ctx = ui.ctx().clone();
        let mut close = if ctx.input(|i| i.viewport().close_requested()) {
            Close::Button
        } else {
            Close::No
        };
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme.panel))
            .show(ui, |ui| {
                if let Some(add) = add.take() {
                    add(ui);
                }
            });
        if close == Close::No && escape(&ctx) {
            close = Close::Escape;
        }
        let (outer, inner) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
        if let Some(inner) = inner {
            ctx.data_mut(|d| {
                d.insert_temp(
                    remembered,
                    Geometry {
                        position: outer.map(|o| o.min),
                        size: inner.size(),
                    },
                )
            });
        }
        close
    })
}

/// The main window while a dialog has its own: dimmed, with a card that brings it back.
fn blocker(ctx: &egui::Context, viewport: egui::ViewportId, title: &str, theme: &Theme) {
    crate::dialogs::modal(ctx, "dialog-window-elsewhere", 320.0, theme, |ui| {
        ui.add(
            egui::Label::new(
                RichText::new(tf(S::DialogOpenElsewhere, &[&title]))
                    .size(14.0)
                    .color(theme.text),
            )
            .wrap(),
        );
        ui.add_space(10.0);
        if crate::dialogs::primary_button(ui, t(S::DialogShow), theme).clicked() {
            ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::Focus);
        }
    });
}
