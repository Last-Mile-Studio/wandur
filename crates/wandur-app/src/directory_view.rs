//! Find a MUD: the world directory laid out like the C# browser (and wandur.net): a search bar
//! with the online and sort selects, further filters, a count, then one card per world; Explore
//! opens the world's own page ([`crate::world_page`]) in the same tab.
//!
//! The results are recomputed only when the query, the catalog or the saved worlds change, never
//! per frame. The list is virtualized: only the cards in view are laid out, and only they ask for
//! artwork (a card that scrolls away cancels its picture).

use std::sync::Arc;
use wandur_core::l10n::{S, t, tf};

use egui::{Align, CornerRadius, FontId, Key, Layout, RichText, Sense, Stroke, Ui, vec2};
use wandur_core::directory::client::{banner_url, generated_art_url};
use wandur_core::directory::{Artwork, Catalog, DirectoryStatus, Facet, Query, Sort, WorldListing, query};
use wandur_core::settings::{SavedWorld, Settings, same_host};

use crate::artwork::{ArtLoader, ArtRequest, ArtState, Target};
use crate::select::Select;
use crate::sessions::AppAction;
use crate::theme::Theme;
use crate::widgets;

/// Below this card width the plate goes on top and the text runs the card's full width, as the
/// site does on a phone.
pub const STACKED_BELOW: f32 = 560.0;
/// Below this the way in leaves its ruled column and sits under the text.
pub const SPLIT_BELOW: f32 = 760.0;
/// Space between cards.
const GAP: f32 = 12.0;
/// Room at the right of the list for its floating scroll bar.
const SCROLL_GUTTER: f32 = 14.0;
/// The plate's aspect (the site's 400 by 160).
const PLATE_ASPECT: f32 = 2.5;
/// The way in's column on wide cards.
const GO_COLUMN: f32 = 190.0;
/// Height of the search box, the selects and the buttons above the list.
const CONTROL_H: f32 = crate::select::HEIGHT;
/// Space between the controls, across and down.
const CONTROL_GAP: f32 = 8.0;
/// Corner radius of the controls.
const CONTROL_RADIUS: u8 = 8;
/// The search box's narrowest width beside the other controls on one row.
const SEARCH_MIN: f32 = 240.0;
/// The narrowest cell of the advanced filters' grid.
const FILTER_CELL_MIN: f32 = 160.0;
/// Space between the filter cells, across and down.
const FILTER_GAP: f32 = 12.0;
/// A filter cell: its caption, a gap, then the control.
const FILTER_CAPTION: f32 = 18.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardMode {
    Stacked,
    Compact,
    Wide,
}

/// How cards are laid out at a width: the mode, the plate's width and height, and the card's
/// height. Every card has the same height at a width, so the list lays out only the cards in
/// view; the text inside wraps to fit it (the blurb takes the rows the title and chips leave).
pub fn card_geometry(width: f32) -> (CardMode, egui::Vec2, f32) {
    if width < STACKED_BELOW {
        let plate_h = (width / PLATE_ASPECT).round().clamp(110.0, 170.0);
        (CardMode::Stacked, vec2(width, plate_h), plate_h + 184.0)
    } else if width < SPLIT_BELOW {
        let plate = (width * 0.36).round().clamp(190.0, 300.0);
        let h = (plate / PLATE_ASPECT).round().max(188.0);
        (CardMode::Compact, vec2(plate, h), h)
    } else {
        let plate = (width * 0.32).round().clamp(240.0, 400.0);
        let h = (plate / PLATE_ASPECT).round().max(172.0);
        (CardMode::Wide, vec2(plate, h), h)
    }
}

/// Where one control of the row above the list goes: its row, its left edge and its width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    pub row: usize,
    pub x: f32,
    pub width: f32,
}

/// Lay out the controls above the list in `avail` points: the search box, then the two selects
/// and the two buttons. `selects` holds each select's (narrowest, natural) width: the narrowest
/// shows the current choice, the natural one the longest. All on one row when they fit at their
/// natural widths, the search box taking the room left. Otherwise the search box takes a row of
/// its own and the selects and the buttons flow under it, a pair kept together while it fits; the
/// selects stretch to fill their row, and buttons on a row of their own share it. Nothing is
/// wider than `avail`.
pub fn control_slots(avail: f32, selects: [(f32, f32); 2], buttons: [f32; 2]) -> [Slot; 5] {
    let gap = CONTROL_GAP;
    let avail = avail.max(1.0);
    let rest = selects[0].1 + selects[1].1 + buttons[0] + buttons[1] + 4.0 * gap;
    let mut slots = [Slot {
        row: 0,
        x: 0.0,
        width: 0.0,
    }; 5];
    if SEARCH_MIN + rest <= avail {
        let mut x = avail - rest + gap;
        slots[0].width = avail - rest;
        for (i, w) in [selects[0].1, selects[1].1, buttons[0], buttons[1]]
            .into_iter()
            .enumerate()
        {
            slots[i + 1] = Slot { row: 0, x, width: w };
            x += w + gap;
        }
        return slots;
    }
    slots[0].width = avail;
    // Items 1..=4 at their narrowest, in pairs; rows of item indices.
    let natural = [0.0, selects[0].0, selects[1].0, buttons[0], buttons[1]];
    let mut rows: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let used = |row: &[usize]| row.iter().map(|&i| natural[i]).sum::<f32>() + gap * row.len().saturating_sub(1) as f32;
    for pair in [[1usize, 2], [3, 4]] {
        let pair_w = natural[pair[0]] + gap + natural[pair[1]];
        if (current.is_empty() && pair_w <= avail) || (!current.is_empty() && used(&current) + gap + pair_w <= avail) {
            current.extend(pair);
            continue;
        }
        for i in pair {
            if !current.is_empty() && used(&current) + gap + natural[i] > avail {
                rows.push(std::mem::take(&mut current));
            }
            current.push(i);
        }
    }
    if !current.is_empty() {
        rows.push(current);
    }
    for (r, row) in rows.iter().enumerate() {
        let extra = (avail - used(row)).max(0.0);
        let flexible: Vec<usize> = row.iter().copied().filter(|&i| i <= 2).collect();
        let share = if flexible.is_empty() { row.len() } else { flexible.len() };
        let mut x = 0.0;
        for &i in row {
            let grow = flexible.is_empty() || flexible.contains(&i);
            let w = (natural[i] + if grow { extra / share as f32 } else { 0.0 }).min(avail);
            slots[i] = Slot {
                row: r + 1,
                x,
                width: w,
            };
            x += w + gap;
        }
    }
    slots
}

/// Columns of the advanced filters' grid at `avail` points: as many cells of at least
/// [`FILTER_CELL_MIN`] as fit, at least one.
pub fn filter_columns(avail: f32) -> usize {
    (((avail + FILTER_GAP) / (FILTER_CELL_MIN + FILTER_GAP)).floor() as usize).max(1)
}

/// Where the directory list is scrolled (for the probe).
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollInfo {
    pub offset: f32,
    pub viewport: f32,
    pub extent: f32,
}

