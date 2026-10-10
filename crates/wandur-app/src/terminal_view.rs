//! Drawing a session: status line, the terminal grid (with the live view under it while scrolled
//! back), the composer (Look, Commands, the command box with its ghost completion, Send) and the
//! footer.
//!
//! The grid is painted directly from the session's `alacritty_terminal` cells: only the rows on
//! screen are visited, and each row becomes a few text runs. Plain runs (ASCII, Latin and box
//! drawing, which the bundled font covers at the cell width) are laid out as one galley per style
//! change; any other character is placed on its own at its cell position so that fallback glyphs
//! with other advances, wide characters and combining marks cannot push the rest of the row off
//! the grid. egui caches galleys by content, so unchanged runs cost a hash lookup per frame.
//!
//! Selection, scrolling and copy work on the grid, not on the widgets: a drag selects cells across
//! the whole scrollback (dragging past the top or bottom edge scrolls), double click selects a
//! word, triple click a line, Shift+click extends, and copying joins soft-wrapped rows.

use std::time::Instant;

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, FontId, Key, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use wandur_core::l10n::{S, t, tf};
use wandur_core::settings::TAIL_SHARES;
use wandur_term::alacritty_terminal::index::Point;
use wandur_term::alacritty_terminal::index::Side;
use wandur_term::alacritty_terminal::term::cell::{Cell, Flags};
use wandur_term::{BLINK, DisplayRow, SelectionKind, TermSize, Terminal, WrapOptions};

use crate::fonts::{FallbackFonts, bold_family};
use crate::grid_text::{self, GridText};
use crate::session_tab::{SessionTab, Status};
use crate::theme::Theme;
use crate::widgets::Icon;

/// Terminal fonts: the regular and bold faces at the chosen size.
#[derive(Clone, Debug)]
pub struct TermFonts {
    pub regular: FontId,
    pub bold: FontId,
}

impl TermFonts {
    pub fn new(size: f32) -> Self {
        Self {
            regular: FontId::monospace(size),
            bold: FontId::new(size, bold_family()),
        }
    }
}

const SCROLLBAR: f32 = 10.0;
/// Height of the divider between the transcript and the live view.
const DIVIDER: f32 = 4.0;
/// Rows the live view draws at most, whatever its height.
pub const MAX_TAIL_ROWS: usize = 400;
/// Half a blink cycle (the C# timer's interval).
const BLINK_HALF: f64 = 0.53;

/// View state of one terminal that is not part of the session model.
#[derive(Debug, Default)]
pub struct TerminalViewState {
    /// Size of one cell in points.
    pub cell: Vec2,
    /// Fractional rows of wheel scrolling not applied yet.
    scroll_rest: f32,
    /// A selection drag is in progress.
    selecting: bool,
    scrollbar_drag: bool,
    /// Rows painted in the last frame (the transcript, and the live view under it).
    pub rows_drawn: usize,
    pub tail_rows_drawn: usize,
    /// Text runs painted in the last frame.
    pub runs_drawn: usize,
    /// Reused per row.
    runs: Vec<RunSpec>,
    text: String,
    /// The transcript's rows on screen (word wrapped), and what they were laid out from:
    /// revision, scroll offset, columns, rows and options. Laid out again only when one changes.
    pub display: Vec<DisplayRow>,
    display_key: Option<(u64, usize, usize, usize, WrapOptions)>,
    /// The live view's rows (and the output strip's), laid out each frame they show.
    tail_display: Vec<DisplayRow>,
    /// The plain text of the visible rows as one mesh.
    pub grid: GridText,
    /// The same for the live view.
    pub tail_grid: GridText,
    /// The divider's share while it is being dragged.
    drag_share: Option<f32>,
    /// A web address waiting for Open or Cancel.
    pub pending_link: Option<String>,
    /// The link under the pointer, by (row, column, revision, scroll offset), so the line is
    /// read only when the pointer or the text moved.
    hover: Option<(HoverKey, Option<String>)>,
    /// Blinking text was drawn in the last frame.
    pub blink_seen: bool,
    /// The draft Escape put the ghost away for; it comes back when the draft changes.
    dismissed: Option<String>,
    /// The ghost text shown after the draft in the last frame.
    pub ghost: Option<String>,
    /// Where the last frame drew the transcript, the divider and the live view.
    pub transcript_rect: Option<Rect>,
    pub divider_rect: Option<Rect>,
    pub tail_rect: Option<Rect>,
    /// Where the link bar's Open and Cancel buttons were drawn (tests).
    pub link_buttons: Option<(Rect, Rect)>,
    /// Play or Diagnostics (the footer's view tabs).
    pub page: Page,
    pub diagnostics: crate::diagnostics_view::DiagnosticsViewState,
    /// The vitals cards drawn in the last frame.
    pub vitals: Vec<crate::vitals_view::Card>,
    /// What the open transcript menu acts on (taken when it was opened by a right click).
    pub menu: Option<MenuTarget>,
    /// The footer's Scripts menu is open.
    pub scripts_menu: bool,
    /// The footer's Agent menu is open, with its Recent decisions and More controls.
    pub agent_menu: bool,
    pub agent_activity_open: bool,
    pub agent_more_open: bool,
    /// The session rail beside the transcript (script panels).
    pub rail: crate::panel_view::RailState,
    /// Play and the map side by side (on the Play and Map pages).
    pub split: bool,
    /// The map's share of the width side by side (0: [`DEFAULT_SPLIT_SHARE`]).
    pub split_share: f32,
    /// The output strip's rows under the full map (0: [`DEFAULT_STRIP_ROWS`]).
    pub strip_rows: usize,
    /// The plain text of the output strip as one mesh.
    pub strip_grid: GridText,
    /// Rows painted in the output strip in the last frame (tests).
    pub strip_rows_drawn: usize,
    /// Where the last frame drew the output strip and where it left room for the map.
    pub strip_rect: Option<Rect>,
    pub map_rect: Option<Rect>,
}

impl TerminalViewState {
    /// The map's share of the width side by side.
    pub fn split_share(&self) -> f32 {
        if self.split_share > 0.0 {
            self.split_share.clamp(*SPLIT_SHARES.start(), *SPLIT_SHARES.end())
        } else {
            DEFAULT_SPLIT_SHARE
        }
    }

    /// The output strip's rows.
    pub fn strip_rows(&self) -> usize {
        if self.strip_rows == 0 {
            DEFAULT_STRIP_ROWS
        } else {
            self.strip_rows.clamp(*STRIP_ROWS.start(), *STRIP_ROWS.end())
        }
    }

    /// The map is on screen: the Map page, or Play and Map side by side.
    pub fn map_shown(&self) -> bool {
        self.page == Page::Map || (self.split && self.page == Page::Play)
    }

    /// Show the Map page (`true`) or Play, leaving side by side.
    pub fn show_map(&mut self, on: bool) {
        self.split = false;
        self.page = if on { Page::Map } else { Page::Play };
    }

    /// Turn side by side on or off; on shows Play and Map, off returns to the page shown.
    pub fn set_split(&mut self, on: bool) {
        self.split = on;
        if on && self.page == Page::Diagnostics {
            self.page = Page::Play;
        }
    }
}

/// What a right click on the transcript or the live view found: the line under the pointer
/// (soft wraps joined) and the selection, if any.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuTarget {
    pub line: String,
    pub selection: Option<String>,
}

impl MenuTarget {
    /// The example for Mark as channel: the first selected line, or the line under the pointer,
    /// with the second selected line as the narrowing example (C#).
    pub fn examples(&self) -> (String, Option<String>) {
        let mut selected = self
            .selection
            .iter()
            .flat_map(|s| s.lines())
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty());
        match selected.next() {
            Some(first) => (first.to_string(), selected.next().map(str::to_string)),
            None => (self.line.trim_end().to_string(), None),
        }
    }
}

/// The session's views: the transcript with the composer, the full map, or Diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    #[default]
    Play,
    /// The full map, with the newest output in a strip under it.
    Map,
    Diagnostics,
}

/// The output strip's rows when the person has not chosen (the last few lines and the prompt).
pub const DEFAULT_STRIP_ROWS: usize = 5;
/// The output strip's rows at least and at most.
pub const STRIP_ROWS: std::ops::RangeInclusive<usize> = 1..=20;
/// The map's share of the width when Play and Map are side by side and nothing was chosen.
pub const DEFAULT_SPLIT_SHARE: f32 = 0.5;
/// The map's share of the width side by side, at least and at most.
pub const SPLIT_SHARES: std::ops::RangeInclusive<f32> = 0.2..=0.8;
/// Width of the divider between Play and the map side by side.
const SPLIT_DIVIDER: f32 = 5.0;
/// The footer's side-by-side toggle (its id among panel actions).
pub const SPLIT_ACTION: &str = "SessionSplit";

/// Where the pointer was and what the grid held: row, column, revision, scroll offset.
type HoverKey = (usize, usize, u64, usize);

/// How the transcript is drawn this frame (from the settings).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintOptions {
    /// The live view's share of the output area while scrolled back (0: no live view).
    pub tail_share: f32,
    /// Blinking text blinks; otherwise it shows steady.
    pub blink: bool,
    /// The composer offers completions.
    pub suggestions: bool,
    /// The System skin's quiet Send button (C# UI review, item 23): no filled accent.
    pub quiet_send: bool,
    /// Word wrapping of long lines (Settings > Terminal).
    pub wrap: WrapOptions,
    /// The widest the transcript gets in columns, centred in a wider pane (Settings > Terminal,
    /// Limit text width); `None` fills the pane.
    pub max_columns: Option<usize>,
}

/// Columns continuation rows are indented by when "Indent wrapped lines" is on.
pub const WRAP_INDENT: usize = 2;

/// The word wrapping the settings ask for.
pub fn wrap_options(settings: &wandur_core::settings::Settings) -> WrapOptions {
    WrapOptions {
        words: settings.wrap_words,
        indent: if settings.wrap_indent { WRAP_INDENT } else { 0 },
    }
}

impl Default for PaintOptions {
    fn default() -> Self {
        Self {
            tail_share: wandur_core::settings::DEFAULT_TAIL_SHARE,
            blink: false,
            suggestions: true,
            quiet_send: false,
            wrap: WrapOptions { words: true, indent: 0 },
            max_columns: None,
        }
    }
}

impl PaintOptions {
    pub fn from_settings(settings: &wandur_core::settings::Settings) -> Self {
        Self {
            tail_share: settings.scroll_tail_share,
            blink: settings.allow_blinking_text,
            suggestions: settings.composer_suggestions,
            quiet_send: crate::skin::SkinId::from_name(&settings.skin) == crate::skin::SkinId::System,
            wrap: wrap_options(settings),
            max_columns: settings
                .limit_text_width
                .then_some(settings.text_width_columns as usize),
        }
    }
}

/// What the person did in the terminal this frame.
#[derive(Debug, Default, PartialEq)]
pub struct TerminalOutput {
    pub latest_clicked: bool,
    /// A click or selection ended: give the keyboard back to the input line.
    pub refocus_input: bool,
    /// The grid changed size (columns, rows), for NAWS.
    pub resized: Option<TermSize>,
    /// The divider was dragged to a new share (to save).
    pub share: Option<f32>,
    /// The person confirmed opening this address.
    pub open_link: Option<String>,
    /// A message for the status line (a refused link).
    pub notice: Option<S>,
    /// Where the transcript and the live view were drawn (tests).
    pub transcript_rect: Option<Rect>,
    pub tail_rect: Option<Rect>,
    /// Mark as channel... was chosen: the example line and a second, narrowing one.
    pub mark_channel: Option<(String, Option<String>)>,
}

