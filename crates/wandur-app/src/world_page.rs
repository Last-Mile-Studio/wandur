//! A world's own page, as the C# browser's (and the site's `/worlds/{id}`): breadcrumbs, the hero
//! picture with the name and tagline over it, a chips bar with Connect and Add to my worlds, then
//! About beside World details (stacked on a narrow tab).

use egui::{Color32, CornerRadius, FontId, RichText, Stroke, Ui, vec2};
use wandur_core::directory::listing::format_host_port;
use wandur_core::directory::time::{ago, date_text, year_of};
use wandur_core::directory::{Artwork, WorldListing};
use wandur_core::l10n::{S, t, tf};

use crate::artwork::{ArtState, Target};
use crate::directory_view::{DirContext, DirectoryView, art_request, is_saved};
use crate::sessions::AppAction;
use crate::theme::Theme;
use crate::widgets;

/// Below this width the page is one column.
const NARROW_BELOW: f32 = 760.0;
/// The hero's fixed ink: it sits on a photograph, so it does not follow the theme (as in C#).
const HERO_TITLE: Color32 = Color32::from_rgb(0xE7, 0xEE, 0xF6);
const HERO_TAGLINE: Color32 = Color32::from_rgb(0xC9, 0xD4, 0xE0);
const HERO_SCRIM: Color32 = Color32::from_rgba_premultiplied(0x0B, 0x12, 0x1E, 0xE6);

/// Draw the page. Returns true when the person asked to go back to the list.
pub fn show(ui: &mut Ui, w: &WorldListing, view: &mut DirectoryView, cx: &mut DirContext<'_>) -> bool {
    let theme = cx.theme;
    let mut back = false;
    egui::ScrollArea::vertical()
        .id_salt(("world-page", &w.id))
        .auto_shrink(false)
        .show(ui, |ui| {
            let width = ui.available_width() - 14.0;
            let narrow = width < NARROW_BELOW;
            ui.set_width(width);
            // Breadcrumbs.
            ui.horizontal(|ui| {
                if widgets::link(ui, "< All worlds", theme.muted, 14.0, false).clicked() {
                    back = true;
                }
                if !w.features.theme.is_empty() {
                    ui.label(RichText::new("/").color(theme.muted));
                    ui.label(RichText::new(&w.features.theme).color(theme.muted));
                }
                ui.label(RichText::new("/").color(theme.muted));
                ui.label(RichText::new(&w.name).size(14.0));
            });
            ui.add_space(10.0);
            hero(ui, w, width, if narrow { 240.0 } else { 320.0 }, cx);
            ui.add_space(8.0);
            chips_bar(ui, w, view, cx);
            ui.add_space(14.0);
            if narrow {
                about(ui, w, theme, width);
                ui.add_space(16.0);
                details(ui, w, view, cx, width);
            } else {
                let right = 380.0;
                let left = width - right - 18.0;
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(left);
                        about(ui, w, theme, left);
                    });
                    ui.add_space(18.0);
                    ui.vertical(|ui| {
                        ui.set_width(right);
                        details(ui, w, view, cx, right);
                    });
                });
            }
            ui.add_space(20.0);
        });
    back
}