#[derive(Default)]
pub struct DirectoryView {
    pub query: Query,
    computed_for: Option<(Query, u64, u64)>,
    pub results: Vec<usize>,
    listed: usize,
    facet_options: Vec<(Facet, Vec<String>)>,
    catalog: Option<Arc<Catalog>>,
    /// The selected world (by id).
    pub selected: Option<String>,
    /// The world whose page is open.
    pub exploring: Option<String>,
    pub use_tls: bool,
    /// The outcome of Add to my worlds, shown under the list.
    pub feedback: String,
    filters_open: bool,
    min_text: String,
    max_text: String,
    /// The probe asks for a scroll offset here.
    pub scroll_to: Option<f32>,
    pub scroll: ScrollInfo,
    /// Cards laid out in the last frame.
    pub cards_drawn: usize,
    /// Set when the view was shown this frame and the directory has never been fetched.
    pub wants_fetch: bool,
    /// The results list's scroll offset when Explore was clicked, restored on the way back.
    results_offset: f32,
    restore_offset: bool,
    /// A select whose list is shown open (scenes): "sort" or "genre".
    pub open_select: Option<&'static str>,
}

/// What the directory views need from the app each frame.
pub struct DirContext<'a> {
    pub status: &'a DirectoryStatus,
    pub base: &'a str,
    pub art: &'a mut ArtLoader,
    pub theme: &'a Theme,
    pub settings: &'a Settings,
    pub actions: &'a mut Vec<AppAction>,
    pub now: i64,
}

/// Whether a saved world is at this listing's address.
pub fn is_saved(worlds: &[SavedWorld], w: &WorldListing) -> bool {
    w.can_connect()
        && worlds.iter().any(|s| {
            same_host(&s.host, &w.host)
                && if s.tls {
                    w.tls_port == Some(s.port)
                } else {
                    w.port == Some(s.port)
                }
        })
}

/// A hash of the saved worlds' addresses, so the results follow saves (a saved adult world is
/// always browsable).
fn saved_signature(worlds: &[SavedWorld]) -> u64 {
    let mut text = String::new();
    for w in worlds {
        text.push_str(&w.host);
        text.push_str(&w.port.to_string());
        text.push(if w.tls { 't' } else { 'p' });
    }
    wandur_core::directory::listing::fnv1a(text.as_bytes())
}

/// The artwork request for a world's picture at a pixel size (`size` is the directory's
/// generated size: "400" for rows, "hero" with "1024" as the fallback for the page).
pub fn art_request(
    w: &WorldListing,
    base: &str,
    kind: &str,
    target: Target,
    size: Option<&str>,
    fallback_size: Option<&str>,
) -> Option<ArtRequest> {
    let preferred = w.preferred_artwork()?;
    let generated = |s: Option<&str>| generated_art_url(base, &w.generated_artwork_path, s);
    let (url, fallback) = match preferred {
        Artwork::Supplied => (
            banner_url(&w.banner_url)?,
            w.has_generated_artwork().then(|| generated(size)).flatten(),
        ),
        Artwork::Generated => (
            generated(size)?,
            fallback_size
                .and_then(|s| generated(Some(s)))
                .or_else(|| banner_url(&w.banner_url)),
        ),
    };
    Some(ArtRequest {
        key: format!("{kind}:{}:{}x{}", w.art_key(), target.width, target.height),
        url,
        fallback,
        target,
    })
}

impl DirectoryView {
    /// Recompute the results if the query, the catalog or the saved worlds changed.
    pub fn refresh(&mut self, status: &DirectoryStatus, settings: &Settings, now: i64) {
        self.query.show_adult = settings.show_adult;
        let key = (self.query.clone(), status.revision, saved_signature(&settings.worlds));
        if self.computed_for.as_ref() == Some(&key) {
            return;
        }
        self.catalog = status.catalog.clone();
        let Some(catalog) = &self.catalog else {
            self.results.clear();
            self.listed = 0;
            self.computed_for = Some(key);
            return;
        };
        let worlds = &settings.worlds;
        let show_adult = self.query.show_adult;
        let browsable = |w: &WorldListing| show_adult || !w.is_adult() || is_saved(worlds, w);
        self.results = catalog.query(&self.query, now, browsable);
        self.listed = catalog.worlds.iter().filter(|w| browsable(w)).count();
        if self.facet_options.is_empty() || self.computed_for.as_ref().is_none_or(|c| c.1 != key.1) {
            self.facet_options = Facet::ALL
                .iter()
                .map(|&f| {
                    (
                        f,
                        query::facet_options(f, catalog.worlds.iter().filter(|w| browsable(w))),
                    )
                })
                .collect();
        }
        // Keep the selection when it is still listed, else select the first result.
        let still = self
            .selected
            .as_ref()
            .is_some_and(|id| self.results.iter().any(|&i| catalog.worlds[i].id == *id));
        if !still {
            self.selected = self.results.first().map(|&i| catalog.worlds[i].id.clone());
        }
        if let Some(id) = &self.exploring
            && catalog.by_id(id).is_none()
        {
            self.exploring = None;
        }
        self.computed_for = Some(key);
    }

    /// Show or hide the advanced filters (scenes open them).
    pub fn set_filters_open(&mut self, open: bool) {
        self.filters_open = open;
    }

    pub fn catalog(&self) -> Option<&Arc<Catalog>> {
        self.catalog.as_ref()
    }