#[derive(Clone, Copy, Debug)]
struct RunSpec {
    col: usize,
    cols: usize,
    text_start: usize,
    text_end: usize,
    look: Look,
    /// One character (with any combining marks) placed on its own.
    single: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Look {
    fg: Color32,
    bg: Color32,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    hidden: bool,
    blink: bool,
}

fn look_of(cell: &Cell, theme: &Theme) -> Look {
    let flags = cell.flags;
    let bold = flags.contains(Flags::BOLD);
    let mut fg = theme.term_fg(cell.fg, bold);
    let mut bg = theme.term_bg(cell.bg);
    if flags.contains(Flags::INVERSE) {
        let solid_bg = if bg == Color32::TRANSPARENT { theme.terminal } else { bg };
        bg = fg;
        fg = solid_bg;
    }
    if flags.contains(Flags::DIM) {
        fg = fg.gamma_multiply(0.6);
    }
    Look {
        fg,
        bg,
        bold,
        italic: flags.contains(Flags::ITALIC),
        underline: flags.intersects(Flags::ALL_UNDERLINES),
        strike: flags.contains(Flags::STRIKEOUT),
        hidden: flags.contains(Flags::HIDDEN),
        blink: flags.contains(BLINK),
    }
}

/// Block elements (U+2580 to U+259F) are painted as rectangles filling the cell, so stacked
/// blocks join without gaps whatever the font's ascent and line gap. Returns up to three
/// rectangles in cell units (x0, y0, x1, y1) and the opacity (shades are partial).
fn block_rects(c: char) -> Option<([[f32; 4]; 3], usize, f32)> {
    const UL: [f32; 4] = [0.0, 0.0, 0.5, 0.5];
    const UR: [f32; 4] = [0.5, 0.0, 1.0, 0.5];
    const LL: [f32; 4] = [0.0, 0.5, 0.5, 1.0];
    const LR: [f32; 4] = [0.5, 0.5, 1.0, 1.0];
    const NONE: [f32; 4] = [0.0; 4];
    let one = |r: [f32; 4]| Some(([r, NONE, NONE], 1, 1.0));
    let n = c as u32;
    match n {
        0x2580 => one([0.0, 0.0, 1.0, 0.5]),
        0x2581..=0x2587 => one([0.0, 1.0 - (n - 0x2580) as f32 / 8.0, 1.0, 1.0]),
        0x2588 => one([0.0, 0.0, 1.0, 1.0]),
        0x2589..=0x258F => one([0.0, 0.0, (0x2590 - n) as f32 / 8.0, 1.0]),
        0x2590 => one([0.5, 0.0, 1.0, 1.0]),
        0x2591..=0x2593 => Some(([[0.0, 0.0, 1.0, 1.0], NONE, NONE], 1, (n - 0x2590) as f32 * 0.25)),
        0x2594 => one([0.0, 0.0, 1.0, 0.125]),
        0x2595 => one([0.875, 0.0, 1.0, 1.0]),
        0x2596 => one(LL),
        0x2597 => one(LR),
        0x2598 => one(UL),
        0x2599 => Some(([UL, LL, LR], 3, 1.0)),
        0x259A => Some(([UL, LR, NONE], 2, 1.0)),
        0x259B => Some(([UL, UR, LL], 3, 1.0)),
        0x259C => Some(([UL, UR, LR], 3, 1.0)),
        0x259D => one(UR),
        0x259E => Some(([UR, LL, NONE], 2, 1.0)),
        0x259F => Some(([UR, LL, LR], 3, 1.0)),
        _ => None,
    }
}

/// Characters the bundled font draws at exactly one cell: they can share a galley.
#[inline]
fn simple(c: char) -> bool {
    matches!(c as u32, 0x20..=0x7E | 0xA0..=0x24F | 0x2500..=0x257F)
}

/// Turn one grid row into runs: `(column, width in columns, text, look)`. Shared by the painter
/// and the tests.
#[cfg(test)]
fn build_runs(cells: &[Cell], theme: &Theme, text: &mut String, runs: &mut Vec<RunSpec>, fallback: &mut FallbackFonts) {
    build_row_runs(&[(cells, 0)], theme, text, runs, fallback);
}

/// Turn one display row (grid row slices, each with the display column it starts at) into runs.
fn build_row_runs(
    segments: &[(&[Cell], usize)],
    theme: &Theme,
    text: &mut String,
    runs: &mut Vec<RunSpec>,
    fallback: &mut FallbackFonts,
) {
    text.clear();
    runs.clear();
    for &(cells, base) in segments {
        push_runs(cells, base, theme, text, runs, fallback);
    }
}

/// Append the runs of `cells`, the first of them at display column `base`.
fn push_runs(
    cells: &[Cell],
    base: usize,
    theme: &Theme,
    text: &mut String,
    runs: &mut Vec<RunSpec>,
    fallback: &mut FallbackFonts,
) {
    // Trailing blank cells with no background need no run.
    let end = cells
        .iter()
        .rposition(|c| {
            (c.c != ' ' && c.c != '\t')
                || c.extra.is_some()
                || c.flags.intersects(Flags::INVERSE)
                || !theme.is_default_bg(c.bg)
        })
        .map_or(0, |i| i + 1);
    let mut open = false;
    for (i, cell) in cells[..end].iter().enumerate() {
        let col = base + i;
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            continue;
        }
        let look = look_of(cell, theme);
        let c = if cell.c == '\t' { ' ' } else { cell.c };
        let zerowidth = cell.zerowidth();
        let plain = simple(c) && zerowidth.is_none() && !cell.flags.contains(Flags::WIDE_CHAR);
        if !c.is_ascii() && block_rects(c).is_none() {
            fallback.note(c);
        }
        if plain
            && open
            && let Some(last) = runs.last_mut()
            && last.look == look
            && last.col + last.cols == col
        {
            text.push(c);
            last.cols += 1;
            last.text_end = text.len();
            continue;
        }
        let start = text.len();
        text.push(c);
        if let Some(zw) = zerowidth {
            for &z in zw {
                if !z.is_ascii() {
                    fallback.note(z);
                }
                text.push(z);
            }
        }
        let cols = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
        runs.push(RunSpec {
            col,
            cols,
            text_start: start,
            text_end: text.len(),
            look,
            single: !plain,
        });
        open = plain;
    }
}

/// The part of the frame one block of rows is painted with.
struct RowPainter<'a> {
    painter: &'a egui::Painter,
    theme: &'a Theme,
    fonts: &'a TermFonts,
    cell: Vec2,
    /// Blinking text is in its hidden half.
    blink_hidden: bool,
}

/// Paint `rows` rows from `origin`, row `r` being the grid slices `row(r, ..)` puts in its
/// vector (each with its display column; false skips the row); `selection` gives the selected
/// columns of a row. Returns the runs painted and whether any blinking text was on them.
#[allow(clippy::too_many_arguments)]
fn paint_rows<'t>(
    ui: &Ui,
    p: &RowPainter<'_>,
    origin: Pos2,
    rows: usize,
    row: impl Fn(usize, &mut Vec<(&'t [Cell], usize)>) -> bool,
    selection: impl Fn(usize) -> Option<(usize, usize)>,
    grid: &mut GridText,
    text: &mut String,
    runs: &mut Vec<RunSpec>,
    fallback: &mut FallbackFonts,
) -> (usize, bool) {
    let (cell_w, row_h) = (p.cell.x, p.cell.y);
    let area = Rect::from_min_size(origin, egui::vec2(0.0, 0.0));
    let mut drawn = 0;
    let mut blink_seen = false;
    grid.begin(ui, &p.fonts.regular);
    let mut segments = Vec::new();
    for r in 0..rows {
        if !row(r, &mut segments) {
            continue;
        }
        let y = origin.y + r as f32 * row_h;
        build_row_runs(&segments, p.theme, text, runs, fallback);
        for run in runs.iter() {
            if run.look.bg != Color32::TRANSPARENT {
                p.painter
                    .rect_filled(cell_rect(area, run.col, run.cols, y, cell_w, row_h), 0.0, run.look.bg);
            }
        }
        if let Some((from, to)) = selection(r)
            && to > from
        {
            p.painter.rect_filled(
                cell_rect(area, from, to - from, y, cell_w, row_h),
                0.0,
                p.theme.selection,
            );
        }
        for run in runs.iter() {
            blink_seen |= run.look.blink;
            if run.look.hidden || (run.look.blink && p.blink_hidden) {
                continue;
            }
            let s = &text[run.text_start..run.text_end];
            let font = if run.look.bold { &p.fonts.bold } else { &p.fonts.regular };
            if !run.single {
                grid.push_run(
                    ui,
                    font,
                    s,
                    run.col,
                    r,
                    p.cell,
                    grid_text::style(run.look.bold, run.look.italic),
                    run.look.fg,
                );
                let rect = cell_rect(area, run.col, run.cols, y, cell_w, row_h);
                let stroke = Stroke::new(1.0, run.look.fg);
                if run.look.underline {
                    p.painter.hline(rect.x_range(), rect.bottom() - 1.5, stroke);
                }
                if run.look.strike {
                    p.painter.hline(rect.x_range(), rect.center().y, stroke);
                }
                drawn += 1;
                continue;
            }
            if let Some((rects, count, alpha)) = s.chars().next().and_then(block_rects) {
                let cell = cell_rect(area, run.col, 1, y, cell_w, row_h);
                let color = run.look.fg.gamma_multiply(alpha);
                for b in &rects[..count] {
                    let min = cell.min + egui::vec2(b[0] * cell_w, b[1] * row_h);
                    let max = cell.min + egui::vec2(b[2] * cell_w, b[3] * row_h);
                    p.painter.rect_filled(Rect::from_min_max(min, max), 0.0, color);
                }
                drawn += 1;
                continue;
            }
            let stroke = Stroke::new(1.0, run.look.fg);
            let format = TextFormat {
                font_id: font.clone(),
                color: run.look.fg,
                italics: run.look.italic,
                underline: if run.look.underline { stroke } else { Stroke::NONE },
                strikethrough: if run.look.strike { stroke } else { Stroke::NONE },
                ..Default::default()
            };
            let mut job = LayoutJob::single_section(s.to_owned(), format);
            job.wrap.max_width = f32::INFINITY;
            let galley = p.painter.layout_job(job);
            // A character placed on its own: centre it in its cells.
            let slack = (run.cols as f32 * cell_w - galley.size().x).max(0.0);
            let x = origin.x + run.col as f32 * cell_w + slack / 2.0;
            p.painter.galley(egui::pos2(x, y), galley, run.look.fg);
            drawn += 1;
        }
    }
    grid.finish(
        p.painter,
        origin,
        egui::vec2(p.painter.clip_rect().width(), rows as f32 * row_h),
    );
    (drawn, blink_seen)
}

/// Paint the grid and handle mouse input over it. While the view is scrolled back, the bottom
/// share of the area becomes the live view: the newest rows keep running there below a
/// draggable divider, and the transcript above keeps the reader's place.
#[allow(clippy::too_many_arguments)]
pub fn show_terminal(
    ui: &mut Ui,
    term: &mut Terminal,
    id: u64,
    view: &mut TerminalViewState,
    theme: &Theme,
    fonts: &TermFonts,
    fallback: &mut FallbackFonts,
    options: &PaintOptions,
) -> TerminalOutput {
    let mut out = TerminalOutput::default();
    let (cell_w, row_h) = ui.fonts_mut(|f| (f.glyph_width(&fonts.regular, 'M'), f.row_height(&fonts.regular)));
    view.cell = egui::vec2(cell_w, row_h);
    let rect = ui.available_rect_before_wrap();
    ui.allocate_rect(rect, Sense::hover());
    let full = Rect::from_min_max(rect.min, egui::pos2(rect.max.x - SCROLLBAR - 2.0, rect.max.y));
    let full = limit_width(full, cell_w, options.max_columns);
    // The grid's size comes from the whole area: the live view covers its lower rows rather
    // than shrinking the grid, so the server's idea of the window and the reader's place stay.
    let columns = ((full.width() / cell_w).floor() as usize).max(2);
    let rows = ((full.height() / row_h).floor() as usize).max(1);
    let size = TermSize::new(columns, rows);
    if term.resize(size) {
        out.resized = Some(size);
    }

    let share = view.drag_share.unwrap_or(options.tail_share);
    let split = term.display_offset() > 0 && TAIL_SHARES.contains(&share);
    let (text_rect, tail) = if split {
        let room = (full.height() - DIVIDER).max(0.0);
        let live = (room * share).round();
        let upper = Rect::from_min_size(full.min, egui::vec2(full.width(), room - live));
        let divider = Rect::from_min_size(egui::pos2(rect.min.x, upper.max.y), egui::vec2(rect.width(), DIVIDER));
        let tail = Rect::from_min_max(egui::pos2(rect.min.x, divider.max.y), rect.max);
        (upper, Some((divider, tail)))
    } else {
        (full, None)
    };
    let visible_rows = ((text_rect.height() / row_h).floor() as usize).clamp(1, rows);
    out.transcript_rect = Some(text_rect);
    view.transcript_rect = Some(text_rect);
    view.divider_rect = tail.map(|(d, _)| d);
    view.tail_rect = tail.map(|(_, t)| t);
    let bar_rect = Rect::from_min_max(
        egui::pos2(rect.max.x - SCROLLBAR, rect.min.y),
        egui::pos2(rect.max.x, text_rect.max.y),
    );

    let response = ui.interact(text_rect, ui.id().with(("terminal", id)), Sense::click_and_drag());
    crate::a11y::expose_terminal(ui.ctx(), response.id, term);
    layout_display(term, view, visible_rows, options.wrap);
    mouse(
        ui,
        term,
        view,
        &response,
        text_rect,
        visible_rows,
        options.wrap,
        &mut out,
    );
    if response.secondary_clicked()
        && let Some(pos) = response.interact_pointer_pos()
    {
        let (row, col, side) = cell_at(term, text_rect, view.cell, pos);
        let (point, _) = display_point(term, &view.display, row, col, side);
        // A right click keeps the selection (it is what Copy copies).
        view.menu = Some(MenuTarget {
            line: term.line_at(point).0,
            selection: term.selection_text().filter(|s| !s.trim().is_empty()),
        });
    }
    transcript_menu(&response, view, &mut out);
    hover_link(ui, term, view, &response, text_rect);
    wheel(ui, term, view, &response);
    scrollbar(ui, term, view, bar_rect, id);
    // Scrolling or a drag past an edge may have moved the view.
    layout_display(term, view, visible_rows, options.wrap);

    // Paint: backgrounds and the selection as rectangles, plain text into one mesh, characters
    // placed on their own (fallback glyphs, wide and combined characters) as galleys.
    let painter = ui.painter_at(rect);
    let blink_hidden = options.blink && (ui.input(|i| i.time) / BLINK_HALF) as u64 % 2 == 1;
    let rp = RowPainter {
        painter: &painter,
        theme,
        fonts,
        cell: view.cell,
        blink_hidden,
    };
    let selection = term.selection_range();
    let mut runs = std::mem::take(&mut view.runs);
    let mut text = std::mem::take(&mut view.text);
    let display = std::mem::take(&mut view.display);
    let mut tail_display = std::mem::take(&mut view.tail_display);
    let term_ref: &Terminal = term;
    let (drawn, mut blink_seen) = paint_rows(
        ui,
        &rp,
        text_rect.min,
        visible_rows,
        |r, segments| match display.get(r) {
            Some(row) => {
                term_ref.row_segments(row, segments);
                true
            }
            None => false,
        },
        |r| term_ref.row_selection(display.get(r)?, selection.as_ref()?),
        &mut view.grid,
        &mut text,
        &mut runs,
        fallback,
    );
    view.display = display;
    view.runs_drawn = drawn;
    view.rows_drawn = visible_rows;
    view.tail_rows_drawn = 0;

    if let Some((divider, tail_rect)) = tail {
        out.tail_rect = Some(tail_rect);
        // The live view: the newest rows, newest at the bottom.
        painter.rect_filled(tail_rect, 0.0, theme.terminal);
        let inner = tail_rect.shrink2(egui::vec2(0.0, 2.0));
        let count = ((inner.height() / row_h + 0.001).floor() as usize).min(MAX_TAIL_ROWS);
        term_ref.tail_display_rows(count, options.wrap, &mut tail_display);
        let count = tail_display.len();
        let origin = egui::pos2(text_rect.min.x, inner.max.y - count as f32 * row_h);
        let (tail_drawn, tail_blink) = paint_rows(
            ui,
            &rp,
            origin,
            count,
            |r, segments| {
                term_ref.row_segments(&tail_display[r], segments);
                true
            },
            |_| None,
            &mut view.tail_grid,
            &mut text,
            &mut runs,
            fallback,
        );
        view.runs_drawn += tail_drawn;
        view.tail_rows_drawn = count;
        blink_seen |= tail_blink;
        let tail_response = ui
            .interact(tail_rect, ui.id().with(("tail", id)), Sense::click())
            .on_hover_text(t(S::LiveViewWhileScrolledUp));
        crate::a11y::control(
            &tail_response,
            egui::accesskit::Role::Button,
            t(S::LiveViewWhileScrolledUp),
        );
        if tail_response.secondary_clicked()
            && let Some(pos) = tail_response.interact_pointer_pos()
        {
            // The live view offers the same menu for the row under the pointer.
            let r = (((pos.y - origin.y) / row_h).floor().max(0.0) as usize).min(count.saturating_sub(1));
            let line = tail_display
                .get(r)
                .map(|row| term.line_at(term.row_point(row, row.start)).0)
                .unwrap_or_default();
            view.menu = Some(MenuTarget {
                line,
                selection: term.selection_text().filter(|s| !s.trim().is_empty()),
            });
        }
        transcript_menu(&tail_response, view, &mut out);
        if tail_response.clicked() {
            term.scroll_to_bottom();
            out.latest_clicked = true;
            out.refocus_input = true;
        }
        // The divider: drag to choose the share; the share it settles on is saved.
        painter.rect_filled(divider, 0.0, theme.border);
        let grab = divider.expand2(egui::vec2(0.0, 3.0));
        let drag = ui
            .interact(grab, ui.id().with(("divider", id)), Sense::drag())
            .on_hover_cursor(egui::CursorIcon::ResizeVertical);
        crate::a11y::control(&drag, egui::accesskit::Role::Splitter, t(S::LiveViewWhileScrolledUp));
        if drag.dragged()
            && let Some(pos) = drag.interact_pointer_pos()
        {
            let room = (full.height() - DIVIDER).max(1.0);
            let share = ((rect.max.y - pos.y - DIVIDER / 2.0) / room).clamp(*TAIL_SHARES.start(), *TAIL_SHARES.end());
            view.drag_share = Some(share);
        }
        if drag.drag_stopped()
            && let Some(share) = view.drag_share.take()
        {
            out.share = Some((share * 100.0).round() / 100.0);
        }
    } else {
        view.drag_share = None;
    }
    view.runs = runs;
    view.text = text;
    view.tail_display = tail_display;
    view.blink_seen = blink_seen;
    if blink_seen && options.blink {
        let now = ui.input(|i| i.time);
        let next = (now / BLINK_HALF).floor() * BLINK_HALF + BLINK_HALF - now;
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64(next.max(0.01)));
    }

    if term.display_offset() > 0 {
        let pos = text_rect.right_bottom() - egui::vec2(16.0, 12.0);
        let button = egui::Area::new(ui.id().with(("latest", id)))
            .fixed_pos(pos)
            .pivot(Align2::RIGHT_BOTTOM)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                ui.add(egui::Button::new(RichText::new(t(S::LatestOutput)).size(14.0)).min_size(egui::vec2(0.0, 30.0)))
            });
        if button.inner.clicked() {
            term.scroll_to_bottom();
            out.latest_clicked = true;
            out.refocus_input = true;
        }
    }
    link_bar(ui, id, view, rect, theme, &mut out);
    out
}

