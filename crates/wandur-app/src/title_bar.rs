//! The title bar's controls (the C# `MainWindow.TitleActions`, `SkinMenuButton`,
//! `ThemeMenuButton`): the Skin, palette and full screen buttons, their menus, the window's own
//! caption buttons on Windows and Linux (macOS keeps its native traffic lights), dragging the
//! window by its band, and the icon and title on the plate. The band itself is painted by
//! [`crate::skin`].

use egui::{Align2, Color32, FontId, Pos2, Rect, Response, RichText, Sense, Stroke, TextureHandle, Ui, pos2, vec2};
use wandur_core::l10n::{S, t};
use wandur_core::settings::Settings;

use crate::skin::{Chrome, SkinId, TITLE_ACTIONS_WIDTH, TitleBarMetrics};
use crate::theme::{Theme, mix, parse_hex};

/// Which title menu is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleMenu {
    Skin,
    Theme,
}

/// What the title bar asks of the app.
#[derive(Clone, Debug, PartialEq)]
pub enum TitleAction {
    Skin(SkinId),
    /// Choose a colour scheme (a preset name or a custom theme id); it stops following worlds.
    Theme(String),
    ToggleWorldThemes,
    FullScreen,
    Minimize,
    Maximize,
    Close,
}

/// The app icon, for the plate and the System toolbar.
pub fn logo(ctx: &egui::Context) -> Option<TextureHandle> {
    let id = egui::Id::new("wandur-title-logo");
    if let Some(handle) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return Some(handle);
    }
    let bytes: &[u8] = include_bytes!("../assets/skins/app-icon-64.png");
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
    let rgba = image.to_rgba8();
    let color =
        egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
    let handle = ctx.load_texture("wandur-logo", color, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, handle.clone()));
    Some(handle)
}

/// The plate's title font: 13 points with a letter's extra spacing.
pub fn title_job(text: &str, metrics: &TitleBarMetrics, color: Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: FontId::proportional(metrics.title_font_size),
            extra_letter_spacing: metrics.title_letter_spacing,
            color,
            ..Default::default()
        },
    );
    job
}

/// How wide `text` is in the plate's font.
pub fn title_width(ctx: &egui::Context, text: &str, metrics: &TitleBarMetrics) -> f32 {
    ctx.fonts_mut(|f| f.layout_job(title_job(text, metrics, Color32::WHITE)).size().x)
}