    pub fn show(&mut self, ui: &mut Ui, cx: &mut DirContext<'_>) {
        if !cx.status.fetched_this_run && !cx.status.loading {
            self.wants_fetch = true;
        }
        self.refresh(cx.status, cx.settings, cx.now);
        let theme = cx.theme;
        egui::Frame::new()
            .fill(theme.shell)
            .inner_margin(egui::Margin::symmetric(18, 12))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                let exploring = self
                    .exploring
                    .clone()
                    .and_then(|id| self.catalog.as_ref().and_then(|c| c.by_id(&id).cloned()));
                match exploring {
                    Some(world) => {
                        let back = crate::world_page::show(ui, &world, self, cx);
                        if back || ui.input(|i| i.key_pressed(Key::Escape)) {
                            self.exploring = None;
                            self.restore_offset = true;
                        }
                    }
                    None => self.results_page(ui, cx),
                }
            });
    }

    fn results_page(&mut self, ui: &mut Ui, cx: &mut DirContext<'_>) {
        let theme = cx.theme;
        // Everything below is laid out to this width and never wider, so nothing is clipped at
        // the panel's edge.
        let width = ui.available_width().max(1.0);
        ui.set_max_width(width);
        let footer_h = 40.0;
        ui.label(RichText::new(t(S::FindAMUD)).size(22.0).strong());
        if ui.available_height() >= 700.0 {
            ui.add_space(2.0);
            ui.label(RichText::new(t(S::DirectoryLede)).size(15.0).color(theme.muted));
        }
        ui.add_space(8.0);
        // The controls leave the scroll bar's gutter free, so their right edge lines up with
        // the cards'.
        let content_w = (width - SCROLL_GUTTER).max(1.0);
        self.filter_bar(ui, cx, content_w);
        ui.add_space(10.0);
        let list_h = (ui.available_height() - footer_h).max(80.0);
        ui.allocate_ui_with_layout(vec2(width, list_h), Layout::top_down(Align::Min), |ui| {
            ui.set_max_width(width);
            self.results_scroll(ui, cx, content_w);
        });
        // Footer: what Add to my worlds did, and where the directory came from.
        ui.add_space(4.0);
        if !self.feedback.is_empty() {
            ui.add(egui::Label::new(RichText::new(&self.feedback).size(12.0).color(theme.text)).truncate());
        }
        ui.add(
            egui::Label::new(
                RichText::new(status_text(cx.status, cx.base))
                    .size(11.0)
                    .color(theme.muted),
            )
            .truncate(),
        );
    }

    /// The count of worlds shown and, while a search or filter is set, the way to clear them:
    /// on one row when both fit, the link under the count otherwise.
    fn count_row(&mut self, ui: &mut Ui, theme: &Theme, width: f32) {
        let count = RichText::new(query::count_text(self.results.len(), self.listed))
            .size(14.0)
            .color(theme.muted);
        let filtered = self.query
            != (Query {
                show_adult: self.query.show_adult,
                ..Query::default()
            });
        if !filtered {
            ui.label(count);
            return;
        }
        let measure = |text: &str, size: f32| {
            ui.fonts_mut(|f| {
                f.layout_no_wrap(text.to_string(), FontId::proportional(size), theme.text)
                    .size()
                    .x
            })
        };
        let one_row = measure(&query::count_text(self.results.len(), self.listed), 14.0)
            + measure(t(S::ClearSearchFilters), 13.0)
            + 24.0
            <= width;
        let mut clear = false;
        if one_row {
            ui.horizontal(|ui| {
                ui.label(count);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    clear = widgets::link(ui, t(S::ClearSearchFilters), theme.muted, 13.0, false).clicked();
                });
            });
        } else {
            ui.label(count);
            clear = widgets::link(ui, t(S::ClearSearchFilters), theme.muted, 13.0, false).clicked();
        }
        if clear {
            self.clear_filters();
        }
    }

    fn clear_filters(&mut self) {
        self.query = Query {
            show_adult: self.query.show_adult,
            ..Query::default()
        };
        self.min_text.clear();
        self.max_text.clear();
    }

    /// The search box, the online and sort selects, Filters and Refresh: one row when they fit,
    /// flowing onto more rows when not ([`control_slots`]).
    fn filter_bar(&mut self, ui: &mut Ui, cx: &mut DirContext<'_>, width: f32) {
        let theme = cx.theme;
        let online_labels = [t(S::AllWorlds), t(S::OnlineNow)];
        let online = Select::new("directory-online", t(S::OnlineNow))
            .radius(CONTROL_RADIUS)
            .dot(if self.query.online_only { theme.ok } else { theme.muted });
        let sort = Select::new("directory-sort", t(S::SortLabel))
            .radius(CONTROL_RADIUS)
            .open_now(self.open_select == Some("sort"))
            .prefix(t(S::SortLabel));
        // Natural widths from the longest option, so a choice never moves a single row.
        let widest = |select: &Select<'_>, labels: &mut dyn Iterator<Item = &str>| {
            labels.map(|l| select.natural_width(ui, l)).fold(0.0f32, f32::max)
        };
        let online_w = (
            online.natural_width(ui, online_labels[usize::from(self.query.online_only)]),
            widest(&online, &mut online_labels.iter().copied()),
        );
        let sort_w = (
            sort.natural_width(ui, self.query.sort.label()),
            widest(&sort, &mut Sort::ALL.iter().map(|s| s.label())),
        );
        let n = self.query.advanced_count();
        let filters_label = if n == 0 {
            t(S::DirectoryFilters).to_string()
        } else {
            format!("{} ({n})", t(S::DirectoryFilters))
        };
        let filters_w = crate::select::field_button_width(ui, &filters_label);
        let refresh_w =
            crate::select::field_button_width(ui, t(S::Refresh)) + if cx.status.loading { 22.0 } else { 0.0 };
        // Refresh keeps its word unless dropping it to an icon saves a row.
        let rows_of = |slots: &[Slot; 5]| slots.iter().map(|s| s.row).max().unwrap_or(0) + 1;
        let mut slots = control_slots(width, [online_w, sort_w], [filters_w, refresh_w]);
        let icon_slots = control_slots(width, [online_w, sort_w], [filters_w, CONTROL_H]);
        let refresh_icon = rows_of(&icon_slots) < rows_of(&slots);
        if refresh_icon {
            slots = icon_slots;
        }
        let rows = rows_of(&slots);
        let height = rows as f32 * CONTROL_H + (rows - 1) as f32 * CONTROL_GAP;
        let (block, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let at = |slot: Slot| {
            egui::Rect::from_min_size(
                egui::pos2(
                    block.left() + slot.x,
                    block.top() + slot.row as f32 * (CONTROL_H + CONTROL_GAP),
                ),
                vec2(slot.width, CONTROL_H),
            )
        };
        // Search.
        let search_rect = at(slots[0]);
        field_text(
            ui,
            search_rect,
            &mut self.query.search,
            FieldText {
                id: "directory-search",
                hint: t(S::SearchWorldsThemesOrAnAddress),
                name: t(S::SearchWorldsThemesOrAnAddress),
                size: 15.0,
                limit: 150,
                icon: true,
            },
        );
        // Online.
        let mut online_index = usize::from(self.query.online_only);
        let r = online.show_index_at(ui, at(slots[1]), &mut online_index, 2, |i| online_labels[i]);
        if r.changed() {
            self.query.online_only = online_index == 1;
        }
        r.on_hover_text(t(S::BasedOnTheDirectorySLatestReportNotA));
        // Sort.
        let mut sort_index = Sort::ALL.iter().position(|&s| s == self.query.sort).unwrap_or(0);
        if sort
            .show_index_at(ui, at(slots[2]), &mut sort_index, Sort::ALL.len(), |i| {
                Sort::ALL[i].label()
            })
            .changed()
        {
            self.query.sort = Sort::ALL[sort_index];
        }
        // Filters.
        let id = ui.make_persistent_id("directory-filters-button");
        if crate::select::field_button(
            ui,
            at(slots[3]),
            id,
            &filters_label,
            self.filters_open,
            true,
            CONTROL_RADIUS,
        )
        .clicked()
        {
            self.filters_open = !self.filters_open;
        }
        // Refresh, with the spinner while the directory loads.
        let rect = at(slots[4]);
        let id = ui.make_persistent_id("directory-refresh");
        let refresh = crate::select::field_button(
            ui,
            rect,
            id,
            if cx.status.loading || refresh_icon {
                ""
            } else {
                t(S::Refresh)
            },
            false,
            !cx.status.loading,
            CONTROL_RADIUS,
        );
        if refresh_icon && !cx.status.loading {
            let color = ui.visuals().text_color();
            widgets::paint_icon(ui, widgets::Icon::Reconnect, rect, color);
        }
        if cx.status.loading {
            let spin = egui::Rect::from_center_size(rect.center(), vec2(16.0, 16.0));
            ui.put(spin, egui::Spinner::new().size(16.0));
        }
        crate::a11y::named(refresh, t(S::Refresh))
            .on_hover_text(t(S::RefreshDirectory))
            .clicked()
            .then(|| cx.actions.push(AppAction::RefreshDirectory));
    }

    /// The advanced filters: a grid of captioned cells, as many columns as fit, flowing onto
    /// more rows; the note under it wraps.
    fn advanced_filters(&mut self, ui: &mut Ui, cx: &mut DirContext<'_>, width: f32) {
        let theme = cx.theme;
        widgets::card_frame(theme).inner_margin(14).show(ui, |ui| {
            let inner = (width - 30.0).max(1.0);
            ui.set_width(inner);
            ui.add(egui::Label::new(RichText::new(t(S::AdvancedSearchFindYourKindOfWorld)).size(13.0)).wrap());
            ui.add_space(8.0);
            let cells = self.facet_options.len() + 7;
            let columns = filter_columns(inner);
            let cell_w = ((inner - FILTER_GAP * (columns - 1) as f32) / columns as f32).max(1.0);
            let rows = cells.div_ceil(columns);
            let cell_h = FILTER_CAPTION + CONTROL_H;
            let height = rows as f32 * cell_h + (rows - 1) as f32 * FILTER_GAP;
            let (block, _) = ui.allocate_exact_size(vec2(inner, height), Sense::hover());
            let cell = |i: usize| {
                let (r, c) = (i / columns, i % columns);
                egui::Rect::from_min_size(
                    egui::pos2(
                        block.left() + c as f32 * (cell_w + FILTER_GAP),
                        block.top() + r as f32 * (cell_h + FILTER_GAP),
                    ),
                    vec2(cell_w, cell_h),
                )
            };
            // A cell's caption, and the rectangle for its control.
            let caption = |ui: &Ui, rect: egui::Rect, text: &str| {
                let g = widgets::clipped(ui, text, FontId::proportional(12.0), theme.muted, rect.width(), 1);
                ui.painter().galley(rect.min, g, theme.muted);
                egui::Rect::from_min_max(egui::pos2(rect.left(), rect.top() + FILTER_CAPTION), rect.max)
            };
            let mut i = 0;
            let facets = std::mem::take(&mut self.facet_options);
            for (facet, options) in &facets {
                let control = caption(ui, cell(i), facet.label());
                i += 1;
                let current = self.query.facet(*facet);
                let mut index = current
                    .and_then(|c| options.iter().position(|o| o == c).map(|p| p + 1))
                    .unwrap_or(0);
                let changed = Select::new(("facet", *facet as u8), facet.label())
                    .radius(CONTROL_RADIUS)
                    .open_now(self.open_select == Some("genre") && *facet == Facet::Genre)
                    .show_index_at(ui, control, &mut index, options.len() + 1, |k| {
                        if k == 0 { t(S::Any) } else { options[k - 1].as_str() }
                    })
                    .changed();
                if changed {
                    self.query
                        .set_facet(*facet, (index > 0).then(|| options[index - 1].clone()));
                }
            }
            self.facet_options = facets;
            for (label, text, value, id) in [
                (
                    t(S::MinObservedPlayers),
                    &mut self.min_text,
                    &mut self.query.min_players,
                    "directory-min",
                ),
                (
                    t(S::MaxObservedPlayers),
                    &mut self.max_text,
                    &mut self.query.max_players,
                    "directory-max",
                ),
            ] {
                let control = caption(ui, cell(i), label);
                i += 1;
                if text.is_empty()
                    && let Some(v) = value
                {
                    *text = v.to_string();
                }
                let response = field_text(
                    ui,
                    control,
                    text,
                    FieldText {
                        id,
                        hint: t(S::Any),
                        name: label,
                        size: 14.0,
                        limit: 9,
                        icon: false,
                    },
                );
                if response.changed() {
                    text.retain(|c| c.is_ascii_digit());
                    *value = text.parse().ok();
                }
            }
            let control = caption(ui, cell(i), t(S::MinimumRating));
            i += 1;
            let ratings = [
                t(S::AnyRating),
                t(S::ThreeStarsAndUp),
                t(S::FourStarsAndUp),
                t(S::FourAndAHalfStarsAndUp),
            ];
            let mut rating = usize::from(self.query.rating.min(3));
            if Select::new("directory-rating", t(S::MinimumRating))
                .radius(CONTROL_RADIUS)
                .show_index_at(ui, control, &mut rating, ratings.len(), |k| ratings[k])
                .changed()
            {
                self.query.rating = rating as u8;
            }
            let control = caption(ui, cell(i), t(S::MUDConnections));
            i += 1;
            {
                use wandur_core::directory::Connection;
                let options = [
                    (Connection::All, t(S::AllWorlds)),
                    (Connection::MudOnly, t(S::MUDConnections)),
                    (Connection::WebOnly, t(S::BrowserOnlyWorlds)),
                ];
                let mut index = options
                    .iter()
                    .position(|(c, _)| *c == self.query.connection)
                    .unwrap_or(0);
                if Select::new("directory-connection", t(S::MUDConnections))
                    .radius(CONTROL_RADIUS)
                    .show_index_at(ui, control, &mut index, options.len(), |k| options[k].1)
                    .changed()
                {
                    self.query.connection = options[index].0;
                }
            }
            let control = caption(ui, cell(i), t(S::ConnectionSecurity));
            i += 1;
            let id = ui.make_persistent_id("directory-tls");
            if field_check(ui, control, id, self.query.tls_only, t(S::TLSAvailable), theme).clicked() {
                self.query.tls_only = !self.query.tls_only;
            }
            let control = caption(ui, cell(i), t(S::AdultWorlds));
            i += 1;
            let id = ui.make_persistent_id("directory-adult");
            let adult = cx.settings.show_adult;
            if field_check(ui, control, id, adult, t(S::ShowAdultWorlds), theme)
                .on_hover_text(t(S::AdultWorldsHint))
                .clicked()
            {
                cx.actions.push(AppAction::ShowAdult(!adult));
            }
            let control = caption(ui, cell(i), t(S::StartAgain));
            let id = ui.make_persistent_id("directory-clear");
            let w = crate::select::field_button_width(ui, t(S::ClearSearchFilters)).min(control.width());
            let rect = egui::Rect::from_min_size(control.min, vec2(w, CONTROL_H));
            if crate::select::field_button(ui, rect, id, t(S::ClearSearchFilters), false, true, CONTROL_RADIUS)
                .clicked()
            {
                self.clear_filters();
            }
            ui.add_space(10.0);
            let hint = match (self.query.min_players, self.query.max_players) {
                (Some(a), Some(b)) if a > b => t(S::MinimumPlayersExceedsMaximumAdjustTheRangeToFind),
                _ => t(S::AllPreferencesCombinePlayerCountsAreLastObservedNot),
            };
            ui.add(egui::Label::new(RichText::new(hint).size(11.5).color(theme.muted)).wrap());
        });
    }

    fn empty_state(&self, ui: &mut Ui, theme: &Theme) {
        let empty_catalog = self.catalog.as_ref().is_none_or(|c| c.is_empty());
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            let (title, text) = if empty_catalog {
                (
                    t(S::AWorldOfPossibilities),
                    t(S::YourDirectoryWillAppearHereWhenItIsAvailable),
                )
            } else {
                (t(S::NoWorldsFound), t(S::TryRemovingAFilterOrShorteningYourSearchAll))
            };
            ui.label(RichText::new(title).size(20.0).strong());
            ui.add_space(6.0);
            ui.label(RichText::new(text).color(theme.muted));
        });
    }

    /// What scrolls under the controls: the advanced filters while open, the count, then the
    /// cards (or the empty state). Only the cards in view are laid out (and ask for artwork);
    /// the rest are space of a known height.
    fn results_scroll(&mut self, ui: &mut Ui, cx: &mut DirContext<'_>, card_w: f32) {
        let Some(catalog) = self.catalog.clone() else {
            self.count_row(ui, cx.theme, card_w);
            self.empty_state(ui, cx.theme);
            return;
        };
        let (mode, plate, base_h) = card_geometry(card_w);
        let go = GoRow::measure(ui, card_w, mode, plate);
        let card_h = base_h + go.extra;
        let pitch = card_h + GAP;
        let mut area = egui::ScrollArea::vertical()
            .id_salt("directory-results")
            .auto_shrink(false);
        if let Some(offset) = self.scroll_to.take() {
            area = area.vertical_scroll_offset(offset);
        } else if std::mem::take(&mut self.restore_offset) {
            area = area.vertical_scroll_offset(self.results_offset);
        }
        let mut drawn = 0;
        let mut explore = None;
        let mut save = None;
        let mut select = None;
        let saved_worlds = &cx.settings.worlds;
        let count = self.results.len();
        let output = area.show_viewport(ui, |ui, viewport| {
            ui.set_width(card_w);
            let origin = ui.max_rect().top();
            if self.filters_open {
                self.advanced_filters(ui, cx, card_w);
                ui.add_space(10.0);
            }
            self.count_row(ui, cx.theme, card_w);
            ui.add_space(6.0);
            if count == 0 {
                self.empty_state(ui, cx.theme);
                return;
            }
            ui.spacing_mut().item_spacing.y = 0.0;
            let top = ui.cursor().top() - origin;
            let first = (((viewport.min.y - top) / pitch).floor().max(0.0) as usize).min(count);
            let last = (((viewport.max.y - top) / pitch).ceil().max(0.0) as usize).clamp(first, count);
            if first > 0 {
                ui.allocate_space(vec2(card_w, first as f32 * pitch));
            }
            for row in first..last {
                let w = &catalog.worlds[self.results[row]];
                drawn += 1;
                let selected = self.selected.as_deref() == Some(w.id.as_str());
                let saved = is_saved(saved_worlds, w);
                let layout = CardLayout {
                    mode,
                    plate,
                    card_h,
                    width: card_w,
                    go,
                };
                match card(ui, w, layout, selected, saved, cx) {
                    CardAction::None => {}
                    CardAction::Select => select = Some(w.id.clone()),
                    CardAction::Explore => explore = Some(w.id.clone()),
                    CardAction::Save => save = Some(w.id.clone()),
                }
                ui.allocate_space(vec2(card_w, GAP));
            }
            if last < count {
                ui.allocate_space(vec2(card_w, (count - last) as f32 * pitch));
            }
        });
        self.cards_drawn = drawn;
        self.scroll = ScrollInfo {
            offset: output.state.offset.y,
            viewport: output.inner_rect.height(),
            extent: output.content_size.y,
        };
        if let Some(id) = select {
            self.selected = Some(id);
        }
        if let Some(id) = save {
            self.selected = Some(id.clone());
            cx.actions.push(AppAction::SaveListing {
                id,
                tls: false,
                connect: false,
            });
        }
        // Enter explores the selected world (when no text field has the keyboard).
        let enter = ui.memory(|m| m.focused().is_none()) && ui.input(|i| i.key_pressed(Key::Enter));
        if explore.is_none() && enter {
            explore = self.selected.clone();
        }
        if let Some(id) = explore {
            self.results_offset = self.scroll.offset;
            self.explore(&catalog, id);
        }
    }

    /// Show a listed world's page.
    pub fn explore(&mut self, catalog: &Catalog, id: String) {
        if let Some(w) = catalog.by_id(&id) {
            self.use_tls = w.port.is_none() && w.tls_port.is_some();
        }
        self.feedback.clear();
        self.selected = Some(id.clone());
        self.exploring = Some(id);
    }
}