/// The transcript's right-click menu (C# `TranscriptMenu`): Copy while there is a selection,
/// and Mark as channel... for the line.
fn transcript_menu(response: &egui::Response, view: &mut TerminalViewState, out: &mut TerminalOutput) {
    response.context_menu(|ui| {
        let Some(target) = view.menu.clone() else {
            ui.close();
            return;
        };
        if let Some(selection) = &target.selection
            && ui.button(t(S::TranscriptCopy)).clicked()
        {
            ui.ctx().copy_text(selection.clone());
            ui.close();
        }
        let (example, _) = target.examples();
        if ui
            .add_enabled(!example.trim().is_empty(), egui::Button::new(t(S::MarkAsChannel)))
            .clicked()
        {
            out.mark_channel = Some(target.examples());
            ui.close();
        }
    });
}

/// The "Open this link?" bar over the top of the transcript: the whole address, Open and Cancel.
fn link_bar(ui: &Ui, id: u64, view: &mut TerminalViewState, rect: Rect, theme: &Theme, out: &mut TerminalOutput) {
    view.link_buttons = None;
    let Some(url) = view.pending_link.clone() else { return };
    let width = (rect.width() - 24.0).clamp(120.0, 720.0);
    let mut buttons = None;
    let area = egui::Area::new(ui.id().with(("link-confirmation", id)))
        .fixed_pos(rect.min + egui::vec2(12.0, 12.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            egui::Frame::new()
                .fill(theme.panel)
                .stroke(Stroke::new(1.0, theme.border))
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(width - 24.0);
                    ui.horizontal(|ui| {
                        let mut action = None;
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let cancel = crate::dialogs::secondary_button(ui, t(S::Cancel));
                            if cancel.clicked() {
                                action = Some(false);
                            }
                            let open = crate::dialogs::primary_button(ui, t(S::LinkOpen), theme);
                            if open.clicked() {
                                action = Some(true);
                            }
                            buttons = Some((open.rect, cancel.rect));
                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                ui.label(RichText::new(t(S::LinkOpenQuestion)).size(13.0).color(theme.text));
                                ui.add(egui::Label::new(
                                    RichText::new(&url).font(FontId::monospace(12.0)).color(theme.text),
                                ))
                                .on_hover_text(&url);
                            });
                        });
                        action
                    })
                    .inner
                })
                .inner
        });
    view.link_buttons = buttons;
    match area.inner {
        Some(true) => {
            view.pending_link = None;
            out.open_link = Some(url);
        }
        Some(false) => view.pending_link = None,
        None => {}
    }
}

/// Whether a click with these modifiers opens a link: Ctrl, and Cmd on macOS (the C# rule).
fn link_modifiers(modifiers: egui::Modifiers) -> bool {
    modifiers.ctrl || (cfg!(target_os = "macos") && modifiers.mac_cmd)
}

/// The hover hint for links on this platform.
fn link_hint() -> &'static str {
    t(if cfg!(target_os = "macos") {
        S::LinkClickHintMac
    } else {
        S::LinkClickHint
    })
}

/// The web address under a point of the transcript, if any.
fn link_under(term: &Terminal, display: &[DisplayRow], text_rect: Rect, cell: Vec2, pos: Pos2) -> Option<String> {
    if !text_rect.contains(pos) {
        return None;
    }
    let (row, col, side) = cell_at(term, text_rect, cell, pos);
    let (point, _) = display_point(term, display, row, col, side);
    let (line, index) = term.line_at(point);
    wandur_core::weblinks::link_at(&line, index).map(str::to_string)
}

/// A link was clicked: offer it with the confirmation bar, or refuse it with a notice (C#
/// `RequestOpenLink`). Returns whether it was offered.
pub fn request_link(view: &mut TerminalViewState, url: &str) -> Result<(), S> {
    match wandur_core::weblinks::check(url) {
        Ok(uri) => {
            view.pending_link = Some(uri.to_string());
            Ok(())
        }
        Err(refusal) => {
            view.pending_link = None;
            Err(match refusal {
                wandur_core::weblinks::Refusal::LocalNetwork => S::LinkRefusedLocal,
                wandur_core::weblinks::Refusal::NotAWebLink => S::LinkRefused,
            })
        }
    }
}

/// While the pointer is over a link: the hand cursor and the hint that names the shortcut.
fn hover_link(ui: &Ui, term: &Terminal, view: &mut TerminalViewState, response: &egui::Response, text_rect: Rect) {
    let Some(pos) = response.hover_pos() else {
        view.hover = None;
        return;
    };
    let (row, col, _) = cell_at(term, text_rect, view.cell, pos);
    let key = (row, col, term.revision(), term.display_offset());
    let link = match &view.hover {
        Some((k, link)) if *k == key => link.clone(),
        _ => {
            let link = link_under(term, &view.display, text_rect, view.cell, pos);
            view.hover = Some((key, link.clone()));
            link
        }
    };
    if link.is_some() {
        if ui.input(|i| link_modifiers(i.modifiers)) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        response.clone().on_hover_text(link_hint());
    }
}

fn cell_rect(text_rect: Rect, col: usize, cols: usize, y: f32, cell_w: f32, row_h: f32) -> Rect {
    let x = text_rect.min.x + col as f32 * cell_w;
    Rect::from_min_size(egui::pos2(x, y), egui::vec2(cols as f32 * cell_w, row_h))
}

/// The transcript's area with Limit text width: at most `max_columns` cells wide, centred in a
/// wider pane (on whole points, so the cells stay on the pixel grid).
fn limit_width(full: Rect, cell_w: f32, max_columns: Option<usize>) -> Rect {
    let Some(columns) = max_columns else { return full };
    let width = columns as f32 * cell_w;
    if full.width() <= width {
        return full;
    }
    let left = (full.center().x - width / 2.0).round();
    Rect::from_min_max(egui::pos2(left, full.min.y), egui::pos2(left + width, full.max.y))
}

/// Lay out the transcript's display rows for this frame, unless nothing they depend on changed.
fn layout_display(term: &Terminal, view: &mut TerminalViewState, visible: usize, wrap: WrapOptions) {
    let key = (
        term.revision(),
        term.display_offset(),
        term.size().columns,
        visible,
        wrap,
    );
    if view.display_key != Some(key) {
        term.view_display_rows(visible, wrap, &mut view.display);
        view.display_key = Some(key);
    }
}

/// The grid point under column `col` of display row `row`. Below the last display row (a short
/// transcript) is the end of the last one.
fn display_point(term: &Terminal, display: &[DisplayRow], row: usize, col: usize, side: Side) -> (Point, Side) {
    match display.get(row).or(display.last()) {
        Some(d) if row < display.len() => term.display_point(d, col, side),
        Some(d) => term.display_point(d, term.size().columns - 1, Side::Right),
        None => (term.view_point(row, col), side),
    }
}

/// The cell under `pos`, clamped to the grid, and which half of it.
fn cell_at(term: &Terminal, text_rect: Rect, cell: Vec2, pos: Pos2) -> (usize, usize, Side) {
    let size = term.size();
    let x = ((pos.x - text_rect.min.x) / cell.x).max(0.0);
    let y = ((pos.y - text_rect.min.y) / cell.y).max(0.0);
    let col = (x.floor() as usize).min(size.columns - 1);
    let row = (y.floor() as usize).min(size.rows - 1);
    let side = if x.fract() < 0.5 { Side::Left } else { Side::Right };
    (row, col, side)
}

#[allow(clippy::too_many_arguments)]
fn mouse(
    ui: &Ui,
    term: &mut Terminal,
    view: &mut TerminalViewState,
    response: &egui::Response,
    text_rect: Rect,
    visible_rows: usize,
    wrap: WrapOptions,
    out: &mut TerminalOutput,
) {
    let cell = view.cell;
    let modifiers = ui.input(|i| i.modifiers);
    let shift = modifiers.shift;
    // Ctrl+click (and Cmd+click on macOS) on a link asks before opening it; nothing else.
    if response.clicked()
        && link_modifiers(modifiers)
        && let Some(pos) = response.interact_pointer_pos()
        && let Some(url) = link_under(term, &view.display, text_rect, cell, pos)
    {
        if let Err(notice) = request_link(view, &url) {
            out.notice = Some(notice);
        }
        return;
    }
    if response.drag_started_by(egui::PointerButton::Primary)
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        let (row, col, side) = cell_at(term, text_rect, cell, origin);
        let (point, side) = display_point(term, &view.display, row, col, side);
        if shift && term.selection_range().is_some() {
            term.select_update(point, side);
        } else {
            term.select_start(SelectionKind::Simple, point, side);
        }
        view.selecting = true;
    }
    if view.selecting
        && response.dragged_by(egui::PointerButton::Primary)
        && let Some(pos) = response.interact_pointer_pos()
    {
        // Past the top or bottom edge: scroll, faster the further out the pointer is.
        let mut target = pos;
        if pos.y < text_rect.min.y {
            let n = (((text_rect.min.y - pos.y) / cell.y).ceil() as i32).clamp(1, 10);
            term.scroll(n);
            layout_display(term, view, visible_rows, wrap);
            target.y = text_rect.min.y;
            ui.ctx().request_repaint();
        } else if pos.y > text_rect.max.y {
            let n = (((pos.y - text_rect.max.y) / cell.y).ceil() as i32).clamp(1, 10);
            term.scroll(-n);
            layout_display(term, view, visible_rows, wrap);
            target.y = text_rect.min.y + (visible_rows as f32 - 0.5) * cell.y;
            ui.ctx().request_repaint();
        }
        let (row, col, side) = cell_at(term, text_rect, cell, target);
        let (point, side) = display_point(term, &view.display, row.min(visible_rows - 1), col, side);
        term.select_update(point, side);
    }
    if response.drag_stopped() && view.selecting {
        view.selecting = false;
        out.refocus_input = true;
    }
    if let Some(pos) = response
        .interact_pointer_pos()
        .filter(|_| response.clicked() || response.double_clicked() || response.triple_clicked())
    {
        let (row, col, side) = cell_at(term, text_rect, cell, pos);
        let (point, side) = display_point(term, &view.display, row, col, side);
        if response.triple_clicked() {
            term.select_start(SelectionKind::Line, point, side);
        } else if response.double_clicked() {
            term.select_start(SelectionKind::Word, point, side);
        } else if shift && term.selection_range().is_some() {
            term.select_update(point, side);
        } else {
            term.select_clear();
        }
        out.refocus_input = true;
    }
}

fn wheel(ui: &Ui, term: &mut Terminal, view: &mut TerminalViewState, response: &egui::Response) {
    if !response.hovered() {
        view.scroll_rest = 0.0;
        return;
    }
    let dy = ui.input(|i| i.smooth_scroll_delta.y);
    if dy == 0.0 {
        return;
    }
    view.scroll_rest += dy / view.cell.y;
    let rows = view.scroll_rest.trunc();
    if rows != 0.0 {
        term.scroll(rows as i32);
        view.scroll_rest -= rows;
    }
}

