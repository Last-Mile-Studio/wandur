//! The Channels panel for the active session: an All tab and one tab per channel with unread
//! counts, the messages (time, speaker, coloured text), and a reply box for the selected channel.
//!
//! Updates are incremental: each message is laid out once (a cached galley per message, keyed by
//! its sequence number and the wrap width), so a new message costs one layout however full the
//! panel is, and only the rows in view are painted. The C# panel learned this the hard way: full
//! rebuilds were its biggest cost.

use std::collections::HashMap;
use std::sync::Arc;
use wandur_core::l10n::{S, t, tf};

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, RichText, Ui, vec2};
use wandur_core::channels::ChannelMessage;

use crate::session_tab::SessionTab;
use crate::sessions::AppAction;
use crate::theme::Theme;

#[derive(Default)]
pub struct ChannelsViewState {
    /// Layouts by message, with their width.
    rows: HashMap<u64, (f32, Row)>,
    /// Layouts made in the last frame (1 per new message once warm).
    pub laid_out: usize,
    /// Rows painted in the last frame.
    pub painted: usize,
    draft: String,
    seen_revision: u64,
    theme_name: &'static str,
    /// The selected tab's rows (galley and bottom edge), rebuilt only when the log, the selected
    /// tab or the width changes, so an unchanged panel costs no per-message work per frame.
    list: Vec<(Row, f32)>,
    list_key: Option<(u64, usize, u32)>,
    /// The reply box's hint as drawn in the last frame, and its size (tests).
    pub reply_hint: (String, f32),
    atlas: crate::grid_text::AtlasWatch,
}

/// `HH:MM` in local time.
pub fn local_hhmm(secs: u64) -> String {
    #[cfg(unix)]
    {
        let t = secs as libc::time_t;
        // SAFETY: localtime_r writes into the zeroed struct we own and reads `t`.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        if !unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
            return format!("{:02}:{:02}", tm.tm_hour, tm.tm_min);
        }
    }
    let day = secs % 86_400;
    format!("{:02}:{:02}", day / 3600, day % 3600 / 60)
}

/// Text with SGR colour codes as styled sections (basic, bright, 256 and true colour; bold),
/// in the transcript's palette and default text colour (the messages sit on the transcript's
/// surface, as in C#).
pub fn ansi_job(job: &mut LayoutJob, text: &str, theme: &Theme, font: &FontId) {
    let ansi = theme.ansi;
    let mut fg: Option<Color32> = None;
    let mut bold = false;
    let mut run = String::new();
    let flush = |job: &mut LayoutJob, run: &mut String, fg: Option<Color32>, bold: bool| {
        if run.is_empty() {
            return;
        }
        let color = fg.unwrap_or(theme.terminal_text);
        let mut format = TextFormat::simple(font.clone(), color);
        if bold {
            format.font_id = FontId::new(font.size, crate::fonts::bold_family());
        }
        job.append(run, 0.0, format);
        run.clear();
    };
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() != Some(&'[') {
                chars.next();
                continue;
            }
            chars.next();
            let mut params = String::new();
            let mut end = ' ';
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    end = c;
                    break;
                }
                params.push(c);
            }
            if end != 'm' {
                continue;
            }
            flush(job, &mut run, fg, bold);
            let codes: Vec<u16> = params.split(';').map(|p| p.parse().unwrap_or(0)).collect();
            let mut i = 0;
            while i < codes.len() {
                match codes[i] {
                    0 => {
                        fg = None;
                        bold = false;
                    }
                    1 => bold = true,
                    22 => bold = false,
                    n @ 30..=37 => fg = Some(ansi[(n - 30) as usize + if bold { 8 } else { 0 }]),
                    39 => fg = None,
                    n @ 90..=97 => fg = Some(ansi[(n - 90) as usize + 8]),
                    38 if codes.get(i + 1) == Some(&5) => {
                        fg = codes.get(i + 2).map(|&n| theme.indexed(n.min(255) as u8));
                        i += 2;
                    }
                    38 if codes.get(i + 1) == Some(&2) => {
                        if let (Some(&r), Some(&g), Some(&b)) = (codes.get(i + 2), codes.get(i + 3), codes.get(i + 4)) {
                            fg = Some(Color32::from_rgb(r.min(255) as u8, g.min(255) as u8, b.min(255) as u8));
                        }
                        i += 4;
                    }
                    _ => {}
                }
                i += 1;
            }
        } else if !c.is_control() || c == '\t' {
            run.push(c);
        }
    }
    flush(job, &mut run, fg, bold);
}