/// What a [`field_text`] shows.
struct FieldText<'a> {
    id: &'a str,
    hint: &'a str,
    /// Its name for screen readers.
    name: &'a str,
    size: f32,
    limit: usize,
    /// A magnifying glass at the left (the search box).
    icon: bool,
}

/// A one-line text field drawn as the selects are (one surface in the field colour), filling
/// `rect`.
fn field_text(ui: &mut Ui, rect: egui::Rect, text: &mut String, f: FieldText<'_>) -> egui::Response {
    let radius = CornerRadius::same(CONTROL_RADIUS);
    let id = ui.make_persistent_id(f.id);
    let focused = ui.memory(|m| m.has_focus(id));
    let hovered = ui.rect_contains_pointer(rect);
    crate::select::paint_field(ui, rect, radius, hovered, false, focused);
    let mut left = rect.left() + 12.0;
    if f.icon {
        let icon = egui::Rect::from_center_size(egui::pos2(rect.left() + 20.0, rect.center().y), vec2(20.0, 20.0));
        widgets::paint_icon(ui, widgets::Icon::Search, icon, ui.visuals().weak_text_color());
        left = rect.left() + 36.0;
    }
    let inner = egui::Rect::from_min_max(
        egui::pos2(left, rect.top()),
        egui::pos2(rect.right() - 10.0, rect.bottom()),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::left_to_right(Align::Center)),
    );
    let edit = egui::TextEdit::singleline(text)
        .id(id)
        .hint_text(f.hint)
        .frame(egui::Frame::NONE)
        .desired_width(inner.width())
        .char_limit(f.limit)
        .font(FontId::proportional(f.size));
    let response = child.add(edit);
    crate::a11y::named(response, f.name)
}