/// The icon and title, centred in `area` (trimmed at the end when they do not fit).
pub fn paint_identity(ui: &Ui, area: Rect, text: &str, metrics: &TitleBarMetrics, color: Color32) {
    let logo_size = metrics.logo_size;
    let mut job = title_job(text, metrics, color);
    job.wrap.max_width = (area.width() - logo_size - crate::skin::TITLE_LOGO_GAP).max(0.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let width = logo_size + crate::skin::TITLE_LOGO_GAP + galley.size().x;
    let left = area.center().x - width / 2.0;
    if let Some(logo) = logo(ui.ctx()) {
        let rect = Rect::from_min_size(
            pos2(left, area.center().y - logo_size / 2.0),
            vec2(logo_size, logo_size),
        );
        ui.painter().image(
            logo.id(),
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    let pos = pos2(
        left + logo_size + crate::skin::TITLE_LOGO_GAP,
        area.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(pos, galley, color);
}

/// A title bar button: a glyph that lights on hover.
fn glyph_button(
    ui: &mut Ui,
    rect: Rect,
    label: &str,
    hover: Color32,
    draw: impl FnOnce(&egui::Painter, Rect),
) -> Response {
    let response = ui.interact(rect, ui.id().with(("title-button", label)), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, hover);
    }
    draw(ui.painter(), rect);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response.on_hover_text(label)
}

/// The Skin, palette and full screen buttons in a row of `TITLE_ACTIONS_WIDTH * scale` ending at
/// `right`, centred on `center_y`. Opens and draws their menus.
#[allow(clippy::too_many_arguments)]
pub fn actions(
    ui: &mut Ui,
    right: f32,
    center_y: f32,
    scale: f32,
    theme: &Theme,
    chrome: &Chrome,
    settings: &Settings,
    follows_world: bool,
    menu: &mut Option<TitleMenu>,
    full_screen: bool,
) -> Vec<TitleAction> {
    let mut out = Vec::new();
    let s = scale;
    let size = 30.0 * s;
    let spacing = 4.0 * s;
    // Skin (36 wide), palette (44 wide), full screen (30 wide), right to left.
    let full_rect = Rect::from_center_size(pos2(right - size / 2.0, center_y), vec2(size, size));
    let theme_rect = Rect::from_center_size(
        pos2(full_rect.left() - spacing - 22.0 * s, center_y),
        vec2(44.0 * s, size),
    );
    let skin_rect = Rect::from_center_size(
        pos2(theme_rect.left() - spacing - 18.0 * s, center_y),
        vec2(36.0 * s, size),
    );
    let ink = chrome.icon;
    let hover = mix(chrome.band.middle(), theme.text, 0.10);
    let stroke = Stroke::new(1.4 * s.max(0.8), ink);
    let skin = glyph_button(ui, skin_rect, t(S::Skin), hover, |p, r| {
        let g = Rect::from_center_size(r.center(), vec2(16.0, 16.0) * s);
        p.rect_stroke(g, 0.0, stroke, egui::StrokeKind::Middle);
        p.line_segment(
            [pos2(g.left(), g.top() + 4.0 * s), pos2(g.right(), g.top() + 4.0 * s)],
            stroke,
        );
        p.line_segment(
            [
                pos2(g.left() + 4.0 * s, g.top() + 4.0 * s),
                pos2(g.left() + 4.0 * s, g.bottom()),
            ],
            stroke,
        );
    });
    let palette = glyph_button(ui, theme_rect, t(S::Theme), hover, |p, r| {
        let c = pos2(r.left() + 6.0 * s + 9.0 * s, r.center().y);
        paint_palette(p, c, 8.0 * s, stroke);
        let chev = pos2(c.x + 13.0 * s, r.center().y);
        p.add(egui::Shape::line(
            vec![
                chev + vec2(-3.0, -1.5) * s,
                chev + vec2(0.0, 1.5) * s,
                chev + vec2(3.0, -1.5) * s,
            ],
            stroke,
        ));
    });
    let label = if full_screen {
        t(S::ExitFullScreen)
    } else {
        t(S::FullScreen)
    };
    let full = glyph_button(ui, full_rect, label, hover, |p, r| {
        let g = Rect::from_center_size(r.center(), vec2(14.0, 14.0) * s);
        let k = 5.0 * s;
        for (corner, dx, dy) in [
            (g.left_top(), 1.0, 1.0),
            (g.right_top(), -1.0, 1.0),
            (g.right_bottom(), -1.0, -1.0),
            (g.left_bottom(), 1.0, -1.0),
        ] {
            p.add(egui::Shape::line(
                vec![corner + vec2(0.0, dy * k), corner, corner + vec2(dx * k, 0.0)],
                stroke,
            ));
        }
    });
    if skin.clicked() {
        *menu = if *menu == Some(TitleMenu::Skin) {
            None
        } else {
            Some(TitleMenu::Skin)
        };
    }
    if palette.clicked() {
        *menu = if *menu == Some(TitleMenu::Theme) {
            None
        } else {
            Some(TitleMenu::Theme)
        };
    }
    if full.clicked() {
        *menu = None;
        out.push(TitleAction::FullScreen);
    }
    match *menu {
        Some(TitleMenu::Skin) => {
            if let Some(action) = skin_menu(&skin, theme, settings, menu) {
                out.push(action);
            }
        }
        Some(TitleMenu::Theme) => {
            if let Some(action) = theme_menu(&palette, theme, settings, follows_world, menu) {
                out.push(action);
            }
        }
        None => {}
    }
    out
}

/// The full width the buttons take at `scale`.
pub fn actions_width(scale: f32) -> f32 {
    TITLE_ACTIONS_WIDTH * scale
}

/// An outline of a painter's palette with three wells.
fn paint_palette(p: &egui::Painter, c: Pos2, r: f32, stroke: Stroke) {
    let points: Vec<Pos2> = (0..=28)
        .map(|i| {
            let a = std::f32::consts::FRAC_PI_4 + i as f32 / 28.0 * std::f32::consts::PI * 1.7;
            c + vec2(a.cos(), -a.sin()) * r
        })
        .collect();
    p.add(egui::Shape::line(points, stroke));
    for (dx, dy) in [(-4.0, -1.0), (-1.0, -4.5), (3.5, -3.5)] {
        p.circle_filled(c + vec2(dx, dy) * (r / 8.0), 1.0 * (r / 8.0), stroke.color);
    }
}

fn menu_frame(theme: &Theme) -> egui::Frame {
    egui::Frame::new()
        .fill(theme.menu_fill())
        .stroke(Stroke::new(1.0, theme.border))
        .corner_radius(4)
        .shadow(egui::Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(48),
        })
        .inner_margin(egui::Margin::symmetric(0, 4))
}

/// A checkable menu row; `swatches` are drawn before the label.
fn check_row(ui: &mut Ui, theme: &Theme, label: &str, checked: bool, swatches: &[Color32]) -> bool {
    let width = ui.available_width().max(132.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect.shrink2(vec2(4.0, 1.0)), 3.0, theme.hover_fill());
    }
    let p = ui.painter();
    if checked {
        let c = pos2(rect.left() + 20.0, rect.center().y);
        p.add(egui::Shape::line(
            vec![c + vec2(-5.0, 0.0), c + vec2(-1.5, 3.5), c + vec2(5.5, -4.0)],
            Stroke::new(1.6, theme.text),
        ));
    }
    let mut x = rect.left() + 40.0;
    for color in swatches {
        let swatch = Rect::from_min_size(pos2(x, rect.center().y - 6.0), vec2(12.0, 12.0));
        p.rect(
            swatch,
            2.0,
            *color,
            Stroke::new(1.0, theme.border),
            egui::StrokeKind::Inside,
        );
        x += 18.0;
    }
    p.text(
        pos2(x, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.0),
        theme.text,
    );
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, checked, label));
    response.clicked()
}