/// One laid-out message: the muted time, the speaker and text beside it, and the speaker's name
/// alone (drawn again a fraction of a point to the right, so it reads heavier: the interface face
/// has no bold weight).
/// The last value is the time column's width (the widest of the time and `00:00`), so every
/// message's text starts at the same x.
pub type Row = (Arc<egui::Galley>, Arc<egui::Galley>, Option<Arc<egui::Galley>>, f32);

/// How far the speaker's name is drawn a second time to the right.
const SPEAKER_WEIGHT: f32 = 0.6;

/// The gap between the time and the message, and between two messages (C# UI review, item 4).
const TIME_GAP: f32 = 6.0;
const MESSAGE_GAP: f32 = 5.0;

/// One message as the C# panel shows it since its UI review: chat is prose, so the interface
/// font at 13 rather than monospace; a muted `HH:MM`, then the speaker and the text in the
/// world's colours with a hanging indent (wrapped lines start under the text, not the time).
fn layout(ui: &Ui, m: &ChannelMessage, theme: &Theme, width: f32) -> Row {
    let time = ui.fonts_mut(|f| f.layout_no_wrap(local_hhmm(m.at), FontId::proportional(12.0), theme.muted));
    let column = ui
        .fonts_mut(|f| f.layout_no_wrap("00:00".into(), FontId::proportional(12.0), theme.muted))
        .size()
        .x
        .max(time.size().x)
        .ceil();
    let font = FontId::proportional(13.0);
    let mut job = LayoutJob::default();
    job.wrap.max_width = (width - column - TIME_GAP).max(40.0);
    let mut speaker = None;
    if !m.speaker.is_empty() {
        let f = TextFormat::simple(font.clone(), theme.terminal_text);
        job.append(&m.speaker, 0.0, f.clone());
        job.append(": ", 0.0, f);
        speaker = Some(ui.fonts_mut(|f| f.layout_no_wrap(m.speaker.clone(), font.clone(), theme.terminal_text)));
    }
    // The world's own prefix (tag and speaker) is cut out: the speaker is shown above.
    ansi_job(&mut job, &m.body, theme, &font);
    let body = ui.fonts_mut(|f| f.layout_job(job));
    // A name too long for the first row wraps; the heavier copy is drawn only when it fits.
    let speaker = speaker.filter(|s| body.rows.first().is_some_and(|r| r.rect().width() >= s.size().x));
    (time, body, speaker, column)
}

fn row_height(row: &Row) -> f32 {
    row.0.size().y.max(row.1.size().y)
}

/// A tab of the strip: the title (private channels in bold) and, while it has unread messages,
/// a count in an accent pill.
fn tab_header(ui: &mut Ui, title: &str, private: bool, unread: usize, selected: bool, theme: &Theme) -> egui::Response {
    let font = if private {
        FontId::new(12.0, crate::fonts::bold_family())
    } else {
        FontId::proportional(12.0)
    };
    let title = ui.fonts_mut(|f| f.layout_no_wrap(title.to_string(), font, theme.text));
    let count = (unread > 0).then(|| {
        let label = if unread > 99 {
            "99+".to_string()
        } else {
            unread.to_string()
        };
        ui.fonts_mut(|f| f.layout_no_wrap(label, FontId::proportional(9.0), theme.on_accent()))
    });
    let pill_w = count.as_ref().map_or(0.0, |c| (c.size().x + 10.0).max(14.0));
    let width = 12.0 + title.size().x + if count.is_some() { 3.0 + pill_w } else { 0.0 };
    let (rect, response) = ui.allocate_exact_size(vec2(width, 24.0), egui::Sense::click());
    let name = if unread > 0 {
        format!("{}, {unread}", title.text())
    } else {
        title.text().to_string()
    };
    crate::a11y::toggle(&response, egui::accesskit::Role::Tab, &name, selected);
    if selected {
        ui.painter().rect_filled(rect, 3.0, theme.selection_fill());
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 3.0, theme.hover_fill());
    }
    let x = rect.left() + 6.0;
    ui.painter().galley(
        egui::pos2(x, rect.center().y - title.size().y / 2.0),
        Arc::clone(&title),
        theme.text,
    );
    if let Some(count) = count {
        let pill = egui::Rect::from_min_size(
            egui::pos2(x + title.size().x + 3.0, rect.center().y - 7.0),
            vec2(pill_w, 14.0),
        );
        ui.painter().rect_filled(pill, 7.0, theme.accent);
        ui.painter()
            .galley(pill.center() - count.size() / 2.0, count, theme.on_accent());
    }
    response
}