/// A check box drawn as a field (a filter cell's): the box and its text on one surface, where a
/// click anywhere toggles it. Returns the response; the caller flips the value on a click.
fn field_check(ui: &mut Ui, rect: egui::Rect, id: egui::Id, on: bool, text: &str, theme: &Theme) -> egui::Response {
    let response = ui.interact(rect, id, Sense::click());
    let radius = CornerRadius::same(CONTROL_RADIUS);
    crate::select::paint_field(ui, rect, radius, response.hovered(), false, response.has_focus());
    let check = egui::Rect::from_center_size(egui::pos2(rect.left() + 20.0, rect.center().y), vec2(16.0, 16.0));
    widgets::paint_check(ui, check, on, true, theme);
    let g = widgets::clipped(
        ui,
        text,
        FontId::proportional(14.0),
        theme.text,
        (rect.width() - 44.0).max(1.0),
        1,
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 36.0, rect.center().y - g.size().y / 2.0),
        g,
        theme.text,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, text));
    response
}

/// The status line under the list (the C# wording).
pub fn status_text(status: &DirectoryStatus, base: &str) -> String {
    if status.loading {
        return t(S::LoadingTheDirectoryTheFirstFullDownloadMayTake).into();
    }
    if let Some(w) = &status.warning {
        return w.clone();
    }
    match status.catalog.as_ref().and_then(|c| c.fetched_at) {
        Some(at) => tf(
            S::DirectorySavedFrom,
            &[&wandur_core::directory::time::date_text(at), &base],
        ),
        None => tf(S::StartTheDirectoryServer, &[&base]),
    }
}