fn hero(ui: &mut Ui, w: &WorldListing, width: f32, height: f32, cx: &mut DirContext<'_>) {
    let theme = cx.theme;
    let ppp = ui.ctx().pixels_per_point();
    let target = Target {
        width: (1024.0 * ppp).round() as u32,
        height: (320.0 * ppp).round() as u32,
        cover: true,
    };
    let state = art_request(w, cx.base, "hero", target, Some("hero"), Some("1024"))
        .map_or(ArtState::Failed, |r| cx.art.request(&r));
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), egui::Sense::hover());
    let radius = CornerRadius::same(12);
    widgets::plate(ui, rect, radius, &state, &w.name, theme, 96.0);
    let painter = ui.painter_at(rect);
    // The scrim: transparent at the top third, the site's night ink at the bottom.
    let mut mesh = egui::Mesh::default();
    let top = rect.top() + rect.height() * 0.35;
    let clear = Color32::from_rgba_premultiplied(0, 0, 0, 0);
    mesh.colored_vertex(egui::pos2(rect.left(), top), clear);
    mesh.colored_vertex(egui::pos2(rect.right(), top), clear);
    mesh.colored_vertex(rect.left_bottom(), HERO_SCRIM);
    mesh.colored_vertex(rect.right_bottom(), HERO_SCRIM);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    painter.add(egui::Shape::mesh(mesh));
    let narrow = width < NARROW_BELOW;
    let margin = if narrow { 18.0 } else { 30.0 };
    let title_size = if narrow { 34.0 } else { 44.0 };
    let tagline = w.summary.split_whitespace().collect::<Vec<_>>().join(" ");
    let tag_g = (!tagline.is_empty()).then(|| {
        widgets::clipped(
            ui,
            &tagline,
            FontId::proportional(if narrow { 17.0 } else { 19.0 }),
            HERO_TAGLINE,
            (width - 2.0 * margin).min(720.0),
            2,
        )
    });
    let title_g = widgets::clipped(
        ui,
        &w.name,
        FontId::proportional(title_size),
        HERO_TITLE,
        width - 2.0 * margin,
        2,
    );
    let mut y = rect.bottom() - margin * 0.8;
    if let Some(g) = &tag_g {
        y -= g.size().y;
        painter.galley(egui::pos2(rect.left() + margin, y), g.clone(), HERO_TAGLINE);
        y -= 8.0;
    }
    y -= title_g.size().y;
    painter.galley(egui::pos2(rect.left() + margin, y), title_g, HERO_TITLE);
    let caption = match (&state, w.preferred_artwork()) {
        (ArtState::Loading, _) => t(S::LoadingSuppliedArtwork).to_string(),
        (_, Some(Artwork::Supplied)) => tf(S::SuppliedArtwork, &[&w.source.name]),
        (_, Some(Artwork::Generated)) => t(S::AIIllustrationInspiredByThisWorldSDescription).to_string(),
        _ => String::new(),
    };
    if !caption.is_empty() {
        ui.label(RichText::new(caption).size(11.0).color(theme.muted));
    }
}

fn chips_bar(ui: &mut Ui, w: &WorldListing, view: &mut DirectoryView, cx: &mut DirContext<'_>) {
    let theme = cx.theme;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        let mut chips: Vec<&str> = Vec::new();
        for c in std::iter::once(w.features.theme.as_str()).chain(w.tags.iter().map(String::as_str)) {
            let c = c.trim();
            if !c.is_empty() && !chips.iter().any(|x| x.eq_ignore_ascii_case(c)) {
                chips.push(c);
            }
        }
        for c in chips.iter().take(5) {
            widgets::pill(ui, c, theme, 14.0);
        }
        if w.beginner_friendly == Some(true) {
            widgets::beginner(ui, theme, 14.0);
        }
        if w.is_adult() {
            widgets::pill(ui, t(S::AdultChip), theme, 14.0);
        }
    });
    // Below the chips, as in C#: who is online, then Connect and Add to my worlds.
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        if w.is_online() {
            let text = match w.live_players(cx.now) {
                Some(1) => tf(S::PlayersOnlineCountOne, &[&1]),
                Some(n) => tf(S::PlayersOnlineCount, &[&n]),
                None => t(S::StatusOnline).to_string(),
            };
            widgets::live(ui, &text, theme, 15.0);
        }
        let connect = egui::Button::new(RichText::new(t(S::Connect2)).size(15.0).color(theme.on_accent()))
            .fill(theme.accent)
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(128.0, 36.0));
        if ui.add_enabled(w.can_connect(), connect).clicked() {
            cx.actions.push(AppAction::SaveListing {
                id: w.id.clone(),
                tls: view.use_tls,
                connect: true,
            });
        }
        let saved = is_saved(&cx.settings.worlds, w);
        let add = egui::Button::new(RichText::new(t(if saved { S::WorldSaved } else { S::AddToMyWorlds })).size(14.0))
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(0.0, 36.0));
        if ui.add_enabled(w.can_connect() && !saved, add).clicked() {
            cx.actions.push(AppAction::SaveListing {
                id: w.id.clone(),
                tls: view.use_tls,
                connect: false,
            });
        }
    });
    ui.add_space(6.0);
    if !view.feedback.is_empty() {
        ui.label(RichText::new(&view.feedback).size(12.0));
    }
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(1.0, theme.border));
}