fn scrollbar(ui: &Ui, term: &mut Terminal, view: &mut TerminalViewState, bar: Rect, id: u64) {
    let history = term.history_len();
    if history == 0 {
        return;
    }
    let rows = term.size().rows;
    let total = (history + rows) as f32;
    let top = (history - term.display_offset()) as f32;
    let thumb_h = (bar.height() * rows as f32 / total).max(16.0);
    let travel = bar.height() - thumb_h;
    let thumb_y = bar.min.y + travel * (top / history as f32);
    let thumb = Rect::from_min_size(
        egui::pos2(bar.min.x + 2.0, thumb_y),
        egui::vec2(bar.width() - 4.0, thumb_h),
    );
    let response = ui.interact(bar, ui.id().with(("scrollbar", id)), Sense::click_and_drag());
    crate::a11y::control(&response, egui::accesskit::Role::ScrollBar, t(S::A11yTranscript));
    if response.drag_started() || response.clicked() {
        view.scrollbar_drag = true;
    }
    if view.scrollbar_drag
        && let Some(pos) = response.interact_pointer_pos()
    {
        let fraction = ((pos.y - bar.min.y - thumb_h / 2.0) / travel.max(1.0)).clamp(0.0, 1.0);
        let top = (fraction * history as f32).round() as usize;
        term.set_display_offset(history - top.min(history));
    }
    if response.drag_stopped() || (!response.dragged() && !response.is_pointer_button_down_on()) {
        view.scrollbar_drag = false;
    }
    let painter = ui.painter_at(bar);
    let color = if response.hovered() || view.scrollbar_drag {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        ui.visuals().widgets.inactive.bg_fill
    };
    painter.rect_filled(thumb, 3.0, color);
}

/// What the person asked for in this frame.
#[derive(Debug, Default, PartialEq)]
pub struct ViewActions {
    pub disconnect: bool,
    pub reconnect: bool,
    pub save_world: bool,
    pub resized: Option<TermSize>,
    /// The live view's divider settled on this share (save it).
    pub share: Option<f32>,
    /// Open this address (confirmed in the link bar).
    pub open_link: Option<String>,
    /// Show this message (a refused link).
    pub notice: Option<S>,
    /// Open the Mark as channel dialog for this line (and a second example).
    pub mark_channel: Option<(String, Option<String>)>,
    /// Scripts menu: Edit configuration... (the world editor's Scripts section).
    pub edit_scripts: bool,
    /// Scripts menu: Reload saved rules.
    pub reload_scripts: bool,
    /// Agent menu: Configure agent... (the world editor's Agent settings section).
    pub edit_agent: bool,
    /// Where the full map goes this frame (the Map page or side by side); the shell draws it.
    pub map_rect: Option<Rect>,
    /// The side-by-side divider settled on this map share (to remember for new sessions).
    pub split_share: Option<f32>,
}

/// Draw a whole session tab.
pub fn show(
    ui: &mut Ui,
    tab: &mut SessionTab,
    view: &mut TerminalViewState,
    theme: &Theme,
    fonts: &TermFonts,
    fallback: &mut FallbackFonts,
    options: &PaintOptions,
) -> ViewActions {
    let mut actions = ViewActions::default();
    view.map_rect = None;
    view.strip_rect = None;
    view.strip_rows_drawn = 0;
    view.rows_drawn = 0;
    view.tail_rows_drawn = 0;
    // Escape on the map (with nothing else holding the keyboard) goes back to Play's composer.
    // The map editor keeps Escape while it has a tool, a gesture, a menu or a selection to step
    // back from (it says so each frame it is drawn).
    let editor_keeps = ui
        .ctx()
        .data(|d| d.get_temp::<bool>(crate::map_view::map_keeps_escape_id()))
        .unwrap_or(false);
    if view.map_shown()
        && !editor_keeps
        && ui.input(|i| i.key_pressed(egui::Key::Escape))
        && ui.memory(|m| m.focused().is_none())
    {
        if !view.split {
            view.page = Page::Play;
        }
        tab.focus_input = true;
    }
    status_line(ui, tab, theme, &mut actions);
    ui.add_space(2.0);
    let width = ui.available_width();
    if view.page == Page::Diagnostics {
        // Diagnostics takes the whole content area; the command input belongs to Play.
        let height = (ui.available_height() - FOOTER_HEIGHT).max(40.0);
        ui.allocate_ui(egui::vec2(width, height), |ui| {
            ui.set_min_size(ui.available_size());
            crate::diagnostics_view::show(ui, &mut tab.protocol, &mut view.diagnostics, theme);
        });
        footer(ui, tab, view, theme, false, &mut actions);
        return actions;
    }
    if view.map_shown() {
        // The map's area: the Map page with the output strip under it, or Play and Map side by
        // side. The shell draws the map in `map_rect`.
        let height = (ui.available_height() - FOOTER_HEIGHT - ui.spacing().item_spacing.y).max(60.0);
        let (body, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
        let mut suggestion = false;
        if view.split {
            let room = (body.width() - SPLIT_DIVIDER).max(0.0);
            let map_width = (room * view.split_share()).round();
            let play = Rect::from_min_max(
                body.min,
                egui::pos2(body.right() - map_width - SPLIT_DIVIDER, body.bottom()),
            );
            let divider = Rect::from_min_max(
                egui::pos2(play.right(), body.top()),
                egui::pos2(play.right() + SPLIT_DIVIDER, body.bottom()),
            );
            let mut column = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt(("play-column", tab.id))
                    .max_rect(play)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            suggestion = play_body(
                &mut column,
                tab,
                view,
                theme,
                fonts,
                fallback,
                options,
                0.0,
                &mut actions,
            );
            ui.painter().rect_filled(divider, 0.0, theme.shell);
            ui.painter().vline(
                divider.center().x,
                divider.y_range(),
                egui::Stroke::new(1.0, theme.border),
            );
            let drag = ui
                .interact(
                    divider.expand2(egui::vec2(2.0, 0.0)),
                    ui.id().with(("split", tab.id)),
                    egui::Sense::drag(),
                )
                .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
            crate::a11y::control(&drag, egui::accesskit::Role::Splitter, t(S::SplitResize));
            if drag.dragged()
                && let Some(pos) = drag.interact_pointer_pos()
            {
                let share = ((body.right() - pos.x - SPLIT_DIVIDER / 2.0) / room.max(1.0))
                    .clamp(*SPLIT_SHARES.start(), *SPLIT_SHARES.end());
                view.split_share = share;
            }
            if drag.drag_stopped() {
                actions.split_share = Some((view.split_share() * 100.0).round() / 100.0);
            }
            view.map_rect = Some(Rect::from_min_max(egui::pos2(divider.right(), body.top()), body.max));
        } else {
            let (cell_w, row_h) = ui.fonts_mut(|f| (f.glyph_width(&fonts.regular, 'M'), f.row_height(&fonts.regular)));
            if view.cell == Vec2::ZERO {
                view.cell = egui::vec2(cell_w, row_h);
            }
            let strip_height = (view.strip_rows() as f32 * row_h + 8.0).min(body.height() * 0.6);
            let strip = Rect::from_min_max(egui::pos2(body.left(), body.bottom() - strip_height), body.max);
            let divider = Rect::from_min_max(
                egui::pos2(body.left(), strip.top() - DIVIDER),
                egui::pos2(body.right(), strip.top()),
            );
            view.map_rect = Some(Rect::from_min_max(body.min, egui::pos2(body.right(), divider.top())));
            if output_strip(ui, strip, &tab.terminal, tab.id, view, theme, fonts, fallback, options).clicked() {
                view.show_map(false);
                tab.focus_input = true;
            }
            ui.painter().rect_filled(divider, 0.0, theme.border);
            let drag = ui
                .interact(
                    divider.expand2(egui::vec2(0.0, 3.0)),
                    ui.id().with(("strip", tab.id)),
                    egui::Sense::drag(),
                )
                .on_hover_cursor(egui::CursorIcon::ResizeVertical);
            crate::a11y::control(&drag, egui::accesskit::Role::Splitter, t(S::OutputStripResize));
            if drag.dragged()
                && let Some(pos) = drag.interact_pointer_pos()
            {
                let rows = ((body.bottom() - pos.y - 8.0) / row_h).round().max(1.0) as usize;
                view.strip_rows = rows.clamp(*STRIP_ROWS.start(), *STRIP_ROWS.end());
            }
        }
        actions.map_rect = view.map_rect;
        footer(ui, tab, view, theme, suggestion, &mut actions);
        return actions;
    }
    let suggestion = play_body(
        ui,
        tab,
        view,
        theme,
        fonts,
        fallback,
        options,
        FOOTER_HEIGHT,
        &mut actions,
    );
    footer(ui, tab, view, theme, suggestion, &mut actions);
    actions
}

/// The newest rows of the session's grid, newest at the bottom, the prompt included: read from
/// the one terminal model (no copy of the transcript). A click returns to Play.
#[allow(clippy::too_many_arguments)]
pub fn output_strip(
    ui: &mut Ui,
    rect: Rect,
    term: &Terminal,
    id: u64,
    view: &mut TerminalViewState,
    theme: &Theme,
    fonts: &TermFonts,
    fallback: &mut FallbackFonts,
    options: &PaintOptions,
) -> egui::Response {
    let row_h = view.cell.y.max(1.0);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme.terminal);
    let inner = Rect::from_min_max(rect.min + egui::vec2(12.0, 4.0), rect.max - egui::vec2(4.0, 4.0));
    let count = ((inner.height() / row_h + 0.001).floor() as usize).min(MAX_TAIL_ROWS);
    let rp = RowPainter {
        painter: &painter,
        theme,
        fonts,
        cell: view.cell,
        blink_hidden: options.blink && (ui.input(|i| i.time) / BLINK_HALF) as u64 % 2 == 1,
    };
    let mut runs = std::mem::take(&mut view.runs);
    let mut text = std::mem::take(&mut view.text);
    let mut rows = std::mem::take(&mut view.tail_display);
    term.tail_display_rows(count, options.wrap, &mut rows);
    let origin = egui::pos2(inner.min.x, inner.max.y - rows.len() as f32 * row_h);
    let (drawn, _) = paint_rows(
        ui,
        &rp,
        origin,
        rows.len(),
        |r, segments| {
            term.row_segments(&rows[r], segments);
            true
        },
        |_| None,
        &mut view.strip_grid,
        &mut text,
        &mut runs,
        fallback,
    );
    let count = rows.len();
    view.runs = runs;
    view.text = text;
    view.tail_display = rows;
    view.runs_drawn = drawn;
    view.strip_rows_drawn = count;
    view.strip_rect = Some(rect);
    let response = ui
        .interact(rect, ui.id().with(("output-strip", id)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(t(S::OutputStripHint));
    crate::a11y::control(&response, egui::accesskit::Role::Button, t(S::OutputStripHint));
    response
}

/// Play: the transcript (and the session rail), the vitals strip and the composer, leaving
/// `reserve` points under them (the footer, when it is drawn after in the same column).
/// Returns whether a completion waits for Tab.
#[allow(clippy::too_many_arguments)]
fn play_body(
    ui: &mut Ui,
    tab: &mut SessionTab,
    view: &mut TerminalViewState,
    theme: &Theme,
    fonts: &TermFonts,
    fallback: &mut FallbackFonts,
    options: &PaintOptions,
    reserve: f32,
    actions: &mut ViewActions,
) -> bool {
    let width = ui.available_width();
    view.diagnostics.console_hidden(&tab.protocol.console);
    view.vitals = crate::vitals_view::cards(tab.protocol.bindings(), tab.is_connected());
    view.vitals.extend(crate::vitals_view::script_cards(&tab.panels));
    let vitals_height = crate::vitals_view::height(&view.vitals, width);
    let composer_height = COMPOSER_HEIGHT + reserve + vitals_height;
    let terminal_height = (ui.available_height() - composer_height).max(40.0);
    let mut output = TerminalOutput::default();
    // The session rail beside the transcript; the vitals strip and the composer span both.
    let rail_shows = crate::panel_view::sync(&mut tab.panels, &mut view.rail);
    let rail_width = crate::panel_view::width(rail_shows, &view.rail).min(width / 2.0);
    let (area, _) = ui.allocate_exact_size(egui::vec2(width, terminal_height), egui::Sense::hover());
    let transcript = Rect::from_min_max(area.min, egui::pos2(area.right() - rail_width, area.bottom()));
    ui.scope_builder(egui::UiBuilder::new().max_rect(transcript), |ui| {
        egui::Frame::new()
            .fill(theme.terminal)
            .inner_margin(egui::Margin {
                left: 12,
                right: 4,
                top: 12,
                bottom: 4,
            })
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                output = show_terminal(ui, &mut tab.terminal, tab.id, view, theme, fonts, fallback, options);
            });
    });
    if rail_width > 0.0 {
        let rail = Rect::from_min_max(egui::pos2(transcript.right(), area.top()), area.max);
        let inputs = crate::panel_view::show(ui, rail, &tab.panels, &mut view.rail, theme);
        for input in inputs {
            tab.panel_event(
                &input.script,
                &input.panel,
                &input.widget,
                input.event,
                input.text.as_deref(),
            );
        }
    }
    if output.refocus_input {
        tab.focus_input = true;
    }
    actions.resized = output.resized;
    actions.share = output.share;
    actions.open_link = output.open_link;
    actions.notice = output.notice;
    actions.mark_channel = output.mark_channel;
    crate::vitals_view::show(ui, &view.vitals, theme);
    composer(
        ui,
        tab,
        view,
        theme,
        &fonts.regular,
        options.suggestions,
        options.quiet_send,
    )
}

/// Height of the composer row (the command box and its buttons).
const COMPOSER_HEIGHT: f32 = 46.0;

/// Height of the session footer.
const FOOTER_HEIGHT: f32 = 28.0;