enum CardAction {
    None,
    Select,
    Explore,
    Save,
}

/// How the way in ("Explore world", "+ Add to my worlds") sits on a card that is not wide: on
/// one row under the text, or on two when the card is too narrow for both (the card grows by
/// `extra`).
#[derive(Clone, Copy, Debug)]
struct GoRow {
    two_rows: bool,
    extra: f32,
}

impl GoRow {
    fn measure(ui: &Ui, card_w: f32, mode: CardMode, plate: egui::Vec2) -> GoRow {
        if mode == CardMode::Wide {
            return GoRow {
                two_rows: false,
                extra: 0.0,
            };
        }
        let text_w = match mode {
            CardMode::Stacked => card_w - 32.0,
            _ => card_w - plate.x - 36.0,
        };
        let w = |text: &str, size: f32| {
            ui.fonts_mut(|f| {
                f.layout_no_wrap(text.to_string(), FontId::proportional(size), egui::Color32::WHITE)
                    .size()
                    .x
            })
        };
        let need = w(t(S::ExploreWorld), 16.0) + 18.0 + w(&format!("+ {}", t(S::AddToMyWorlds)), 12.0) + 4.0;
        let two_rows = need > text_w;
        GoRow {
            two_rows,
            extra: if two_rows { 22.0 } else { 0.0 },
        }
    }
}

/// A card's geometry for the frame (the same for every card).
#[derive(Clone, Copy, Debug)]
struct CardLayout {
    mode: CardMode,
    plate: egui::Vec2,
    card_h: f32,
    width: f32,
    go: GoRow,
}