fn separator(ui: &mut Ui, theme: &Theme) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
    ui.painter().hline(
        rect.left() + 12.0..=rect.right() - 12.0,
        rect.center().y,
        Stroke::new(1.0, theme.border),
    );
}

fn popup(button: &Response, id: &str, theme: &Theme, menu: &mut Option<TitleMenu>, body: impl FnOnce(&mut Ui)) {
    let mut open = true;
    egui::Popup::from_response(button)
        .id(egui::Id::new(id))
        .open_bool(&mut open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::BOTTOM_END)
        .gap(2.0)
        .frame(menu_frame(theme))
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            body(ui);
        });
    let name = if id == "title-skin-menu" {
        t(S::Skin)
    } else {
        t(S::ColorScheme)
    };
    crate::a11y::name_shown_popup(&button.ctx, egui::Id::new(id), egui::accesskit::Role::Menu, name);
    if !open {
        *menu = None;
    }
}

/// Fleet, Armored and System, the current one checked.
fn skin_menu(
    button: &Response,
    theme: &Theme,
    settings: &Settings,
    menu: &mut Option<TitleMenu>,
) -> Option<TitleAction> {
    let current = SkinId::from_name(&settings.skin);
    let mut chosen = None;
    popup(button, "title-skin-menu", theme, menu, |ui| {
        ui.set_width(132.0);
        for id in SkinId::ALL {
            if check_row(ui, theme, id.label(), id == current, &[]) {
                chosen = Some(TitleAction::Skin(id));
            }
        }
    });
    if chosen.is_some() {
        *menu = None;
    }
    chosen
}

/// Every preset with its transcript and accent swatches, the custom themes, and Follow MUD
/// theme. A theme is checked only while no world's theme is on screen.
fn theme_menu(
    button: &Response,
    theme: &Theme,
    settings: &Settings,
    follows_world: bool,
    menu: &mut Option<TitleMenu>,
) -> Option<TitleAction> {
    let mut chosen = None;
    popup(button, "title-theme-menu", theme, menu, |ui| {
        ui.set_width(220.0);
        for name in Theme::names() {
            let preset = Theme::preset(name);
            let checked = !follows_world && settings.theme == name;
            if check_row(
                ui,
                theme,
                crate::theme::preset_label(name),
                checked,
                &[preset.terminal, preset.accent],
            ) {
                chosen = Some(TitleAction::Theme(name.into()));
            }
        }
        if !settings.custom_themes.is_empty() {
            separator(ui, theme);
            for custom in &settings.custom_themes {
                let resolved = Theme::from_custom(custom);
                let checked = !follows_world && settings.theme == custom.id;
                let accent = custom.color("Accent").and_then(parse_hex).unwrap_or(resolved.accent);
                if check_row(ui, theme, &custom.name, checked, &[resolved.terminal, accent]) {
                    chosen = Some(TitleAction::Theme(custom.id.clone()));
                }
            }
        }
        separator(ui, theme);
        if check_row(ui, theme, t(S::FollowMudTheme), settings.use_world_themes, &[]) {
            chosen = Some(TitleAction::ToggleWorldThemes);
        }
    });
    if chosen.is_some() {
        *menu = None;
    }
    chosen
}