/// The session footer, as the C# one under the command box: the Play and Diagnostics views,
/// Scripts (t09) and this session's Macros switch on the left; the command hint and the Private
/// input padlock on the right.
fn footer(
    ui: &mut Ui,
    tab: &mut SessionTab,
    view: &mut TerminalViewState,
    theme: &Theme,
    suggesting: bool,
    actions: &mut ViewActions,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), FOOTER_HEIGHT), egui::Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.top(), egui::Stroke::new(1.0, theme.border));
    let inner = rect.shrink2(egui::vec2(12.0, 1.0));
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            let split = view.split && view.page != Page::Diagnostics;
            if footer_tab(ui, t(S::SessionPlay), view.page == Page::Play || split, true, theme).clicked() {
                view.show_map(false);
                tab.focus_input = true;
            }
            if footer_tab(ui, t(S::Map), view.page == Page::Map || split, true, theme).clicked() {
                view.show_map(true);
            }
            if footer_tab(ui, t(S::DiagnosticsTab), view.page == Page::Diagnostics, true, theme).clicked() {
                view.page = Page::Diagnostics;
            }
            let (rect, _) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::hover());
            let action =
                crate::panel_header::HeaderAction::toggle(SPLIT_ACTION, Icon::Columns, S::SessionSplitView, split);
            if crate::panel_header::action_button(ui, rect, &action, theme).clicked() {
                view.set_split(!split);
                tab.focus_input = true;
            }
            ui.add_space(12.0);
            let scripts = footer_button(
                ui,
                Some(Icon::Tune),
                t(S::ScriptsButton),
                view.scripts_menu,
                true,
                theme,
            )
            .on_hover_text(t(S::ScriptsMenu));
            if scripts.clicked() {
                view.scripts_menu = !view.scripts_menu;
            }
            scripts_menu(&scripts, tab, view, theme, actions);
            let on = tab.macros.switched_on();
            let macros = footer_button(ui, None, t(S::MacrosTab), on, tab.macros.has_macros(), theme)
                .on_hover_text(t(S::MacrosLiveHelp));
            if macros.clicked() {
                tab.switch_macros(!on, Instant::now());
            }
            if let Some(error) = &tab.macros.error {
                ui.add_space(8.0);
                ui.label(RichText::new(error).size(11.0).color(theme.error));
            }
            #[cfg(feature = "agent")]
            crate::agent_menu::footer_controls(ui, tab, view, theme, actions);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // On while the person turned it on; private prompts and echo-off mask the input
                // without pressing it (as the C# toggle shows ManualPrivate).
                let padlock = footer_button(
                    ui,
                    Some(Icon::Lock),
                    t(S::PrivateInput2),
                    tab.manual_private,
                    tab.is_connected(),
                    theme,
                )
                .on_hover_text(t(S::MasksYourInputAndKeepsItOutOfCommand));
                if padlock.clicked() {
                    let on = !tab.manual_private;
                    tab.set_manual_private(on);
                }
                ui.add_space(8.0);
                // No standing hint (C# UI review, item 6): only while input is private or a
                // completion waits for Tab, or a typed line's commands are still going.
                if let Some((sent, total)) = tab.queue_progress() {
                    let status = match tab.queue_waiting() {
                        Some(text) => tf(S::CommandWaiting, &[&text]),
                        None => tf(S::CommandRepeating, &[&sent, &total]),
                    };
                    ui.label(RichText::new(status).size(11.0).color(theme.accent));
                } else if ui.available_width() > 240.0 && (tab.private_input() || suggesting) {
                    let hint = footer_hint(tab.private_input(), suggesting);
                    ui.label(RichText::new(hint).size(11.0).color(theme.muted));
                }
            });
        },
    );
}

/// Width of the Scripts menu's content (the C# flyout's 340).
const SCRIPTS_MENU_WIDTH: f32 = 340.0;

/// The footer's Scripts menu (the C# `SessionScriptsButton` flyout): a switch per script with its
/// status, Reload saved rules, and Edit configuration... Switches affect this session only.
fn scripts_menu(
    button: &egui::Response,
    tab: &mut SessionTab,
    view: &mut TerminalViewState,
    theme: &Theme,
    actions: &mut ViewActions,
) {
    if !view.scripts_menu {
        return;
    }
    let frame = egui::Frame::new()
        .shadow(egui::Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: egui::Color32::from_black_alpha(48),
        })
        .fill(theme.panel)
        .stroke(egui::Stroke::new(1.0, theme.border))
        .corner_radius(6)
        .inner_margin(egui::Margin::same(12));
    let mut open = true;
    egui::Popup::from_response(button)
        .id(egui::Id::new(("scripts-menu", tab.id)))
        .open_bool(&mut open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .align(egui::RectAlign::TOP_END)
        .align_alternatives(&[])
        .gap(4.0)
        .frame(frame)
        .show(|ui| {
            ui.set_width(SCRIPTS_MENU_WIDTH);
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.horizontal(|ui| {
                ui.label(RichText::new(t(S::ScriptsButton)).size(15.0).color(theme.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let reload = crate::widgets::tool_button(ui, Icon::Reconnect, None, theme, true)
                        .on_hover_text(t(S::AutomationReload));
                    if reload.clicked() {
                        actions.reload_scripts = true;
                    }
                });
            });
            ui.add(egui::Label::new(RichText::new(t(S::AutomationLiveHint)).size(11.0).color(theme.muted)).wrap());
            let now = Instant::now();
            let mut toggled = None;
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                for entry in &tab.scripts.entries {
                    let status = tab.scripts.status(entry).label();
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(SCRIPTS_MENU_WIDTH, 38.0), egui::Sense::click());
                    let check = egui::Rect::from_min_size(
                        egui::pos2(rect.left(), rect.center().y - 10.0),
                        egui::vec2(20.0, 20.0),
                    );
                    crate::widgets::paint_check(ui, check, entry.enabled, true, theme);
                    let x = check.right() + 8.0;
                    ui.painter().text(
                        egui::pos2(x, rect.top() + 4.0),
                        egui::Align2::LEFT_TOP,
                        &entry.name,
                        egui::FontId::proportional(12.0),
                        theme.text,
                    );
                    ui.painter().text(
                        egui::pos2(x, rect.top() + 22.0),
                        egui::Align2::LEFT_TOP,
                        status,
                        egui::FontId::proportional(10.0),
                        theme.muted,
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, entry.enabled, &entry.name)
                    });
                    if response.clicked() {
                        toggled = Some((entry.id.clone(), !entry.enabled));
                    }
                }
            });
            if let Some((id, on)) = toggled {
                tab.switch_script(&id, on, now);
            }
            if tab.scripts.entries.is_empty() {
                ui.label(RichText::new(t(S::AutomationEmpty)).size(12.0).color(theme.muted));
            }
            let edit = ui.add_enabled(
                tab.world.is_some(),
                egui::Button::new(RichText::new(t(S::AutomationEdit)).size(13.0)).min_size(egui::vec2(0.0, 32.0)),
            );
            if edit.clicked() {
                actions.edit_scripts = true;
                view.scripts_menu = false;
            }
        });
    crate::a11y::name_shown_popup(
        &button.ctx,
        egui::Id::new(("scripts-menu", tab.id)),
        egui::accesskit::Role::Menu,
        t(S::ScriptsButton),
    );
    if !open {
        view.scripts_menu = false;
    }
}

/// The command hint: Tab is named only while a ghost shows (the C# `RefreshSuggestion`).
pub fn footer_hint(private: bool, suggesting: bool) -> &'static str {
    t(if private {
        S::PrivateHiddenFromEchoAndHistory
    } else if suggesting {
        S::CommandHintTabComplete
    } else {
        S::CommandHistoryEnterSend
    })
}

/// One of the footer's view tabs.
fn footer_tab(ui: &mut Ui, text: &str, selected: bool, enabled: bool, theme: &Theme) -> egui::Response {
    let font = egui::FontId::proportional(12.0);
    let color = if !enabled {
        theme.disabled_text()
    } else if selected {
        theme.text
    } else {
        theme.muted
    };
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let size = egui::vec2(galley.size().x + 20.0, 24.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    crate::a11y::toggle(&response, egui::accesskit::Role::Tab, text, selected);
    if selected {
        ui.painter().rect_filled(rect, 3.0, theme.hover_fill());
        ui.painter().hline(
            rect.shrink2(egui::vec2(6.0, 0.0)).x_range(),
            rect.bottom() - 1.0,
            egui::Stroke::new(2.0, theme.accent),
        );
    }
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, enabled, selected, text));
    response
}

/// A compact footer button; `checked` draws it pressed (a toggle that is on).
pub(crate) fn footer_button(
    ui: &mut Ui,
    icon: Option<Icon>,
    text: &str,
    checked: bool,
    enabled: bool,
    theme: &Theme,
) -> egui::Response {
    let font = egui::FontId::proportional(11.0);
    let color = if enabled { theme.text } else { theme.disabled_text() };
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let size = egui::vec2(galley.size().x + 12.0 + icon_w, 24.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    crate::a11y::toggle(&response, egui::accesskit::Role::Button, text, checked);
    if checked {
        ui.painter().rect_filled(rect, 3.0, theme.selection_fill());
    } else if enabled && response.hovered() {
        ui.painter().rect_filled(rect, 3.0, theme.hover_fill());
    }
    if let Some(icon) = icon {
        let r = egui::Rect::from_min_size(rect.min + egui::vec2(4.0, 2.0), egui::vec2(20.0, 20.0));
        crate::widgets::paint_icon(ui, icon, r, color);
    }
    ui.painter().galley(
        egui::pos2(rect.left() + 6.0 + icon_w, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    if icon == Some(Icon::Lock) || icon.is_none() {
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, checked, text));
    } else {
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text));
    }
    response
}

fn status_line(ui: &mut Ui, tab: &mut SessionTab, theme: &Theme, actions: &mut ViewActions) {
    ui.horizontal(|ui| {
        let color = match &tab.status {
            Status::Connecting | Status::Waiting { .. } => theme.warn,
            Status::Connected { .. } | Status::Demo => theme.ok,
            Status::Closed { .. } => theme.error,
        };
        Theme::dot(ui, color);
        ui.label(RichText::new(tab.title()).strong());
        ui.label(RichText::new(tab.status.label()).color(theme.muted));
        if tab.private_input() {
            ui.label(RichText::new(t(S::PrivateInput2)).color(theme.accent));
        }
        let mut protocols = Vec::new();
        if tab.gmcp_enabled {
            protocols.push(format!("GMCP {}", tab.gmcp_messages));
        }
        if let Some(mssp) = &tab.mssp {
            protocols.push(match mssp.first("NAME") {
                Some(name) => format!("MSSP: {name}"),
                None => "MSSP".into(),
            });
        }
        if !protocols.is_empty() {
            ui.label(RichText::new(protocols.join("  ")).color(theme.muted).small());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if tab.is_closed() {
                if ui.button(t(S::Reconnect)).clicked() {
                    actions.reconnect = true;
                }
                if matches!(tab.status, Status::Waiting { .. }) && ui.button(t(S::ScriptStop)).clicked() {
                    actions.disconnect = true;
                }
            } else if ui.button(t(S::Disconnect2)).clicked() {
                actions.disconnect = true;
            }
            if !tab.is_demo() && ui.button(t(S::SaveWorld)).on_hover_text(t(S::AddToMyWorlds)).clicked() {
                actions.save_world = true;
            }
            let size = tab.terminal.size();
            ui.label(
                RichText::new(tf(
                    S::GridSizeAndHistory,
                    &[
                        &size.columns,
                        &size.rows,
                        &tab.terminal.history_len(),
                        &tab.terminal.scrollback(),
                    ],
                ))
                .color(theme.muted)
                .small(),
            );
        });
    });
}

const FUNCTION_KEYS: [Key; 12] = [
    Key::F1,
    Key::F2,
    Key::F3,
    Key::F4,
    Key::F5,
    Key::F6,
    Key::F7,
    Key::F8,
    Key::F9,
    Key::F10,
    Key::F11,
    Key::F12,
];

/// The composer: Look and Commands beside the command box, the box with its ghost
/// completion, and Send. Returns whether a ghost shows.
fn composer(
    ui: &mut Ui,
    tab: &mut SessionTab,
    view: &mut TerminalViewState,
    theme: &Theme,
    font: &FontId,
    suggestions: bool,
    quiet_send: bool,
) -> bool {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), COMPOSER_HEIGHT), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme.terminal);
    ui.painter()
        .hline(rect.x_range(), rect.top(), Stroke::new(1.0, theme.border));
    let inner = rect.shrink2(egui::vec2(8.0, 5.0));
    let mut suggesting = false;
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let live = tab.is_connected() && !tab.private_input();
            let look = composer_button(ui, Icon::Eye, live, theme).on_hover_text(t(S::LookAround));
            crate::a11y::control(&look, egui::accesskit::Role::Button, t(S::LookAround));
            if look.clicked() {
                tab.run_command("look");
                tab.focus_input = true;
            }
            let commands = composer_button(ui, Icon::Grid, live, theme).on_hover_text(t(S::Controls));
            crate::a11y::control(&commands, egui::accesskit::Role::Button, t(S::Controls));
            if let Some(command) = quick_commands(&commands, theme) {
                tab.run_command(command);
                tab.focus_input = true;
            }
            ui.add_space(6.0);
            let send_width = 84.0;
            let edit_width = (ui.available_width() - send_width - 10.0).max(60.0);
            suggesting = input_line(ui, tab, view, theme, font, suggestions, edit_width);
            ui.add_space(6.0);
            // "Send ↵": the arrow is painted (the bundled fonts have no U+21B5).
            let label = t(S::Send).trim_end_matches('\u{21b5}').trim_end();
            // System: a quiet button on the composer's own surface, the text in the transcript's
            // colour (Enter does the same); the drawn skins keep the filled accent.
            let (fill, ink_color, stroke) = if quiet_send {
                (
                    crate::theme::mix(theme.terminal, theme.panel, 0.5),
                    theme.terminal_text,
                    Stroke::new(1.0, crate::theme::mix(theme.terminal, theme.terminal_text, 0.14)),
                )
            } else {
                (theme.primary(), theme.on_primary(), Stroke::NONE)
            };
            let mut text = RichText::new(format!("{label}    ")).size(14.0).color(ink_color);
            if !quiet_send {
                text = text.strong();
            }
            let send = ui.add_enabled(
                tab.is_connected(),
                egui::Button::new(text)
                    .fill(fill)
                    .stroke(stroke)
                    .min_size(egui::vec2(send_width - 8.0, 34.0)),
            );
            let c = egui::pos2(send.rect.right() - 20.0, send.rect.center().y);
            let ink = Stroke::new(1.6, ink_color);
            ui.painter()
                .line_segment([c + egui::vec2(5.0, -5.0), c + egui::vec2(5.0, 1.0)], ink);
            ui.painter()
                .line_segment([c + egui::vec2(5.0, 1.0), c + egui::vec2(-5.0, 1.0)], ink);
            ui.painter()
                .line_segment([c + egui::vec2(-5.0, 1.0), c + egui::vec2(-2.0, -2.0)], ink);
            ui.painter()
                .line_segment([c + egui::vec2(-5.0, 1.0), c + egui::vec2(-2.0, 4.0)], ink);
            if send.clicked() {
                tab.submit();
                tab.focus_input = true;
            }
        },
    );
    suggesting
}