/// A chip on a card: an outlined pill, the green beginner pill, or the online count.
enum Chip<'a> {
    Pill(&'a str),
    Beginner,
    Live(&'a str),
}

/// Lay chips out from `(x, y)` within `width`, wrapping onto at most `max_rows` rows (chips past
/// them are left out), and paint them. Returns the bottom of the last row (or `y` when none).
fn paint_chips(
    ui: &Ui,
    painter: &egui::Painter,
    origin: egui::Pos2,
    width: f32,
    chips: &[Chip<'_>],
    max_rows: usize,
    theme: &Theme,
) -> f32 {
    const H: f32 = 20.0;
    const PAD: f32 = 8.0;
    const GAP_X: f32 = 6.0;
    const GAP_Y: f32 = 6.0;
    let size = 12.0;
    let (mut x, mut row) = (0.0f32, 0usize);
    let mut bottom = origin.y;
    for chip in chips {
        let (text, color) = match chip {
            Chip::Pill(text) => (*text, theme.text),
            Chip::Beginner => (t(S::BeginnerFriendly), theme.ok),
            Chip::Live(text) => (*text, theme.text),
        };
        let live = matches!(chip, Chip::Live(_));
        let max_text = (width - if live { 12.0 } else { 2.0 * PAD }).max(1.0);
        let g = widgets::clipped(ui, text, FontId::proportional(size), color, max_text, 1);
        let w = g.size().x + if live { 12.0 } else { 2.0 * PAD };
        if x > 0.0 && x + w > width {
            row += 1;
            x = 0.0;
        }
        if row >= max_rows {
            break;
        }
        let rect = egui::Rect::from_min_size(origin + vec2(x, row as f32 * (H + GAP_Y)), vec2(w.min(width), H));
        let text_y = rect.center().y - g.size().y / 2.0;
        match chip {
            Chip::Live(_) => {
                painter.galley(egui::pos2(rect.left(), text_y), g, color);
                painter.circle_filled(egui::pos2(rect.right() - 4.0, rect.center().y), 3.0, theme.ok);
            }
            _ => {
                let stroke = if matches!(chip, Chip::Beginner) {
                    Stroke::new(1.0, theme.ok.gamma_multiply(0.7))
                } else {
                    Stroke::new(1.0, theme.border)
                };
                painter.rect_stroke(rect, CornerRadius::same(255), stroke, egui::StrokeKind::Inside);
                painter.galley(egui::pos2(rect.left() + PAD, text_y), g, color);
                if matches!(chip, Chip::Beginner) {
                    let id = egui::Id::new(("beginner-chip", rect.min.x as i32, rect.min.y as i32));
                    ui.interact(rect, id, Sense::hover())
                        .on_hover_text(t(S::BeginnerFriendlyHint));
                }
            }
        }
        x += w + GAP_X;
        bottom = rect.bottom();
    }
    bottom
}

/// One world, laid out as the site's row: a wide art plate, the name, a line of chips, the
/// blurb, one bottom chip, then the accent "Explore world" (in a ruled column on wide cards).
/// The text wraps to the card and never leaves it: the name takes up to two rows, the chips up
/// to two, and the blurb the rows that are left, each ending in an ellipsis when cut.
fn card(
    ui: &mut Ui,
    w: &WorldListing,
    layout: CardLayout,
    selected: bool,
    saved: bool,
    cx: &mut DirContext<'_>,
) -> CardAction {
    let theme = cx.theme;
    let CardLayout {
        mode,
        plate,
        card_h,
        width,
        go,
    } = layout;
    let (rect, response) = ui.allocate_exact_size(vec2(width, card_h), Sense::click());
    crate::a11y::toggle(&response, egui::accesskit::Role::ListBoxOption, &w.name, selected);
    let radius = CornerRadius::same(12);
    ui.painter().rect_filled(rect, radius, theme.panel);
    // The plate: the row size is 400 by 160 points; ask for that many pixels.
    let ppp = ui.ctx().pixels_per_point();
    let target = Target {
        width: (400.0 * ppp).round() as u32,
        height: (160.0 * ppp).round() as u32,
        cover: true,
    };
    let state =
        art_request(w, cx.base, "card", target, Some("400"), None).map_or(ArtState::Failed, |r| cx.art.request(&r));
    let (plate_rect, text_rect, plate_radius) = match mode {
        CardMode::Stacked => {
            let plate = egui::Rect::from_min_size(rect.min, plate);
            (
                plate,
                egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 16.0, plate.bottom() + 12.0),
                    rect.max - vec2(16.0, 12.0),
                ),
                CornerRadius {
                    nw: 12,
                    ne: 12,
                    sw: 0,
                    se: 0,
                },
            )
        }
        CardMode::Compact | CardMode::Wide => {
            let plate = egui::Rect::from_min_size(rect.min, vec2(plate.x, card_h));
            (
                plate,
                egui::Rect::from_min_max(
                    egui::pos2(plate.right() + 20.0, rect.top() + 14.0),
                    rect.max - vec2(16.0, 12.0),
                ),
                CornerRadius {
                    nw: 12,
                    ne: 0,
                    sw: 12,
                    se: 0,
                },
            )
        }
    };
    widgets::plate(ui, plate_rect, plate_radius, &state, &w.name, theme, 28.0);
    let stroke = if selected {
        Stroke::new(1.5, theme.accent)
    } else {
        Stroke::new(1.0, theme.border)
    };
    ui.painter().rect_stroke(rect, radius, stroke, egui::StrokeKind::Inside);
    // The text column (left of the way in's column on wide cards); painted clipped to the card.
    let go_w = if mode == CardMode::Wide { GO_COLUMN } else { 0.0 };
    let copy = egui::Rect::from_min_max(text_rect.min, egui::pos2(text_rect.right() - go_w, text_rect.bottom()));
    let tw = copy.width().max(1.0);
    let painter = ui.painter_at(rect.intersect(ui.clip_rect()));
    let mut y = copy.top();
    let name = widgets::clipped(ui, &w.name, FontId::proportional(19.0), theme.text, tw, 2);
    y += name.size().y;
    painter.galley(copy.min, name, theme.text);
    let (top, bottom) = w.row_pills();
    let online = w.online_text(cx.now);
    let mut chips: Vec<Chip<'_>> = top.iter().map(|p| Chip::Pill(p)).collect();
    if w.is_adult() {
        chips.push(Chip::Pill(t(S::AdultChip)));
    }
    if let Some(text) = &online {
        chips.push(Chip::Live(text));
    }
    if !chips.is_empty() {
        y = paint_chips(ui, &painter, egui::pos2(copy.left(), y + 7.0), tw, &chips, 2, theme);
    }
    // What goes under the blurb: one chip, then (unless wide) the way in.
    let tail = if w.beginner_friendly == Some(true) {
        Some(Chip::Beginner)
    } else {
        bottom.map(Chip::Pill)
    };
    let go_h = if mode == CardMode::Wide {
        0.0
    } else if go.two_rows {
        50.0
    } else {
        28.0
    };
    let reserved = go_h + if tail.is_some() { 28.0 } else { 0.0 };
    let blurb = w.blurb();
    if !blurb.is_empty() {
        let font = FontId::proportional(14.0);
        let row_h = ui.fonts_mut(|f| f.row_height(&font));
        let room = copy.bottom() - reserved - (y + 7.0);
        let rows = ((room / row_h).floor().max(0.0) as usize).min(4);
        if rows > 0 {
            let g = widgets::clipped(ui, &blurb, font, theme.muted, tw, rows);
            let h = g.size().y;
            painter.galley(egui::pos2(copy.left(), y + 7.0), g, theme.muted);
            y += 7.0 + h;
        }
    }
    if let Some(chip) = tail {
        paint_chips(ui, &painter, egui::pos2(copy.left(), y + 8.0), tw, &[chip], 1, theme);
    }
    let mut action = CardAction::None;
    let go_rect = if mode == CardMode::Wide {
        let x = text_rect.right() - go_w;
        ui.painter().vline(
            x,
            (rect.top() + 22.0)..=(rect.bottom() - 22.0),
            Stroke::new(1.0, theme.border),
        );
        egui::Rect::from_min_max(
            egui::pos2(x + 24.0, rect.top()),
            egui::pos2(rect.right() - 12.0, rect.bottom()),
        )
    } else {
        egui::Rect::from_min_max(
            egui::pos2(copy.left(), copy.bottom() - go_h),
            egui::pos2(copy.right(), copy.bottom()),
        )
    };
    let mut ui_go = ui.new_child(egui::UiBuilder::new().max_rect(go_rect).layout(
        if mode == CardMode::Wide || go.two_rows {
            Layout::top_down(Align::Min)
        } else {
            Layout::left_to_right(Align::Center)
        },
    ));
    ui_go.set_clip_rect(rect.intersect(ui.clip_rect()));
    if mode == CardMode::Wide {
        ui_go.add_space((go_rect.height() - 52.0) / 2.0);
    }
    ui_go.spacing_mut().item_spacing = vec2(18.0, 6.0);
    if widgets::link(&mut ui_go, t(S::ExploreWorld), theme.accent, 16.0, true).clicked() {
        action = CardAction::Explore;
    }
    if w.can_connect() {
        if saved {
            ui_go.label(RichText::new(t(S::WorldSaved)).size(12.0).color(theme.muted));
        } else if widgets::link(
            &mut ui_go,
            &format!("+ {}", t(S::AddToMyWorlds)),
            theme.muted,
            12.0,
            false,
        )
        .clicked()
        {
            action = CardAction::Save;
        }
    }
    if matches!(action, CardAction::None) {
        if response.double_clicked() {
            action = CardAction::Explore;
        } else if response.clicked() {
            action = CardAction::Select;
        }
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_modes_follow_the_breakpoints() {
        assert_eq!(card_geometry(400.0).0, CardMode::Stacked);
        assert_eq!(card_geometry(400.0).1, vec2(400.0, 160.0));
        assert_eq!(card_geometry(600.0).0, CardMode::Compact);
        let (mode, plate, h) = card_geometry(1250.0);
        assert_eq!((mode, plate, h), (CardMode::Wide, vec2(400.0, 172.0), 172.0));
        assert_eq!(card_geometry(800.0).1.x, 256.0);
        assert_eq!(card_geometry(700.0).0, CardMode::Compact);
    }

    #[test]
    fn the_controls_never_leave_their_width() {
        let selects = [(130.0, 140.0), (170.0, 230.0)];
        let buttons = [76.0, 90.0];
        let mut width = 120.0;
        while width <= 2000.0 {
            let slots = control_slots(width, selects, buttons);
            for (i, s) in slots.iter().enumerate() {
                assert!(s.x >= 0.0 && s.width > 0.0, "{width}: slot {i} {s:?}");
                assert!(
                    s.x + s.width <= width + 0.01,
                    "{width}: slot {i} {s:?} runs past the edge"
                );
            }
            for a in 0..5 {
                for b in a + 1..5 {
                    let (p, q) = (slots[a], slots[b]);
                    if p.row == q.row {
                        assert!(
                            p.x + p.width <= q.x + 0.01 || q.x + q.width <= p.x + 0.01,
                            "{width}: slots {a} and {b} overlap"
                        );
                    }
                }
            }
            width += 7.0;
        }
        // Wide: one row, the search box taking the room left.
        let slots = control_slots(1100.0, selects, buttons);
        assert!(slots.iter().all(|s| s.row == 0));
        assert!((slots[4].x + slots[4].width - 1100.0).abs() < 0.01);
        // Narrow: the search box on its own row, the selects filling theirs.
        let slots = control_slots(470.0, selects, buttons);
        assert_eq!((slots[0].row, slots[0].width), (0, 470.0));
        assert!(slots[1..].iter().all(|s| s.row >= 1));
        assert_eq!(slots[1].row, slots[2].row);
        assert!((slots[2].x + slots[2].width - 470.0).abs() < 0.01 || slots[2].row != slots[4].row);
        // Very narrow: one control a row, none wider than the space.
        let slots = control_slots(150.0, selects, buttons);
        assert!(slots.iter().all(|s| s.width <= 150.0));
    }

    #[test]
    fn the_filter_grid_wraps_into_columns_that_fit() {
        assert_eq!(filter_columns(100.0), 1);
        assert_eq!(filter_columns(330.0), 1);
        assert_eq!(filter_columns(340.0), 2);
        assert_eq!(filter_columns(760.0), 4);
        assert_eq!(filter_columns(1200.0), 7);
        for w in [200.0, 333.0, 520.0, 777.0, 1024.0, 1500.0] {
            let c = filter_columns(w) as f32;
            let cell = (w - FILTER_GAP * (c - 1.0)) / c;
            assert!(cell >= FILTER_CELL_MIN || c == 1.0, "{w}: cell {cell}");
        }
    }

    fn sample_catalog() -> Arc<Catalog> {
        let long = "An enormous persistent world of sprawling continents, warring houses, hand-written quests \
                    and a crafting economy run entirely by players, with roleplay enforced in every zone";
        let worlds = (0..12)
            .map(|i| {
                let mut w = WorldListing {
                    id: format!("w{i}"),
                    name: if i % 2 == 0 {
                        format!("The Extraordinarily Long Named Realm of Everlasting Twilight {i}")
                    } else {
                        format!("Brief {i}")
                    },
                    host: "mud.example.org".into(),
                    port: Some(4000),
                    summary: long.into(),
                    tags: ["Roleplay", "Exploration", "Crafting", "Player killing", "Clans"]
                        .iter()
                        .map(|t| t.to_string())
                        .collect(),
                    beginner_friendly: Some(i % 3 == 0),
                    ..Default::default()
                };
                w.features.theme = "Science fiction and fantasy crossover".into();
                w.features.codebase = format!("Codebase number {i} with a long name");
                w.features.language = "English".into();
                w.availability.online = Some(true);
                w
            })
            .collect();
        Arc::new(Catalog::new(wandur_core::directory::snapshot::Snapshot {
            fetched_at: Some(1_700_000_000),
            worlds,
            skipped: 0,
        }))
    }

    /// Every node a screen reader finds while Find a MUD is shown in a panel `width` points wide,
    /// with the panel's rectangle.
    fn render(width: f32, filters: bool, theme: &str) -> (egui::Rect, Vec<egui::accesskit::Node>) {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let theme = Theme::preset(theme);
        theme.apply(&ctx);
        let status = DirectoryStatus {
            catalog: Some(sample_catalog()),
            revision: 1,
            fetched_this_run: true,
            ..Default::default()
        };
        let settings = Settings::default();
        let fetch: crate::artwork::FetchArt = Arc::new(|_| Err("offline".into()));
        let mut art = ArtLoader::new(1, fetch, None, 1 << 20, Arc::new(|| ()));
        let mut view = DirectoryView::default();
        view.set_filters_open(filters);
        let panel = egui::Rect::from_min_size(egui::pos2(40.0, 30.0), vec2(width, 900.0));
        let mut nodes = Vec::new();
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(width + 200.0, 1000.0))),
                ..Default::default()
            };
            let mut actions = Vec::new();
            let mut out = ctx.run_ui(input, |ui| {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(panel));
                child.set_clip_rect(panel);
                let mut cx = DirContext {
                    status: &status,
                    base: "http://127.0.0.1:9/",
                    art: &mut art,
                    theme: &theme,
                    settings: &settings,
                    actions: &mut actions,
                    now: 1_700_000_000,
                };
                cx.art.begin_frame(ui.ctx());
                view.show(&mut child, &mut cx);
                cx.art.end_frame();
            });
            out.textures_delta.clear();
            nodes = out
                .platform_output
                .accesskit_update
                .take()
                .map(|u| u.nodes.into_iter().map(|(_, n)| n).collect())
                .unwrap_or_default();
        }
        (panel, nodes)
    }

    #[test]
    fn nothing_runs_past_the_panel_at_any_width() {
        for theme in ["Slate", "Linen"] {
            for width in [280.0, 360.0, 440.0, 520.0, 640.0, 800.0, 1100.0, 1500.0] {
                for filters in [false, true] {
                    let (panel, nodes) = render(width, filters, theme);
                    assert!(nodes.len() > 10, "{width}: {} nodes", nodes.len());
                    let mut controls = 0;
                    for n in &nodes {
                        let Some(b) = n.bounds() else { continue };
                        if n.role() == egui::accesskit::Role::Window || b.x1 - b.x0 < 0.5 {
                            continue;
                        }
                        let what = format!("{:?} {:?} at {:.0}..{:.0}", n.role(), n.label(), b.x0, b.x1);
                        assert!(
                            b.x0 >= f64::from(panel.left()) - 0.5 && b.x1 <= f64::from(panel.right()) + 0.5,
                            "{theme} {width} filters {filters}: {what} leaves {:.0}..{:.0}",
                            panel.left(),
                            panel.right()
                        );
                        controls += 1;
                    }
                    assert!(controls > 5);
                }
            }
        }
    }

    #[test]
    fn the_filter_grid_flows_onto_more_rows_when_narrow() {
        let top_of = |nodes: &[egui::accesskit::Node], label: &str| {
            nodes
                .iter()
                .find(|n| n.role() == egui::accesskit::Role::ComboBox && n.label() == Some(label))
                .and_then(|n| n.bounds())
                .map(|b| b.y0)
                .unwrap_or_else(|| panic!("no {label} select"))
        };
        let (_, nodes) = render(520.0, true, "Slate");
        assert!(
            top_of(&nodes, t(S::Codebase)) > top_of(&nodes, t(S::Genre)) + 30.0,
            "a second row"
        );
        let (_, nodes) = render(1500.0, true, "Slate");
        assert!(
            (top_of(&nodes, t(S::Codebase)) - top_of(&nodes, t(S::Genre))).abs() < 1.0,
            "one row of six"
        );
        // The controls above share one row when wide.
        let (_, nodes) = render(1100.0, false, "Slate");
        assert!((top_of(&nodes, t(S::SortLabel)) - top_of(&nodes, t(S::OnlineNow))).abs() < 1.0);
    }

    #[test]
    fn art_requests_prefer_the_owner_banner_then_the_illustration() {
        let target = Target {
            width: 800,
            height: 320,
            cover: true,
        };
        let mut w = WorldListing {
            id: "a".into(),
            name: "A".into(),
            generated_artwork_path: "worlds/a/art".into(),
            ..Default::default()
        };
        let r = art_request(&w, "http://127.0.0.1:9/", "card", target, Some("400"), None).unwrap();
        assert_eq!(r.url, "http://127.0.0.1:9/worlds/a/art?size=400");
        assert!(r.key.starts_with("card:") && r.key.ends_with(":800x320"));
        w.banner_url = "https://cdn.example.org/a.png".into();
        w.banner_by_owner = Some(true);
        let r = art_request(&w, "http://127.0.0.1:9/", "hero", target, Some("hero"), Some("1024")).unwrap();
        assert_eq!(r.url, "https://cdn.example.org/a.png");
        assert_eq!(r.fallback.as_deref(), Some("http://127.0.0.1:9/worlds/a/art?size=hero"));
        w.banner_url.clear();
        w.generated_artwork_path.clear();
        assert!(art_request(&w, "http://127.0.0.1:9/", "card", target, None, None).is_none());
    }

    #[test]
    fn saved_matching_uses_the_chosen_port() {
        let w = WorldListing {
            id: "a".into(),
            name: "A".into(),
            host: "mud.example.org".into(),
            port: Some(4000),
            tls_port: Some(4001),
            ..Default::default()
        };
        let mut s = SavedWorld::from_endpoint("A", &wandur_core::Endpoint::new("MUD.example.org", 4001));
        assert!(!is_saved(std::slice::from_ref(&s), &w), "plain 4001 is not the listing");
        s.tls = true;
        assert!(is_saved(std::slice::from_ref(&s), &w));
    }
}