fn about(ui: &mut Ui, w: &WorldListing, theme: &Theme, width: f32) {
    widgets::card_frame(theme).show(ui, |ui| {
        ui.set_width(width - 42.0);
        widgets::heading(ui, &tf(S::AboutWorld, &[&w.name]), 21.0);
        ui.add_space(8.0);
        let description = w.description.trim();
        if description.is_empty() {
            ui.label(
                RichText::new(t(S::NoFurtherDescriptionProvided))
                    .size(15.0)
                    .color(theme.muted),
            );
        } else {
            for paragraph in description.split("\n\n").map(str::trim).filter(|p| !p.is_empty()) {
                ui.label(RichText::new(paragraph).size(15.0));
                ui.add_space(6.0);
            }
        }
        let place: Vec<(&str, &str)> = [
            (t(S::Roleplaying), w.features.roleplaying.as_str()),
            (t(S::PlayerKilling), w.features.player_killing.as_str()),
            (t(S::WorldSize), w.features.world_size.as_str()),
        ]
        .into_iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .collect();
        if !place.is_empty() {
            ui.add_space(8.0);
            ui.separator();
            widgets::heading(ui, t(S::FindYourPlace), 17.0);
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 28.0;
                for (name, value) in place {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(name).size(15.0).strong());
                        ui.label(RichText::new(value).size(14.0).color(theme.muted));
                    });
                }
            });
        }
    });
}

fn fact_grid(ui: &mut Ui, id: &str, facts: &[(String, String)], theme: &Theme, size: f32) {
    egui::Grid::new(id).num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        for (name, value) in facts {
            ui.label(RichText::new(name).size(size - 1.0).color(theme.muted));
            ui.add(egui::Label::new(RichText::new(value).size(size)).wrap());
            ui.end_row();
        }
    });
}