/// A 36 point square icon button on the composer (drawn in the transcript's text colour).
fn composer_button(ui: &mut Ui, icon: Icon, enabled: bool, theme: &Theme) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(32.0, 34.0), sense);
    if enabled && response.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, crate::theme::mix(theme.terminal, theme.terminal_text, 0.12));
    }
    let color = if enabled {
        theme.terminal_text
    } else {
        crate::theme::mix(theme.terminal_text, theme.terminal, 0.55)
    };
    crate::widgets::paint_icon(ui, icon, rect, color);
    let label = t(if icon == Icon::Eye { S::LookAround } else { S::Controls });
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    response
}

/// The Commands flyout: quick commands and the compass (the C# `QuickCommands`). Returns the
/// command chosen.
fn quick_commands(button: &egui::Response, theme: &Theme) -> Option<&'static str> {
    let mut chosen = None;
    egui::Popup::menu(button)
        .align(egui::RectAlign::TOP_START)
        .width(176.0)
        .show(|ui| {
            ui.set_width(176.0);
            ui.spacing_mut().item_spacing.y = 4.0;
            let eyebrow = |ui: &mut Ui, key: S| {
                ui.label(RichText::new(t(key)).size(10.0).strong().color(theme.muted));
            };
            eyebrow(ui, S::QUICKCOMMANDS);
            for (key, command) in [
                (S::LookAround, "look"),
                (S::WhoSHere, "who"),
                (S::Inventory, "inventory"),
                (S::Help, "help"),
            ] {
                let button = egui::Button::new(RichText::new(t(key)).size(12.0)).min_size(egui::vec2(176.0, 30.0));
                if ui.add(button).on_hover_text(command).clicked() {
                    chosen = Some(command);
                }
            }
            ui.add_space(4.0);
            eyebrow(ui, S::NAVIGATION);
            let compass: [[Option<(&str, &'static str)>; 3]; 3] = [
                [None, Some(("N", "north")), None],
                [Some(("W", "west")), Some(("\u{25c8}", "look")), Some(("E", "east"))],
                [None, Some(("S", "south")), None],
            ];
            for row in compass {
                ui.horizontal(|ui| {
                    ui.add_space((176.0 - 3.0 * 28.0 - 8.0) / 2.0);
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for cell in row {
                        match cell {
                            Some((label, command)) => {
                                let button =
                                    egui::Button::new(RichText::new(label).size(12.0)).min_size(egui::vec2(28.0, 28.0));
                                if ui.add(button).on_hover_text(command).clicked() {
                                    chosen = Some(command);
                                }
                            }
                            None => {
                                ui.add_space(28.0);
                            }
                        }
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (key, command) in [(S::Up, "up"), (S::Down, "down")] {
                    let button = egui::Button::new(RichText::new(t(key)).size(12.0)).min_size(egui::vec2(86.0, 30.0));
                    if ui.add(button).clicked() {
                        chosen = Some(command);
                    }
                }
            });
            ui.label(
                RichText::new(t(S::CommandsDependOnYourWorld))
                    .size(10.0)
                    .color(theme.muted),
            );
            if chosen.is_some() {
                ui.close();
            }
        });
    crate::a11y::name_popup(
        &button.ctx,
        egui::Popup::default_response_id(button),
        egui::accesskit::Role::Menu,
        t(S::Controls),
    );
    chosen
}

/// The command box. Draws the ghost completion after the draft and takes Tab or Right (at the
/// end) to accept it and Escape to put it away. Returns whether a ghost shows.
#[allow(clippy::too_many_arguments)]
fn input_line(
    ui: &mut Ui,
    tab: &mut SessionTab,
    view: &mut TerminalViewState,
    theme: &Theme,
    font: &FontId,
    suggestions: bool,
    width: f32,
) -> bool {
    let enabled = !tab.is_closed();
    let private = tab.private_input();
    let id = egui::Id::new(("wandur-input", tab.id));
    let hint = if private {
        t(S::PrivateHiddenFromEchoAndHistory)
    } else if enabled {
        t(S::EnterACommand)
    } else {
        t(S::Disconnected)
    };
    // What the ghost would say for the draft as it stands.
    let caret_at_end = |ui: &Ui, input: &str| {
        egui::TextEdit::load_state(ui.ctx(), id)
            .and_then(|s| s.cursor.char_range())
            .is_none_or(|r| r.is_empty() && r.primary.index.0 >= input.chars().count())
    };
    if std::mem::take(&mut tab.caret_to_end) {
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        let end = egui::text::CCursor::new(tab.input.chars().count());
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
        state.store(ui.ctx(), id);
    }
    if view.dismissed.as_deref().is_some_and(|d| d != tab.input) {
        view.dismissed = None;
    }
    let ghost = |ui: &Ui, tab: &SessionTab, view: &TerminalViewState| -> Option<String> {
        if !suggestions || !enabled || view.dismissed.is_some() {
            return None;
        }
        let at_end = caret_at_end(ui, &tab.input);
        if tab.input.is_empty() || !at_end || private {
            return None;
        }
        tab.completions.with(|words| {
            wandur_core::completion::suggest(&tab.input, at_end, private, tab.history(), words).map(str::to_string)
        })
    };
    let mut suggestion = ghost(ui, tab, view);
    let focused = ui.memory(|m| m.has_focus(id));
    // Escape stops what a typed `#10 say 1` or `a;b` still has to send, before it dismisses a
    // completion.
    if focused && tab.queue_progress().is_some() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
    {
        tab.stop_queue(true);
    }
    if focused && let Some(rest) = &suggestion {
        // (The caret is read before the input lock is taken: both live in the context.)
        let at_end = caret_at_end(ui, &tab.input);
        let (tab_key, right, escape) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::Tab),
                at_end && i.key_pressed(Key::ArrowRight) && i.modifiers.is_none(),
                i.consume_key(egui::Modifiers::NONE, Key::Escape),
            )
        });
        if tab_key || right {
            if right {
                ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::ArrowRight));
            }
            tab.input.push_str(rest);
            let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            let end = egui::text::CCursor::new(tab.input.chars().count());
            state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
            state.store(ui.ctx(), id);
            suggestion = ghost(ui, tab, view);
        } else if escape {
            view.dismissed = Some(tab.input.clone());
            suggestion = None;
        }
    }
    let showing = suggestion.is_some();
    let field = crate::theme::mix(theme.terminal, theme.terminal_text, 0.06);
    let edit = egui::TextEdit::singleline(&mut tab.input)
        .id(id)
        .font(font.clone())
        .password(private)
        .text_color(theme.terminal_text)
        .background_color(field)
        .hint_text(RichText::new(hint).color(crate::theme::mix(theme.terminal_text, theme.terminal, 0.45)))
        .desired_width(width)
        .margin(egui::Margin::symmetric(10, 8))
        // While a ghost shows, Tab and Escape belong to it rather than to focus movement.
        .event_filter(egui::EventFilter {
            tab: showing,
            escape: showing,
            horizontal_arrows: true,
            vertical_arrows: true,
        })
        .return_key(None);
    let output = ui.add_enabled_ui(enabled, |ui| edit.show(ui)).inner;
    let response = output.response.response.clone();
    crate::a11y::label(&response, hint);
    if std::mem::take(&mut tab.focus_input) && enabled {
        response.request_focus();
    }
    // The ghost: muted, right after the typed text, clipped to the box. Never part of the text.
    if let Some(rest) = &suggestion {
        let end = output
            .galley
            .pos_from_cursor(egui::text::CCursor::new(tab.input.chars().count()));
        let pos = output.galley_pos + end.min.to_vec2();
        // The box's own text clip hugs the typed text, so the ghost clips to the box instead.
        let painter = ui.painter_at(response.rect.shrink2(egui::vec2(8.0, 2.0)));
        painter.text(
            pos,
            Align2::LEFT_TOP,
            rest,
            font.clone(),
            crate::theme::mix(theme.terminal_text, theme.terminal, 0.5),
        );
    }
    view.ghost = suggestion;
    // Copy: the terminal selection, unless the input line has its own selected text.
    let input_selected = output.cursor_range.is_some_and(|r| !r.is_empty());
    let copy = ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
    if copy
        && !input_selected
        && let Some(text) = tab.terminal.selection_text()
    {
        ui.ctx().copy_text(text);
    }
    if response.has_focus() {
        // F1 to F12: shortcut macros, while the command input has focus (as in C#).
        let pressed = ui.input(|i| FUNCTION_KEYS.iter().position(|k| i.key_pressed(*k)));
        if let Some(n) = pressed
            && tab.press_key(wandur_core::macros::SHORTCUT_KEYS[n], Instant::now())
        {
            ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, FUNCTION_KEYS[n]));
        }
        let (enter, up, down, page_up, page_down) = ui.input(|i| {
            (
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::PageUp),
                i.key_pressed(Key::PageDown),
            )
        });
        let page = tab.terminal.size().rows.saturating_sub(1).max(1) as i32;
        if page_up {
            tab.terminal.scroll(page);
        } else if page_down {
            tab.terminal.scroll(-page);
        }
        let mut moved = false;
        if enter {
            tab.submit();
        } else if up {
            tab.history_back();
            moved = true;
        } else if down {
            tab.history_forward();
            moved = true;
        }
        if moved {
            // Put the caret at the end of the recalled command.
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                let end = egui::text::CCursor::new(tab.input.chars().count());
                state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
                state.store(ui.ctx(), id);
            }
        }
    }
    showing
}

#[cfg(test)]
mod tests {
    use super::*;
    use wandur_term::alacritty_terminal::index::{Column, Line};

    fn runs_of(t: &Terminal, row: usize) -> Vec<(usize, usize, String)> {
        let theme = Theme::ember();
        let mut text = String::new();
        let mut runs = Vec::new();
        let mut fb = FallbackFonts::new();
        build_runs(t.view_row(row), &theme, &mut text, &mut runs, &mut fb);
        runs.iter()
            .map(|r| (r.col, r.cols, text[r.text_start..r.text_end].to_string()))
            .collect()
    }

    #[test]
    fn plain_text_is_one_run_per_style() {
        let mut t = Terminal::new(TermSize::new(30, 3), 10);
        t.feed(b"ab\x1b[31mcd\x1b[0mef   ");
        assert_eq!(
            runs_of(&t, 0),
            [(0, 2, "ab".into()), (2, 2, "cd".into()), (4, 2, "ef".into())]
        );
    }

    #[test]
    fn wide_and_special_characters_get_their_own_cells() {
        let mut t = Terminal::new(TermSize::new(30, 3), 10);
        t.feed("a漢b★c e\u{301}x".as_bytes());
        assert_eq!(
            runs_of(&t, 0),
            [
                (0, 1, "a".into()),
                (1, 2, "漢".into()),
                (3, 1, "b".into()),
                (4, 1, "★".into()),
                (5, 2, "c ".into()),
                (7, 1, "e\u{301}".into()),
                (8, 1, "x".into()),
            ]
        );
    }