/// Minimize, maximize and close, drawn at the band's right end on Windows and Linux.
pub fn caption_buttons(
    ui: &mut Ui,
    area: Rect,
    theme: &Theme,
    chrome: &Chrome,
    maximized: bool,
) -> Option<TitleAction> {
    let w = area.width() / 3.0;
    let ink = chrome.icon;
    let stroke = Stroke::new(1.2, ink);
    let mut out = None;
    let maximize_label = if maximized {
        t(S::WindowRestoreDown)
    } else {
        t(S::WindowMaximize)
    };
    for (i, (label, action)) in [
        (t(S::Minimize), TitleAction::Minimize),
        (maximize_label, TitleAction::Maximize),
        (t(S::WindowClose), TitleAction::Close),
    ]
    .into_iter()
    .enumerate()
    {
        let rect = Rect::from_min_size(pos2(area.left() + w * i as f32, area.top()), vec2(w, area.height()));
        let close = action == TitleAction::Close;
        let hover = if close {
            Color32::from_rgb(0xC4, 0x2B, 0x1C)
        } else {
            mix(chrome.band.middle(), theme.text, 0.12)
        };
        let response = ui.interact(rect, ui.id().with(("caption", i)), Sense::click());
        if response.hovered() {
            ui.painter().rect_filled(rect, 0.0, hover);
        }
        let ink = if close && response.hovered() {
            Color32::WHITE
        } else {
            ink
        };
        let stroke = Stroke { color: ink, ..stroke };
        let c = rect.center();
        let p = ui.painter();
        match action {
            TitleAction::Minimize => {
                p.line_segment([c + vec2(-5.0, 0.0), c + vec2(5.0, 0.0)], stroke);
            }
            TitleAction::Maximize => {
                if maximized {
                    p.rect_stroke(
                        Rect::from_center_size(c + vec2(-1.0, 1.0), vec2(8.0, 8.0)),
                        0.0,
                        stroke,
                        egui::StrokeKind::Middle,
                    );
                    p.add(egui::Shape::line(
                        vec![
                            c + vec2(-2.0, -3.0),
                            c + vec2(-2.0, -5.0),
                            c + vec2(5.0, -5.0),
                            c + vec2(5.0, 2.0),
                            c + vec2(3.0, 2.0),
                        ],
                        stroke,
                    ));
                } else {
                    p.rect_stroke(
                        Rect::from_center_size(c, vec2(10.0, 10.0)),
                        0.0,
                        stroke,
                        egui::StrokeKind::Middle,
                    );
                }
            }
            _ => {
                p.line_segment([c + vec2(-5.0, -5.0), c + vec2(5.0, 5.0)], stroke);
                p.line_segment([c + vec2(5.0, -5.0), c + vec2(-5.0, 5.0)], stroke);
            }
        }
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
        if response.on_hover_text(label).clicked() {
            out = Some(action);
        }
    }
    out
}

/// Dragging the band moves the window; a double click maximizes or restores it. Call before the
/// band's buttons so they take their own clicks.
pub fn drag_area(ui: &mut Ui, rect: Rect) {
    let response = ui.interact(rect, ui.id().with("title-drag"), Sense::click_and_drag());
    crate::a11y::control(&response, egui::accesskit::Role::TitleBar, t(S::A11yTitleBar));
    if response.drag_started_by(egui::PointerButton::Primary) {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if response.double_clicked() {
        let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    }
}

/// Resize handles along an undecorated window's edges (Windows and Linux with a drawn skin).
pub fn resize_edges(ctx: &egui::Context) {
    use egui::{CursorIcon, ResizeDirection as D};
    let screen = ctx.content_rect();
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let grip = 5.0;
    let (l, r, t, b) = (
        pos.x - screen.left() <= grip,
        screen.right() - pos.x <= grip,
        pos.y - screen.top() <= grip,
        screen.bottom() - pos.y <= grip,
    );
    let direction = match (l, r, t, b) {
        (true, _, true, _) => D::NorthWest,
        (_, true, true, _) => D::NorthEast,
        (true, _, _, true) => D::SouthWest,
        (_, true, _, true) => D::SouthEast,
        (true, ..) => D::West,
        (_, true, ..) => D::East,
        (_, _, true, _) => D::North,
        (.., true) => D::South,
        _ => return,
    };
    let icon = match direction {
        D::North | D::South => CursorIcon::ResizeVertical,
        D::East | D::West => CursorIcon::ResizeHorizontal,
        D::NorthWest | D::SouthEast => CursorIcon::ResizeNwSe,
        D::NorthEast | D::SouthWest => CursorIcon::ResizeNeSw,
    };
    ctx.set_cursor_icon(icon);
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
    }
}

/// The System skin's identity at the toolbar's left: the icon and the title, trimmed to 260.
pub fn system_identity(ui: &mut Ui, text: &str, theme: &Theme) {
    if let Some(logo) = logo(ui.ctx()) {
        ui.add(egui::Image::new((logo.id(), vec2(26.0, 26.0))));
        ui.add_space(9.0 - ui.spacing().item_spacing.x);
    }
    ui.add(
        egui::Label::new(RichText::new(text).size(13.0).color(theme.text))
            .truncate()
            .halign(egui::Align::LEFT),
    )
    .on_hover_text(text);
}