fn details(ui: &mut Ui, w: &WorldListing, view: &mut DirectoryView, cx: &mut DirContext<'_>, width: f32) {
    let theme = cx.theme;
    let now = cx.now;
    widgets::card_frame(theme).show(ui, |ui| {
        ui.set_width(width - 42.0);
        widgets::heading(ui, t(S::WorldDetails), 21.0);
        ui.add_space(8.0);
        let status = if w.availability.archived == Some(true) {
            t(S::ArchivedListing)
        } else {
            match w.availability.online {
                Some(true) => t(S::StatusOnline),
                Some(false) => t(S::StatusOffline),
                None => t(S::AvailabilityUnknown),
            }
        };
        let mut facts: Vec<(String, String)> = vec![(t(S::WorldStatus).into(), status.into())];
        let mut add = |name: &str, value: Option<String>| {
            if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
                facts.push((name.into(), v));
            }
        };
        add(t(S::Language), Some(w.features.language.clone()));
        add(
            t(S::PlayStyle),
            (!w.features.roleplaying.is_empty())
                .then(|| tf(S::RoleplayStyle, &[&w.features.roleplaying.to_lowercase()])),
        );
        add(t(S::Established), w.established_at.map(|t| year_of(t).to_string()));
        add(t(S::Codebase), Some(w.features.codebase.clone()));
        add(t(S::PlayerKilling), Some(w.features.player_killing.clone()));
        let players = match (w.live_players(now), w.population.observed_at) {
            (Some(n), Some(at)) => Some(tf(S::PlayersObservedAgo, &[&n, &ago(at, now)])),
            _ => w.population_history(now),
        };
        add(t(S::LastObservedPlayers), players);
        fact_grid(ui, "world-facts", &facts, theme, 15.0);
        ui.add_space(10.0);
        ui.label(RichText::new(t(S::ConnectWithAnyClient)).size(15.0).strong());
        let address = match (view.use_tls, w.tls_port) {
            (true, Some(port)) if w.can_connect() => format_host_port(&w.host, port),
            _ => w.address(),
        };
        ui.horizontal(|ui| {
            egui::Frame::new()
                .fill(theme.shell)
                .stroke(Stroke::new(1.0, theme.border))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(&address).monospace().size(14.0)).selectable(true));
                });
            if w.can_connect()
                && widgets::link(ui, t(S::Copy), theme.accent, 13.0, false)
                    .on_hover_text(t(S::CopyAddress))
                    .clicked()
            {
                ui.ctx().copy_text(address.clone());
            }
        });
        if w.tls_port.is_some() && w.can_connect() {
            ui.add_enabled(w.port.is_some(), egui::Checkbox::new(&mut view.use_tls, t(S::UseTLS)));
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            for (label, url) in [
                (t(S::Website).to_string(), &w.website_url),
                (t(S::Discord).to_string(), &w.discord_url),
                (t(S::PlayInBrowser).to_string(), &w.play_url),
                (tf(S::SourceListing, &[&w.source.name]), &w.source.listing_url),
            ] {
                if (url.starts_with("https://") || url.starts_with("http://"))
                    && widgets::link(ui, &label, theme.accent, 13.0, false)
                        .on_hover_text(url.as_str())
                        .clicked()
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url.as_str()));
                }
            }
        });
        ui.add_space(6.0);
        ui.separator();
        let source = if w.source.name.is_empty() {
            t(S::Directory)
        } else {
            w.source.name.as_str()
        };
        let mut quiet: Vec<(String, String)> = Vec::new();
        let mut q = |name: String, value: Option<String>| {
            if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
                quiet.push((name, v));
            }
        };
        q(t(S::GameType).into(), Some(w.features.kind.clone()));
        q(t(S::Location).into(), Some(w.features.location.clone()));
        q(t(S::Development).into(), Some(w.features.development_status.clone()));
        q(
            t(S::TLSConnection).into(),
            w.tls_port.map(|p| format_host_port(&w.host, p)),
        );
        q(
            t(S::DirectoryRating).into(),
            (w.community.rating.is_some() || w.community.rating_count.is_some()).then(|| w.rating_summary()),
        );
        q(tf(S::SourceRank, &[&source]), w.community.rank.map(|r| format!("#{r}")));
        q(
            tf(S::SourceReviews, &[&source]),
            w.community.review_count.map(|r| r.to_string()),
        );
        q(
            t(S::MonthlyVotes).into(),
            w.community.monthly_votes.map(|r| r.to_string()),
        );
        q(
            t(S::ListedPlayerRange).into(),
            Some(w.population.reported_range.clone()),
        );
        q(
            t(S::AveragePlayers).into(),
            w.population.average_count.map(|a| format!("{a:.1}")),
        );
        q(t(S::StatusChecked).into(), w.availability.checked_at.map(date_text));
        q(t(S::LastReached).into(), w.availability.last_online_at.map(date_text));
        if w.availability.archived == Some(true) {
            q(t(S::ArchiveReason).into(), Some(w.availability.archive_reason.clone()));
        }
        q(t(S::ListingUpdated).into(), w.source.updated_at.map(date_text));
        fact_grid(ui, "world-quiet-facts", &quiet, theme, 13.0);
        ui.add_space(6.0);
        let attribution = match w.source.updated_at {
            Some(at) => tf(S::DetailsFromUpdated, &[&source, &date_text(at)]),
            None => tf(S::DetailsFrom, &[&source]),
        };
        ui.label(RichText::new(attribution).size(12.0).color(theme.muted));
    });
}