pub fn show(
    ui: &mut Ui,
    tab: Option<&mut SessionTab>,
    state: &mut ChannelsViewState,
    theme: &Theme,
    actions: &mut Vec<AppAction>,
) {
    let Some(tab) = tab else {
        // No session: the note alone, centred on the panel (C# UI review, item 8).
        let area = ui.available_rect_before_wrap();
        ui.allocate_rect(area, egui::Sense::hover());
        let galley = crate::widgets::clipped(
            ui,
            t(S::ChannelsMirrorNote),
            FontId::proportional(12.0),
            theme.muted,
            (area.width() - 36.0).max(60.0),
            4,
        );
        ui.painter()
            .galley(area.center() - galley.size() / 2.0, galley, theme.muted);
        return;
    };
    if state.atlas.changed(ui) || state.theme_name != theme.name {
        state.rows.clear();
        state.list_key = None;
        state.theme_name = theme.name;
    }
    let connected = tab.is_connected() && !tab.private_input();
    let session = tab.id;
    let log = &mut tab.channels.log;
    // The tabs, with unread counts (All, then private channels, then the rest).
    let mut select = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
        for (i, tab) in log.tabs().iter().enumerate() {
            let response = tab_header(ui, tab.title(), tab.private, tab.unread, log.selected() == i, theme);
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    true,
                    log.selected() == i,
                    tab.title(),
                )
            });
            if response.clicked() {
                select = Some(i);
            }
        }
    });
    if let Some(i) = select {
        log.select(i);
        state.draft.clear();
    }
    let selected = log.selected_tab();
    state.laid_out = 0;
    state.painted = 0;
    // The note while nothing has arrived, on the panel (C# `ChannelsMirrorNote`).
    let note = log.is_empty();
    // The messages and the reply bar sit on the transcript's surface, as in C#.
    let reply_h = 49.0;
    let area = ui.available_rect_before_wrap();
    ui.allocate_rect(area, egui::Sense::hover());
    ui.painter().rect_filled(area, 0.0, theme.terminal);
    let bar = egui::Rect::from_min_max(
        egui::pos2(area.left(), (area.bottom() - reply_h).max(area.top())),
        area.max,
    );
    let list_rect = egui::Rect::from_min_max(area.min, egui::pos2(area.right(), bar.top()));
    ui.painter()
        .hline(bar.x_range(), bar.top(), egui::Stroke::new(1.0, theme.border));
    let inner = list_rect.shrink2(vec2(10.0, 8.0));
    // The note while nothing has arrived, centred on the empty panel (C# UI review, item 8).
    if note {
        let galley = crate::widgets::clipped(
            ui,
            t(S::ChannelsMirrorNote),
            FontId::proportional(12.0),
            theme.muted,
            (inner.width() - 24.0).max(60.0),
            4,
        );
        ui.painter()
            .galley(list_rect.center() - galley.size() / 2.0, galley, theme.muted);
    }
    let width = (inner.width() - 10.0).max(80.0);
    if !log.is_empty() && inner.height() > 0.0 {
        // Forget layouts of messages that left every tab.
        if state.seen_revision != log.revision {
            state.seen_revision = log.revision;
            let oldest = log
                .tabs()
                .iter()
                .filter_map(|t| t.messages.front().map(|m| m.seq))
                .min()
                .unwrap_or(0);
            state.rows.retain(|seq, _| *seq >= oldest);
        }
        let spacing = MESSAGE_GAP;
        let key = (log.revision, log.selected(), width.to_bits());
        if state.list_key != Some(key) {
            state.list_key = Some(key);
            state.list.clear();
            let mut y = 0.0;
            for m in &selected.messages {
                let entry = state.rows.get(&m.seq).filter(|(w, _)| (*w - width).abs() < 0.5);
                let row = match entry {
                    Some((_, r)) => r.clone(),
                    None => {
                        let r = layout(ui, m, theme, width);
                        state.rows.insert(m.seq, (width, r.clone()));
                        state.laid_out += 1;
                        r
                    }
                };
                y += row_height(&row) + spacing;
                state.list.push((row, y));
            }
        }
        let total = state.list.last().map_or(0.0, |(_, y)| *y);
        let list = &state.list;
        let painted = &mut state.painted;
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            egui::ScrollArea::vertical()
                .id_salt(("channel-list", session, log.selected()))
                .max_height(inner.height())
                .min_scrolled_height(inner.height())
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show_viewport(ui, |ui, viewport| {
                    let (rect, _) = ui.allocate_exact_size(vec2(width, total), egui::Sense::hover());
                    // Rows are sorted by their bottom edge: start at the first one reaching the view.
                    let first = list.partition_point(|(_, bottom)| *bottom < viewport.min.y);
                    for (row, bottom) in &list[first..] {
                        let top = bottom - row_height(row) - spacing;
                        if top > viewport.max.y {
                            break;
                        }
                        let (time, body, speaker, column) = row;
                        // The time sits on the first line's baseline band.
                        let dy = (body.rows.first().map_or(0.0, |r| r.height()) - time.size().y).max(0.0) / 2.0;
                        ui.painter()
                            .galley(rect.min + vec2(0.0, top + dy), Arc::clone(time), theme.muted);
                        let text_at = rect.min + vec2(column + TIME_GAP, top);
                        ui.painter().galley(text_at, Arc::clone(body), theme.terminal_text);
                        if let Some(speaker) = speaker {
                            ui.painter().galley(
                                text_at + vec2(SPEAKER_WEIGHT, 0.0),
                                Arc::clone(speaker),
                                theme.terminal_text,
                            );
                        }
                        *painted += 1;
                    }
                });
        });
    }
    // The reply box: it speaks on the shown channel, never on All, never while input is private.
    let can_reply = !selected.is_all && selected.reply("x").is_some() && connected;
    let (hint, short) = if selected.is_all {
        (
            t(S::ChannelsReplyAllHint).to_string(),
            t(S::ChannelsReplyAllShort).to_string(),
        )
    } else if selected.reply("x").is_none() {
        (
            t(S::ChannelsReplyUnknown).to_string(),
            t(S::ChannelsReplyUnknownShort).to_string(),
        )
    } else {
        (
            tf(S::ChannelsReplyPlaceholder, &[&selected.title()]),
            tf(S::ChannelsReplyPlaceholderShort, &[&selected.title()]),
        )
    };
    ui.scope_builder(egui::UiBuilder::new().max_rect(bar.shrink2(vec2(8.0, 6.0))), |ui| {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let field = ui.available_width() - 70.0;
            // The whole hint when it fits (at 13 points, else 12), else the short wording; an
            // ellipsis only when even that does not fit, with the whole hint as a tooltip.
            let (shown, size, fits) = crate::widgets::fit_text(ui, &[&hint, &short], &[13.0, 12.0], field - 22.0);
            let edit = egui::TextEdit::singleline(&mut state.draft)
                .id_salt(("channel-reply", session))
                .hint_text(RichText::new(shown).size(size))
                .char_limit(1024)
                .font(FontId::proportional(13.0))
                .text_color(theme.terminal_text)
                .background_color(theme.terminal)
                .margin(egui::Margin::symmetric(10, 9))
                .desired_width(field);
            let mut response = ui.add_enabled(can_reply, edit);
            crate::a11y::label(&response, &hint);
            if (!fits || shown != hint) && state.draft.is_empty() {
                response = response.on_hover_text(&hint).on_disabled_hover_text(&hint);
            }
            state.reply_hint = (shown.to_string(), size);
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let send = ui.add_enabled(
                can_reply && !state.draft.trim().is_empty(),
                egui::Button::new(RichText::new(t(S::ChannelsSend)).size(13.0)).min_size(vec2(64.0, 36.0)),
            );
            if (send.clicked() || enter)
                && can_reply
                && let Some(command) = selected.reply(state.draft.trim())
                && !state.draft.trim().is_empty()
            {
                actions.push(AppAction::SendCommand(session, command));
                state.draft.clear();
                response.request_focus();
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use wandur_core::channels::ChannelLog;

    fn sections(text: &str) -> Vec<(String, Color32)> {
        let theme = Theme::ember();
        let mut job = LayoutJob::default();
        ansi_job(&mut job, text, &theme, &FontId::monospace(13.0));
        job.sections
            .iter()
            .map(|s| {
                (
                    egui::epaint::text::ByteRangeExt::slice(&s.byte_range, &job.text).to_string(),
                    s.format.color,
                )
            })
            .collect()
    }

    #[test]
    fn sgr_colours_become_sections() {
        let theme = Theme::ember();
        let s = sections("plain \u{1b}[31mred\u{1b}[0m \u{1b}[1;32mbright\u{1b}[38;5;196mcube\u{1b}[38;2;1;2;3mrgb");
        assert_eq!(s[0], ("plain ".into(), theme.terminal_text));
        assert_eq!(s[1], ("red".into(), theme.ansi[1]));
        assert_eq!(s[3], ("bright".into(), theme.ansi[10]));
        assert_eq!(s[4].1, Color32::from_rgb(255, 0, 0));
        assert_eq!(s[5].1, Color32::from_rgb(1, 2, 3));
    }

    /// A message row in a narrow panel: the time in a column of its own (every row's text starts
    /// at the same x), the speaker drawn heavier, the text wrapped inside the panel with a hanging
    /// indent (the rows after the first start under the text, not under the time), colours kept.
    #[test]
    fn a_message_row_has_a_time_column_a_heavier_speaker_and_a_hanging_indent() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::ember();
        let mut log = ChannelLog::default();
        let long = log.add(
            "gossip",
            "Talek",
            "has anyone restocked with \u{1b}[31mhunted goods\u{1b}[0m at the market yet, or are the traders still waiting",
            0,
        );
        let short = log.add("ooc", "Ilsa", "yes", 11 * 3600 + 7 * 60);
        for width in [220.0, 260.0, 400.0] {
            let mut rows = Vec::new();
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                rows.push(layout(ui, &long, &theme, width));
                rows.push(layout(ui, &short, &theme, width));
            });
            out.textures_delta.clear();
            let (_, body, speaker, column) = &rows[0];
            assert_eq!(*column, rows[1].3, "one time column for every row");
            assert!(*column >= rows[0].0.size().x && *column >= rows[1].0.size().x);
            assert!(speaker.is_some(), "the speaker is drawn heavier");
            assert!(
                body.size().x <= width - column - TIME_GAP + 0.5,
                "{width}: {}",
                body.size().x
            );
            assert!(body.rows.len() >= 2, "wrapped at {width}");
            // Every row of the text starts at the text's left edge: the indent hangs.
            assert!(body.rows.iter().all(|r| r.rect().left() < 1.0));
            // The red run keeps its colour.
            assert!(body.job.sections.iter().any(|s| s.format.color == theme.ansi[1]));
        }
    }

    /// The reply box's hint fits: the whole wording when there is room (at 13 points, or 12),
    /// the short one in a narrow panel, never cut short at the widths the panel is used at.
    #[test]
    fn the_reply_hint_fits_narrow_panels() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::ember();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = wandur_core::Endpoint::new("127.0.0.1", listener.local_addr().unwrap().port());
        let mut tab = SessionTab::open(1, endpoint, &Default::default(), Arc::new(|| {}));
        tab.channels.log.add("gossip", "Ann", "hello", 0);
        let mut actions = Vec::new();
        let mut hint_at = |width: f32| {
            let mut state = ChannelsViewState::default();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(width, 400.0))),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| show(ui, Some(&mut tab), &mut state, &theme, &mut actions));
            });
            out.textures_delta.clear();
            state.reply_hint
        };
        let full = t(S::ChannelsReplyAllHint);
        let short = t(S::ChannelsReplyAllShort);
        assert_eq!(hint_at(400.0), (full.to_string(), 13.0));
        let (text, size) = hint_at(260.0);
        assert!(text == full || text == short, "{text}");
        assert!(size >= 12.0);
        assert_eq!(hint_at(220.0).0, short);
    }

    /// Drive the panel headlessly: a new message costs one layout, and a full panel paints only
    /// the rows in view.
    #[test]
    fn updates_are_incremental_and_painting_is_bounded() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::ember();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = wandur_core::Endpoint::new("127.0.0.1", listener.local_addr().unwrap().port());
        let mut tab = SessionTab::open(1, endpoint, &Default::default(), Arc::new(|| {}));
        let mut log = ChannelLog::default();
        for i in 0..600 {
            log.add(
                if i % 3 == 0 { "newbie" } else { "gossip" },
                "Ann",
                &format!("message {i}"),
                0,
            );
        }
        tab.channels.log = log;
        let mut state = ChannelsViewState::default();
        let mut actions = Vec::new();
        let mut frame = |tab: &mut SessionTab, state: &mut ChannelsViewState| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(360.0, 400.0))),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| show(ui, Some(tab), state, &theme, &mut actions));
            });
            out.textures_delta.clear();
        };
        frame(&mut tab, &mut state);
        assert_eq!(
            state.laid_out, 500,
            "the All tab holds 500 messages, each laid out once"
        );
        assert!(state.painted < 40, "only rows in view are painted: {}", state.painted);
        frame(&mut tab, &mut state);
        assert_eq!(state.laid_out, 0, "nothing new, nothing laid out");
        tab.channels.log.add("gossip", "Bob", "one more", 0);
        frame(&mut tab, &mut state);
        assert_eq!(state.laid_out, 1, "one new message, one layout");
        assert!(state.rows.len() <= 601, "layouts of dropped messages are released");
    }
}