    /// The output strip under the full map paints the newest rows of the one grid, the prompt
    /// last, straight from the terminal model: nothing is copied (the grid's memory is the same
    /// after), and only the strip's rows are drawn.
    #[test]
    fn the_output_strip_shows_the_newest_rows_without_copying_the_transcript() {
        use egui::{RawInput, pos2, vec2};
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::ember();
        let fonts = TermFonts::new(13.0);
        let mut fallback = FallbackFonts::new();
        let mut view = TerminalViewState::default();
        let mut term = Terminal::new(TermSize::new(80, 24), 1_000);
        for i in 0..300 {
            term.feed(format!("line {i:03}\n").as_bytes());
        }
        term.feed(b"hp 10> ");
        let bytes = term.heap_bytes();
        let newest: Vec<String> = (0..DEFAULT_STRIP_ROWS)
            .rev()
            .map(|back| row_string(term.tail_row(back).unwrap()).trim_end().to_string())
            .collect();
        assert_eq!(newest, ["line 296", "line 297", "line 298", "line 299", "hp 10>"]);
        let mut clicked = false;
        for _ in 0..2 {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0))),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                let (cell_w, row_h) =
                    ui.fonts_mut(|f| (f.glyph_width(&fonts.regular, 'M'), f.row_height(&fonts.regular)));
                view.cell = vec2(cell_w, row_h);
                let rect = Rect::from_min_size(pos2(0.0, 100.0), vec2(600.0, DEFAULT_STRIP_ROWS as f32 * row_h + 8.0));
                clicked |= output_strip(
                    ui,
                    rect,
                    &term,
                    1,
                    &mut view,
                    &theme,
                    &fonts,
                    &mut fallback,
                    &PaintOptions::default(),
                )
                .clicked();
            });
            out.textures_delta.clear();
        }
        assert!(!clicked);
        assert_eq!(view.strip_rows_drawn, DEFAULT_STRIP_ROWS);
        // Every visible character of those five rows, and nothing else.
        let ink: usize = newest.iter().map(|l| l.chars().filter(|c| *c != ' ').count()).sum();
        assert_eq!(view.strip_grid.glyphs, ink);
        assert_eq!(term.heap_bytes(), bytes, "no copy of the transcript");
        assert_eq!(view.rows_drawn, 0, "the transcript itself is not drawn");
    }

    /// The view's Play, Map and side-by-side switches.
    #[test]
    fn play_map_and_side_by_side_switches() {
        let mut view = TerminalViewState::default();
        assert!(!view.map_shown());
        assert_eq!(view.split_share(), DEFAULT_SPLIT_SHARE);
        assert_eq!(view.strip_rows(), DEFAULT_STRIP_ROWS);
        view.show_map(true);
        assert!(view.map_shown() && view.page == Page::Map);
        view.set_split(true);
        assert!(view.map_shown() && view.split);
        view.set_split(false);
        assert_eq!(view.page, Page::Map, "back to the page it was on");
        view.page = Page::Diagnostics;
        assert!(!view.map_shown());
        view.set_split(true);
        assert_eq!(view.page, Page::Play, "side by side leaves Diagnostics");
        view.show_map(false);
        assert!(!view.split && !view.map_shown());
        view.split_share = 3.0;
        assert_eq!(view.split_share(), *SPLIT_SHARES.end());
        view.strip_rows = 99;
        assert_eq!(view.strip_rows(), *STRIP_ROWS.end());
    }

    /// Drive the real widget with pointer events: press on a screen row, drag past the top edge
    /// for a while (the view scrolls into history), release, and copy.
    #[test]
    fn mouse_drag_past_the_top_selects_into_history() {
        use egui::{Event, PointerButton, RawInput, pos2, vec2};
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::ember();
        let fonts = TermFonts::new(13.0);
        let mut fallback = FallbackFonts::new();
        let mut view = TerminalViewState::default();
        let mut term = Terminal::new(TermSize::new(80, 24), 1_000);
        for i in 0..300 {
            term.feed(format!("line {i:03}\n").as_bytes());
        }
        let mut frame = |term: &mut Terminal, view: &mut TerminalViewState, events: Vec<Event>| {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0))),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    show_terminal(
                        ui,
                        term,
                        1,
                        view,
                        &theme,
                        &fonts,
                        &mut fallback,
                        &PaintOptions::default(),
                    );
                });
            });
            out.textures_delta.clear();
        };
        frame(&mut term, &mut view, vec![]);
        let rows = term.size().rows;
        assert!(rows > 5);
        let cell = view.cell;
        // The panel has an 8 point margin; aim at the middle of row 5, column 0.
        let origin = ctx.content_rect().min + vec2(8.0, 8.0);
        let press = origin + vec2(cell.x * 0.2, cell.y * 5.5);
        let pointer = |pos: Pos2, pressed: Option<bool>| {
            let mut e = vec![Event::PointerMoved(pos)];
            if let Some(pressed) = pressed {
                e.push(Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                });
            }
            e
        };
        frame(&mut term, &mut view, pointer(press, Some(true)));
        frame(&mut term, &mut view, pointer(press - vec2(0.0, 20.0), None));
        // Hold the pointer above the terminal for several frames: it scrolls up each frame.
        for _ in 0..10 {
            frame(&mut term, &mut view, pointer(pos2(press.x, -40.0), None));
        }
        assert!(term.display_offset() > 0, "dragging past the top scrolled the view");
        frame(&mut term, &mut view, pointer(pos2(press.x, -40.0), Some(false)));
        let text = term.selection_text().expect("a selection");
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            lines.len() > rows,
            "selection reaches beyond one screen: {} lines",
            lines.len()
        );
        // It ends where the drag began: the left half of column 0 of screen row 5, which held
        // "line N" with N = 300 - rows + 6 (the last screen row is the empty cursor row). The
        // left half excludes that cell, so the selection ends with the line before.
        let anchor = 300 - (rows - 1) + 5;
        assert_eq!(
            lines.last().copied(),
            Some(format!("line {:03}", anchor - 1).as_str()),
            "{text}"
        );
        // Whole lines, no wrap artefacts, consecutive numbers.
        for pair in lines[1..].windows(2) {
            let a: usize = pair[0][5..].parse().unwrap();
            let b: usize = pair[1][5..].parse().unwrap();
            assert_eq!(a + 1, b);
        }
        // New output does not disturb it.
        let before = text.clone();
        for i in 0..20 {
            term.feed(format!("more {i}\n").as_bytes());
        }
        assert_eq!(term.selection_text().as_deref(), Some(before.as_str()));
    }

    /// A long chat line is drawn word wrapped; dragging from its first word to past the end of
    /// its last display row selects grid cells, and copying gives the line exactly as received.
    #[test]
    fn dragging_over_word_wrapped_rows_copies_the_original_line() {
        use egui::{Event, PointerButton, RawInput, pos2, vec2};
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::ember();
        let fonts = TermFonts::new(13.0);
        let mut fallback = FallbackFonts::new();
        let mut view = TerminalViewState::default();
        let mut term = Terminal::new(TermSize::new(80, 24), 1_000);
        let line = "[gossip] Talek: has anyone restocked with hunted goods at the market yet, \
                    or are the traders still waiting on the caravan from the river road to the north";
        term.feed(format!("before\n\x1b[36m{line}\x1b[0m\nafter\n").as_bytes());
        let mut frame = |term: &mut Terminal, view: &mut TerminalViewState, events: Vec<Event>| {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0))),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    show_terminal(
                        ui,
                        term,
                        1,
                        view,
                        &theme,
                        &fonts,
                        &mut fallback,
                        &PaintOptions::default(),
                    );
                });
            });
            out.textures_delta.clear();
        };
        frame(&mut term, &mut view, vec![]);
        let columns = term.size().columns;
        assert!(line.len() > 2 * columns, "the line spans several grid rows");
        // The display rows of the line: every one ends at a word (the grid's rows cut words).
        let start = term.line_at(Point::new(Line(0), Column(0))).0;
        let at = if start.starts_with("[gossip]") {
            Line(0)
        } else {
            Line(1)
        };
        let rows: Vec<DisplayRow> = view.display.iter().copied().filter(|r| r.line == at).collect();
        assert!(rows.len() >= 3, "{:?} {:?}", view.display, term.size());
        let mut shown = Vec::new();
        for row in &rows {
            let mut segments = Vec::new();
            term.row_segments(row, &mut segments);
            let text: String = segments.iter().flat_map(|(c, _)| c.iter().map(|c| c.c)).collect();
            shown.push(text.trim_end().to_string());
        }
        assert_eq!(shown.join(" "), line);
        let first = view.display.iter().position(|r| *r == rows[0]).unwrap();
        let last = first + rows.len() - 1;
        let cell = view.cell;
        let origin = ctx.content_rect().min + vec2(8.0, 8.0);
        let press = origin + vec2(cell.x * 0.2, cell.y * (first as f32 + 0.5));
        let release = origin + vec2(cell.x * (columns as f32 - 0.5), cell.y * (last as f32 + 0.5));
        let pointer = |pos: Pos2, pressed: Option<bool>| {
            let mut e = vec![Event::PointerMoved(pos)];
            if let Some(pressed) = pressed {
                e.push(Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                });
            }
            e
        };
        frame(&mut term, &mut view, pointer(press, Some(true)));
        frame(&mut term, &mut view, pointer(press + vec2(30.0, 10.0), None));
        frame(&mut term, &mut view, pointer(release, None));
        frame(&mut term, &mut view, pointer(release, Some(false)));
        assert_eq!(term.selection_text().as_deref(), Some(line), "no inserted line breaks");
    }

    /// Limit text width: in a pane wider than the limit, the grid is that many columns and the
    /// transcript is centred; in a narrower pane it fills the pane as before.
    #[test]
    fn limit_text_width_centres_a_narrower_grid() {
        use egui::{RawInput, pos2, vec2};
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::ember();
        let fonts = TermFonts::new(13.0);
        let mut fallback = FallbackFonts::new();
        let mut view = TerminalViewState::default();
        let mut term = Terminal::new(TermSize::new(80, 24), 100);
        let options = PaintOptions {
            max_columns: Some(60),
            ..PaintOptions::default()
        };
        let mut run = |width: f32, term: &mut Terminal, view: &mut TerminalViewState| {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(width, 400.0))),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    show_terminal(ui, term, 1, view, &theme, &fonts, &mut fallback, &options);
                });
            });
            out.textures_delta.clear();
            ctx.content_rect()
        };
        let screen = run(1200.0, &mut term, &mut view);
        assert_eq!(term.size().columns, 60);
        let text = view.transcript_rect.unwrap();
        assert!((text.width() - 60.0 * view.cell.x).abs() < 0.5);
        // Centred: the space left and right of it differs by less than a cell and the scrollbar.
        let left = text.left() - screen.left();
        let right = screen.right() - text.right();
        assert!((left - right).abs() < view.cell.x + SCROLLBAR + 20.0, "{left} {right}");
        assert!(left > 100.0);
        // A pane narrower than the limit: the grid fills it.
        run(400.0, &mut term, &mut view);
        assert!(term.size().columns < 60);
        assert!(view.transcript_rect.unwrap().left() < 20.0);
    }

    /// Plain text goes into one mesh: one glyph quad per visible character, each character laid
    /// out once, and the next frame reuses the buffers without new layouts.
    #[test]
    fn plain_text_is_one_reused_mesh() {
        use egui::{RawInput, pos2, vec2};
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::ember();
        let fonts = TermFonts::new(13.0);
        let mut fallback = FallbackFonts::new();
        let mut view = TerminalViewState::default();
        let mut term = Terminal::new(TermSize::new(80, 24), 100);
        term.feed(b"abc \x1b[1mbold\x1b[0m \x1b[3mit\x1b[0m\n");
        let mut frame = |term: &mut Terminal, view: &mut TerminalViewState| {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0))),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    show_terminal(
                        ui,
                        term,
                        1,
                        view,
                        &theme,
                        &fonts,
                        &mut fallback,
                        &PaintOptions::default(),
                    );
                });
            });
            out.textures_delta.clear();
            out.shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) => Some(t.galley.num_vertices),
                    _ => None,
                })
                .max()
                .unwrap_or(0)
        };
        let vertices = frame(&mut term, &mut view);
        assert_eq!(view.grid.glyphs, 3 + 4 + 2);
        assert_eq!(vertices, view.grid.glyphs * 4, "one quad per glyph");
        assert_eq!(view.grid.laid_out, 9, "each character and style laid out once");
        frame(&mut term, &mut view);
        assert_eq!(view.grid.laid_out, 0, "known glyphs need no layout");
        term.feed(b"abc again\n");
        frame(&mut term, &mut view);
        assert_eq!(view.grid.laid_out, 3, "only g, n and a regular i are new");
    }

    #[test]
    fn block_elements_are_painted_cells() {
        let mut t = Terminal::new(TermSize::new(30, 3), 10);
        t.feed("█▀a".as_bytes());
        assert_eq!(
            runs_of(&t, 0),
            [(0, 1, "█".into()), (1, 1, "▀".into()), (2, 1, "a".into())]
        );
        assert_eq!(block_rects('█').unwrap().0[0], [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(block_rects('▄').unwrap().0[0], [0.0, 0.5, 1.0, 1.0]);
        assert_eq!(block_rects('▌').unwrap().0[0], [0.0, 0.0, 0.5, 1.0]);
        assert_eq!(block_rects('▒').unwrap().2, 0.5);
        assert!(block_rects('─').is_none());
    }

    #[test]
    fn bold_basic_colours_are_bright_and_inverse_swaps() {
        let theme = Theme::ember();
        let mut t = Terminal::new(TermSize::new(30, 3), 10);
        t.feed(b"\x1b[1;31mA\x1b[0;7mB");
        let row = t.view_row(0);
        let a = look_of(&row[0], &theme);
        assert_eq!(a.fg, theme.ansi[9]);
        assert!(a.bold);
        let b = look_of(&row[1], &theme);
        assert_eq!(b.bg, theme.terminal_text);
        assert_eq!(b.fg, theme.terminal);
    }

    fn key(key: Key) -> Vec<egui::Event> {
        vec![
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    fn row_string(cells: &[Cell]) -> String {
        let s: String = cells.iter().map(|c| c.c).collect();
        s.trim_end().to_string()
    }

    /// ComposerCompletionTests.GhostFollowsSeenWordsAndHistoryAndTheKeysAcceptOrDismissIt: a
    /// muted ghost after the typed text, learned from public lines and the history, taken with
    /// Tab or Right, put away with Escape, absent while private or with the setting off.
    #[test]
    fn the_ghost_follows_seen_words_and_history_and_the_keys_accept_or_dismiss_it() {
        use crate::session_tab::test_support::*;
        use egui::{RawInput, pos2, vec2};
        use std::io::Write;
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::preset("Hull");
        let fonts = TermFonts::new(13.0);
        let mut fallback = FallbackFonts::new();
        let mut view = TerminalViewState::default();
        let (mut tab, mut server) = local_tab();
        pump_until(&mut tab, SessionTab::is_connected);
        let mut options = PaintOptions::default();
        let mut frame =
            |tab: &mut SessionTab, view: &mut TerminalViewState, options: &PaintOptions, events: Vec<egui::Event>| {
                let input = RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 550.0))),
                    events,
                    ..Default::default()
                };
                let mut out = ctx.run_ui(input, |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        show(ui, tab, view, &theme, &fonts, &mut fallback, options);
                    });
                });
                out.textures_delta.clear();
                // The ghost is drawn: a text shape with exactly its text, inside the screen.
                if let Some(ghost) = &view.ghost {
                    let drawn = out.shapes.iter().any(|c| match &c.shape {
                        egui::Shape::Text(t) => {
                            t.galley.text() == ghost && c.clip_rect.contains(t.pos + t.galley.size() / 2.0)
                        }
                        _ => false,
                    });
                    assert!(drawn, "ghost {ghost:?} drawn");
                }
            };
        let type_text = |tab: &mut SessionTab, text: &str| {
            tab.input = text.into();
            tab.caret_to_end = true;
        };
        frame(&mut tab, &mut view, &options, vec![]);
        frame(&mut tab, &mut view, &options, vec![]);
        assert_eq!(view.ghost, None);

        // A public line teaches the words in it.
        server.write_all(b"A Vicious Womprat scurries past.\r\n").unwrap();
        pump_until(&mut tab, |t| !t.completions.with(|w| w.suggest("wom", 1)).is_empty());
        type_text(&mut tab, "wom");
        frame(&mut tab, &mut view, &options, vec![]);
        assert_eq!(view.ghost.as_deref(), Some("prat"));
        assert_eq!(footer_hint(false, view.ghost.is_some()), t(S::CommandHintTabComplete));

        // Tab takes the whole ghost and sends nothing.
        let history = tab.history().len();
        frame(&mut tab, &mut view, &options, key(Key::Tab));
        assert_eq!(tab.input, "womprat");
        assert_eq!(view.ghost, None);
        assert_eq!(tab.history().len(), history);
        assert_eq!(footer_hint(false, view.ghost.is_some()), t(S::CommandHistoryEnterSend));

        // A sent command becomes a line to complete, and the line wins over the word.
        assert!(tab.run_command("look womprat"));
        assert!(tab.completions.settle(std::time::Duration::from_secs(5)));
        type_text(&mut tab, "look");
        frame(&mut tab, &mut view, &options, vec![]);
        assert_eq!(view.ghost.as_deref(), Some(" womprat"));
        type_text(&mut tab, "loo");
        frame(&mut tab, &mut view, &options, vec![]);
        assert_eq!(view.ghost.as_deref(), Some("k womprat"));

        // Escape puts it away until the draft changes; Right at the end takes it.
        frame(&mut tab, &mut view, &options, key(Key::Escape));
        assert_eq!(view.ghost, None);
        assert_eq!(tab.input, "loo");
        frame(&mut tab, &mut view, &options, vec![]);
        assert_eq!(view.ghost, None, "still put away");
        type_text(&mut tab, "look");
        frame(&mut tab, &mut view, &options, vec![]);
        assert_eq!(view.ghost.as_deref(), Some(" womprat"));
        frame(&mut tab, &mut view, &options, key(Key::ArrowRight));
        assert_eq!(tab.input, "look womprat");
        assert_eq!(view.ghost, None);

        // Away from the end of the text there is nothing to offer.
        type_text(&mut tab, "loo");
        frame(&mut tab, &mut view, &options, vec![]);
        assert!(view.ghost.is_some());
        let id = egui::Id::new(("wandur-input", tab.id));
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(1))));
        state.store(&ctx, id);
        frame(&mut tab, &mut view, &options, vec![]);
        assert_eq!(view.ghost, None);

        // Nothing is offered while private.
        tab.set_manual_private(true);
        type_text(&mut tab, "wom");
        frame(&mut tab, &mut view, &options, vec![]);
        assert_eq!(view.ghost, None);
        tab.set_manual_private(false);

        // The setting turns the ghost off at once, and Tab then does nothing to the draft.
        options.suggestions = false;
        type_text(&mut tab, "wom");
        frame(&mut tab, &mut view, &options, vec![]);
        assert_eq!(view.ghost, None);
        frame(&mut tab, &mut view, &options, key(Key::Tab));
        assert_eq!(tab.input, "wom");
        options.suggestions = true;
        type_text(&mut tab, "wom");
        frame(&mut tab, &mut view, &options, vec![]);
        assert_eq!(view.ghost.as_deref(), Some("prat"));
    }

    /// Custom terminal colours and the text and background overrides are read when a frame is
    /// painted: the same grid, unchanged, paints in the new colours.
    #[test]
    fn palette_and_overrides_apply_at_paint_time_without_rebuilding_the_grid() {
        use wandur_core::settings::{CustomTheme, Settings};
        let mut t = Terminal::new(TermSize::new(30, 3), 10);
        t.feed(b"\x1b[31mred\x1b[0m plain");
        let revision = t.revision();
        let looks = |theme: &Theme| {
            let row = t.view_row(0);
            (look_of(&row[0], theme).fg, look_of(&row[4], theme).fg)
        };
        let preset = Theme::preset("Hull");
        assert_eq!(looks(&preset), (preset.ansi[1], preset.terminal_text));
        let mut custom = CustomTheme {
            name: "Hull copy 1".into(),
            base: "Hull".into(),
            ..CustomTheme::default()
        };
        custom.ansi_colors.insert(1, "#112233".into());
        let settings = Settings {
            theme: custom.id.clone(),
            custom_themes: vec![custom],
            foreground: Some("#445566".into()),
            ..Settings::default()
        };
        let edited = Theme::from_settings(&settings);
        assert_eq!(
            looks(&edited),
            (Color32::from_rgb(0x11, 0x22, 0x33), Color32::from_rgb(0x44, 0x55, 0x66))
        );
        assert_eq!(t.revision(), revision, "the grid was not touched");
    }

    /// TranscriptTailTests: the live view appears only while scrolled back, at the share the
    /// settings give; new output reaches it without moving the transcript; dragging the divider
    /// reports a new share to save; a click on it returns to the tail; share 0 turns it off.
    #[test]
    fn the_live_view_shows_only_while_scrolled_back_and_follows_the_tail() {
        use egui::{Event, PointerButton, RawInput, pos2, vec2};
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::preset("Hull");
        let fonts = TermFonts::new(13.0);
        let mut fallback = FallbackFonts::new();
        let mut view = TerminalViewState::default();
        let mut term = Terminal::new(TermSize::new(80, 24), 1_000);
        for i in 0..200 {
            term.feed(format!("Line {i}\n").as_bytes());
        }
        let mut frame = |term: &mut Terminal, view: &mut TerminalViewState, share: f32, events: Vec<Event>| {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 550.0))),
                events,
                ..Default::default()
            };
            let mut result = TerminalOutput::default();
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let options = PaintOptions {
                        tail_share: share,
                        ..PaintOptions::default()
                    };
                    result = show_terminal(ui, term, 1, view, &theme, &fonts, &mut fallback, &options);
                });
            });
            out.textures_delta.clear();
            result
        };
        let out = frame(&mut term, &mut view, 0.25, vec![]);
        assert_eq!(out.tail_rect, None, "following the tail: no split");
        let whole = out.transcript_rect.unwrap().height();

        term.scroll(5);
        let out = frame(&mut term, &mut view, 0.25, vec![]);
        let tail = out.tail_rect.expect("scrolled back: the live view");
        let above = out.transcript_rect.unwrap().height();
        let share = tail.height() / (above + tail.height());
        assert!((0.23..=0.27).contains(&share), "share {share}");
        assert!(above < whole);
        assert!(view.tail_rows_drawn >= 3, "{} rows", view.tail_rows_drawn);
        assert_eq!(term.tail_row(0).map(row_string).as_deref(), Some("Line 199"));
        assert_eq!(
            term.tail_row(view.tail_rows_drawn - 1).map(row_string),
            Some(format!("Line {}", 200 - view.tail_rows_drawn))
        );

        // New output reaches the live view; the transcript stays where the reader is.
        let top = row_string(term.view_row(0));
        for i in 200..205 {
            term.feed(format!("Line {i}\n").as_bytes());
        }
        let out = frame(&mut term, &mut view, 0.25, vec![]);
        assert!(out.tail_rect.is_some());
        assert_eq!(row_string(term.view_row(0)), top);
        assert_eq!(term.tail_row(0).map(row_string).as_deref(), Some("Line 204"));

        // Drag the divider up: a larger share is reported once, when the drag ends.
        let divider = view.divider_rect.unwrap().center();
        let to = divider - vec2(0.0, 80.0);
        let button = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(
            &mut term,
            &mut view,
            0.25,
            vec![Event::PointerMoved(divider), button(divider, true)],
        );
        frame(
            &mut term,
            &mut view,
            0.25,
            vec![Event::PointerMoved(divider - vec2(0.0, 10.0))],
        );
        let dragging = frame(&mut term, &mut view, 0.25, vec![Event::PointerMoved(to)]);
        assert_eq!(dragging.share, None);
        let released = frame(&mut term, &mut view, 0.25, vec![button(to, false)]);
        let saved = released.share.expect("the share settled on");
        assert!(saved > 0.3 && saved <= 0.6, "{saved}");
        assert_eq!(row_string(term.view_row(0)), top, "the reader's place is kept");

        // The setting sizes the split; 0 turns it off.
        let out = frame(&mut term, &mut view, 0.5, vec![]);
        let tail = out.tail_rect.unwrap();
        let share = tail.height() / (out.transcript_rect.unwrap().height() + tail.height());
        assert!((0.48..=0.52).contains(&share), "share {share}");
        let out = frame(&mut term, &mut view, 0.0, vec![]);
        assert_eq!(out.tail_rect, None);

        // A click on the live view returns the transcript to the latest output.
        let out = frame(&mut term, &mut view, 0.25, vec![]);
        let inside = out.tail_rect.unwrap().center();
        frame(
            &mut term,
            &mut view,
            0.25,
            vec![Event::PointerMoved(inside), button(inside, true)],
        );
        frame(&mut term, &mut view, 0.25, vec![button(inside, false)]);
        assert_eq!(term.display_offset(), 0);
        let out = frame(&mut term, &mut view, 0.25, vec![]);
        assert_eq!(out.tail_rect, None);
        assert_eq!(out.transcript_rect.unwrap().height(), whole);
    }

    /// LinkClickTests: a Ctrl+click (Cmd+click on macOS) on a web address asks first with the
    /// whole address; Open hands it over; Cancel drops it; other schemes, user info, look-alike
    /// characters and local hosts are refused with their notice and never offered.
    #[test]
    fn a_clicked_link_asks_before_opening_and_unsafe_links_are_refused() {
        use egui::{Event, PointerButton, RawInput, pos2, vec2};
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::preset("Hull");
        let fonts = TermFonts::new(13.0);
        let mut fallback = FallbackFonts::new();
        let mut view = TerminalViewState::default();
        let mut term = Terminal::new(TermSize::new(80, 24), 100);
        term.feed(b"See https://www.wandur.net/help, then come back.\nOr http://localhost:8080/ there.\n");
        let mut frame = |term: &mut Terminal, view: &mut TerminalViewState, mut events: Vec<Event>| {
            // The keys held come with the input, as a window reports them.
            let modifiers = events.iter().find_map(|e| match e {
                Event::PointerButton { modifiers, .. } => Some(*modifiers),
                _ => None,
            });
            if let Some(modifiers) = modifiers {
                events.insert(0, Event::ModifiersChanged(modifiers));
            }
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 550.0))),
                events,
                ..Default::default()
            };
            let mut result = TerminalOutput::default();
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    result = show_terminal(
                        ui,
                        term,
                        1,
                        view,
                        &theme,
                        &fonts,
                        &mut fallback,
                        &PaintOptions::default(),
                    );
                });
            });
            out.textures_delta.clear();
            result
        };
        frame(&mut term, &mut view, vec![]);
        let origin = view.transcript_rect.unwrap().min;
        let cell = view.cell;
        let at = |col: usize, row: usize| origin + vec2((col as f32 + 0.5) * cell.x, (row as f32 + 0.5) * cell.y);
        let modifiers = if cfg!(target_os = "macos") {
            egui::Modifiers::MAC_CMD
        } else {
            egui::Modifiers::CTRL
        };
        let click = |pos, modifiers| {
            vec![
                vec![
                    Event::PointerMoved(pos),
                    Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers,
                    },
                ],
                vec![Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed: false,
                    modifiers,
                }],
            ]
        };
        // A plain click on the link only moves the keyboard; it opens nothing.
        let mut outs = Vec::new();
        for events in click(at(10, 0), egui::Modifiers::NONE) {
            outs.push(frame(&mut term, &mut view, events));
        }
        assert_eq!(view.pending_link, None);
        // With the modifier it asks.
        for events in click(at(10, 0), modifiers) {
            frame(&mut term, &mut view, events);
        }
        assert_eq!(view.pending_link.as_deref(), Some("https://www.wandur.net/help"));
        // The text after the link is not the link.
        view.pending_link = None;
        for events in click(at(36, 0), modifiers) {
            frame(&mut term, &mut view, events);
        }
        assert_eq!(view.pending_link, None);
        // A local address is refused with its own notice.
        let mut notice = None;
        for events in click(at(7, 1), modifiers) {
            notice = notice.or(frame(&mut term, &mut view, events).notice);
        }
        assert_eq!(notice, Some(S::LinkRefusedLocal));
        assert_eq!(view.pending_link, None);

        // The refusal rules, as RequestOpenLink applies them.
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "mailto:someone@example.test",
            "https://user:secret@example.test/",
            "https://bank.example@evil.example/login",
            "https://wandur.net\u{202e}gnp.exe",
            "https://\u{430}pple.com/",
            "https://app\u{200b}le.com/",
            "https://example\u{ff0e}com/",
        ] {
            assert_eq!(request_link(&mut view, url), Err(S::LinkRefused), "{url:?}");
            assert_eq!(view.pending_link, None);
        }
        for url in [
            "http://localhost:8080/",
            "http://192.168.1.1/admin",
            "http://0x7f.1/",
            "http://[::1]/",
        ] {
            assert_eq!(request_link(&mut view, url), Err(S::LinkRefusedLocal), "{url}");
        }

        // Cancel closes the bar; Open hands the address over and closes it.
        assert_eq!(request_link(&mut view, "http://example.test/page"), Ok(()));
        frame(&mut term, &mut view, vec![]);
        frame(&mut term, &mut view, vec![]);
        let (_, cancel) = view.link_buttons.expect("the bar shows Open and Cancel");
        let mut opened = None;
        for events in click(cancel.center(), egui::Modifiers::NONE) {
            opened = opened.or(frame(&mut term, &mut view, events).open_link);
        }
        assert_eq!((opened.clone(), view.pending_link.clone()), (None, None));
        assert_eq!(request_link(&mut view, "http://example.test/page"), Ok(()));
        frame(&mut term, &mut view, vec![]);
        frame(&mut term, &mut view, vec![]);
        let (open, _) = view.link_buttons.unwrap();
        for events in click(open.center(), egui::Modifiers::NONE) {
            opened = opened.or(frame(&mut term, &mut view, events).open_link);
        }
        assert_eq!(opened.as_deref(), Some("http://example.test/page"));
        assert_eq!(view.pending_link, None);
        frame(&mut term, &mut view, vec![]);
        assert_eq!(view.link_buttons, None);
    }

    /// Blinking text hides in every other half second when allowed, and stays steady when not.
    #[test]
    fn blinking_text_blinks_only_when_allowed() {
        use egui::{RawInput, pos2, vec2};
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        let theme = Theme::preset("Hull");
        let fonts = TermFonts::new(13.0);
        let mut fallback = FallbackFonts::new();
        let mut view = TerminalViewState::default();
        let mut term = Terminal::new(TermSize::new(80, 24), 100);
        term.feed(b"steady \x1b[5mblink\x1b[0m\n");
        let mut frame = |view: &mut TerminalViewState, time: f64, blink: bool| {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(600.0, 400.0))),
                time: Some(time),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let options = PaintOptions {
                        blink,
                        ..PaintOptions::default()
                    };
                    show_terminal(ui, &mut term, 1, view, &theme, &fonts, &mut fallback, &options);
                });
            });
            out.textures_delta.clear();
            view.grid.glyphs
        };
        let shown = frame(&mut view, 0.1, true);
        assert!(view.blink_seen);
        let hidden = frame(&mut view, 0.6, true);
        assert_eq!(shown - hidden, 5, "blink hides in its second half");
        assert_eq!(frame(&mut view, 0.6, false), shown, "not allowed: steady");
    }
}
