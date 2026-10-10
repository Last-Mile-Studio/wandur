//! The terminal model of one session: an `alacritty_terminal` grid fed directly with server text
//! (no PTY), and everything line oriented read back from that grid.
//!
//! One canonical model: the grid is the transcript. It holds the visible screen and a bounded
//! scrollback of rows, reflows on resize, knows wide characters and owns the selection. Consumers
//! that think in lines do not keep a second transcript:
//!
//! - activity uses the counters [`Terminal::lines_total`] and [`Terminal::revision`];
//! - prompt detection reads [`Terminal::current_line`] from the grid when it needs it;
//! - line events (for future triggers and channels) are opt in: when on, each completed logical
//!   line is read back from the grid as it completes and queued, bounded, until the caller takes
//!   them with [`Terminal::take_lines`];
//! - transcript export is [`Terminal::transcript`], built from the grid on request.
//!
//! MUD servers often send a bare line feed; a terminal would only move down. [`Terminal::feed`]
//! treats every LF as CR LF (what xterm.js calls `convertEol`), which is also where completed lines
//! are counted.

use std::collections::VecDeque;

mod blink;
mod wrap;
pub use blink::BLINK;
use blink::Feeder;
pub use wrap::{DisplayRow, MAX_WRAP_ROWS, WrapOptions, wrap_line};

pub use alacritty_terminal;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::term::cell::{Cell, Flags, LineLength};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{ClearMode, Color, Handler, Processor};

/// Default scrollback, in rows (the C# client keeps 2,000 display rows).
pub const DEFAULT_SCROLLBACK: usize = 2_000;
/// Hard maximum scrollback in rows, whatever the settings say. A row holds one cell per column
/// (24 bytes each), so 50,000 rows at 120 columns is about 140 MB.
pub const MAX_SCROLLBACK: usize = 50_000;
/// Line events kept between two [`Terminal::take_lines`] calls; older ones are dropped and counted.
pub const MAX_PENDING_LINES: usize = 4_096;

/// Grid size in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TermSize {
    pub columns: usize,
    pub rows: usize,
}

impl TermSize {
    pub fn new(columns: usize, rows: usize) -> Self {
        Self {
            columns: columns.max(2),
            rows: rows.max(1),
        }
    }
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// A completed logical line, read from the grid when its line feed arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletedLine {
    /// Sequence number: the value of [`Terminal::lines_total`] after this line completed.
    pub seq: u64,
    /// Plain text, soft wraps joined, trailing blanks removed.
    pub text: String,
}

/// How a selection grows from where it started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionKind {
    /// Character by character.
    Simple,
    /// Whole words (double click).
    Word,
    /// Whole lines (triple click).
    Line,
}

/// The grid of one session plus its counters.
pub struct Terminal {
    term: Term<VoidListener>,
    /// The one escape sequence parser. vte reserves 2 MB inside it for synchronized updates
    /// (untouched pages: address space, not resident memory).
    parser: Processor,
    scrollback: usize,
    lines_total: u64,
    revision: u64,
    collect_lines: bool,
    pending: VecDeque<CompletedLine>,
    /// Line texts handed back by [`Self::recycle_lines`], reused for the next lines.
    spare_texts: Vec<String>,
    dropped_lines: u64,
    /// Spare rows were released after the history filled up (see [`Self::release_spare_rows`]).
    trimmed: bool,
}

impl Terminal {
    /// A terminal of `size` keeping up to `scrollback` rows of history (capped at [`MAX_SCROLLBACK`]).
    pub fn new(size: TermSize, scrollback: usize) -> Self {
        let scrollback = scrollback.min(MAX_SCROLLBACK);
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        Self {
            term: Term::new(config, &size, VoidListener),
            parser: Processor::new(),
            scrollback,
            lines_total: 0,
            revision: 0,
            collect_lines: false,
            pending: VecDeque::new(),
            spare_texts: Vec::new(),
            dropped_lines: 0,
            trimmed: false,
        }
    }

    /// Feed server text (escape sequences included). LF is treated as CR LF.
    pub fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut rest = bytes;
        while let Some(i) = rest.iter().position(|&b| b == b'\n') {
            let mut feeder = Feeder { term: &mut self.term };
            self.parser.advance(&mut feeder, &rest[..i]);
            self.parser.advance(&mut feeder, b"\r\n");
            self.line_completed(true);
            rest = &rest[i + 1..];
        }
        self.parser.advance(&mut Feeder { term: &mut self.term }, rest);
        self.revision += 1;
        self.release_spare_rows();
    }

    /// alacritty grows its row storage up to 1,000 rows at a time and keeps the spare rows. Once
    /// the history is full, scrolling only rotates rows already in use, so the spares are never
    /// needed: release them once (about 2.8 MB at 120 columns).
    fn release_spare_rows(&mut self) {
        if !self.trimmed && self.term.history_size() >= self.scrollback {
            self.term.grid_mut().truncate();
            self.trimmed = true;
        }
    }

    /// Write text the client produced (local echo, notices) in the given palette colour. Lines it
    /// completes raise no line event (triggers and channels see server output only). It goes
    /// straight to the grid, not through the parser, so a server escape sequence that is still
    /// incomplete is not disturbed, and the server's colours are restored afterwards.
    pub fn feed_local(&mut self, text: &str, color: u8) {
        let saved = self.term.grid().cursor.template.clone();
        self.term.grid_mut().cursor.template = Cell {
            fg: Color::Indexed(color),
            ..Cell::default()
        };
        for c in text.chars() {
            if c == '\n' {
                Handler::carriage_return(&mut self.term);
                Handler::linefeed(&mut self.term);
                // Client text (echo, notices) is not server output: no line event.
                self.line_completed(false);
            } else if !c.is_control() {
                Handler::input(&mut self.term, c);
            }
        }
        self.term.grid_mut().cursor.template = saved;
        self.revision += 1;
    }

    fn line_completed(&mut self, server: bool) {
        self.lines_total += 1;
        if !self.collect_lines || !server {
            return;
        }
        let end = self.term.grid().cursor.point.line - 1i32;
        if end < self.term.topmost_line() {
            return;
        }
        let mut text = if self.pending.len() >= MAX_PENDING_LINES {
            self.dropped_lines += 1;
            self.pending.pop_front().map(|l| l.text).unwrap_or_default()
        } else {
            self.spare_texts.pop().unwrap_or_default()
        };
        text.clear();
        self.logical_text_into(end, None, &mut text);
        self.pending.push_back(CompletedLine {
            seq: self.lines_total,
            text,
        });
    }

    /// The text of the logical line that ends on grid row `end` (joining rows wrapped into it),
    /// up to column `until` on that row, or the whole row.
    fn logical_text(&self, end: Line, until: Option<Column>) -> String {
        let mut text = String::new();
        self.logical_text_into(end, until, &mut text);
        text
    }

    fn logical_text_into(&self, end: Line, until: Option<Column>, text: &mut String) {
        let grid = self.term.grid();
        let last = self.term.last_column();
        let mut start = end;
        while start > self.term.topmost_line() && grid[start - 1i32][last].flags.contains(Flags::WRAPLINE) {
            start -= 1i32;
        }
        let mut line = start;
        while line <= end {
            let row = &grid[line];
            let stop = if line == end {
                until.map_or(row.line_length().0, |c| c.0.min(self.term.columns()))
            } else {
                self.term.columns()
            };
            for cell in &row[..][..stop] {
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                text.push(if cell.c == '\t' { ' ' } else { cell.c });
                if let Some(zw) = cell.zerowidth() {
                    text.extend(zw.iter());
                }
            }
            line += 1i32;
        }
        let trimmed = text.trim_end_matches(' ').len();
        text.truncate(trimmed);
    }

    /// The line the cursor is on (a prompt, usually), from its start up to the cursor.
    pub fn current_line(&self) -> String {
        let cursor = self.term.grid().cursor.point;
        self.logical_text(cursor.line, Some(cursor.column))
    }

    /// Whether the cursor sits at the start of an empty line (no unterminated text).
    pub fn at_line_start(&self) -> bool {
        let cursor = self.term.grid().cursor.point;
        cursor.column.0 == 0
            && !(cursor.line > self.term.topmost_line()
                && self.term.grid()[cursor.line - 1i32][self.term.last_column()]
                    .flags
                    .contains(Flags::WRAPLINE))
    }

    /// Turn line events on or off. Off by default: nothing is read back or kept.
    pub fn set_line_events(&mut self, on: bool) {
        self.collect_lines = on;
        if !on {
            self.pending.clear();
        }
    }

    /// Take the lines completed since the last call (oldest first).
    pub fn take_lines(&mut self, into: &mut Vec<CompletedLine>) {
        into.extend(self.pending.drain(..));
    }

    /// Take the texts of the lines completed since the last call (oldest first), without their
    /// sequence numbers.
    pub fn take_line_texts(&mut self, into: &mut Vec<String>) {
        into.extend(self.pending.drain(..).map(|l| l.text));
    }

    /// Hand line texts back once read (see [`Self::recycle_lines`]). Empties `texts`.
    pub fn recycle_texts(&mut self, texts: &mut Vec<String>) {
        let room = MAX_PENDING_LINES.saturating_sub(self.spare_texts.len());
        self.spare_texts.extend(texts.drain(..).take(room));
        texts.clear();
    }

    /// Hand taken lines back once read: their text buffers are reused for the next lines, so a
    /// steady stream of line events allocates nothing. Empties `lines`.
    pub fn recycle_lines(&mut self, lines: &mut Vec<CompletedLine>) {
        let room = MAX_PENDING_LINES.saturating_sub(self.spare_texts.len());
        self.spare_texts.extend(lines.drain(..).take(room).map(|l| l.text));
        lines.clear();
    }

    /// Line events dropped because nobody took them in time.
    pub fn dropped_lines(&self) -> u64 {
        self.dropped_lines
    }

    /// Completed lines (line feeds) since creation; never decreases.
    pub fn lines_total(&self) -> u64 {
        self.lines_total
    }

    /// Changes on every feed.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn size(&self) -> TermSize {
        TermSize {
            columns: self.term.columns(),
            rows: self.term.screen_lines(),
        }
    }

    /// Resize the grid; text reflows to the new width. Returns true when the size changed.
    pub fn resize(&mut self, size: TermSize) -> bool {
        if size == self.size() {
            return false;
        }
        self.term.resize(size);
        self.revision += 1;
        self.trimmed = false;
        self.release_spare_rows();
        true
    }

    /// Rows of history kept at most.
    pub fn scrollback(&self) -> usize {
        self.scrollback
    }

    /// Change the scrollback bound (capped at [`MAX_SCROLLBACK`]); shrinking drops old rows at once.
    pub fn set_scrollback(&mut self, rows: usize) {
        self.scrollback = rows.min(MAX_SCROLLBACK);
        self.term.grid_mut().update_history(self.scrollback);
        self.revision += 1;
        self.trimmed = false;
        self.release_spare_rows();
    }

    /// Rows of history currently held.
    pub fn history_len(&self) -> usize {
        self.term.history_size()
    }

    /// How many rows the view is scrolled up from the bottom (0 follows new output).
    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// Scroll the view by `rows` (positive is up, into history).
    pub fn scroll(&mut self, rows: i32) {
        self.term.scroll_display(Scroll::Delta(rows));
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    /// Scroll so that `offset` rows of history are above the bottom of the view.
    pub fn set_display_offset(&mut self, offset: usize) {
        let delta = offset as i32 - self.display_offset() as i32;
        if delta != 0 {
            self.scroll(delta);
        }
    }

    /// The grid point under a cell of the view (row 0 is the top row on screen).
    pub fn view_point(&self, row: usize, column: usize) -> Point {
        let row = row.min(self.term.screen_lines() - 1);
        Point::new(
            Line(row as i32 - self.display_offset() as i32),
            Column(column.min(self.term.columns() - 1)),
        )
    }

    /// Cells of a row of the view, for drawing.
    pub fn view_row(&self, row: usize) -> &[Cell] {
        let line = Line(row as i32 - self.display_offset() as i32);
        &self.term.grid()[line][..]
    }

    /// A row of the live screen counted back from the newest row with output (0 is the newest),
    /// whatever the view is scrolled to: what the live view under a scrolled-back transcript
    /// shows. The cursor's row counts when it holds text (a prompt); an empty one left behind by
    /// a finished line does not. `None` past the oldest row kept.
    pub fn tail_row(&self, back: usize) -> Option<&[Cell]> {
        let grid = self.term.grid();
        let cursor = grid.cursor.point;
        let mut newest = cursor.line;
        if cursor.column.0 == 0 && grid[newest].line_length().0 == 0 && newest > self.term.topmost_line() {
            newest -= 1i32;
        }
        let line = newest - back as i32;
        (line >= self.term.topmost_line()).then(|| &grid[line][..])
    }

    /// The logical line shown on a row of the view (soft-wrapped rows joined), and the index in
    /// characters of the character at `column` of that row, for finding a link under the pointer.
    pub fn view_line_at(&self, row: usize, column: usize) -> (String, usize) {
        self.line_at(self.view_point(row, column))
    }

    /// The logical line holding a grid point (soft-wrapped rows joined), and the index in
    /// characters of the character at that point.
    pub fn line_at(&self, target: Point) -> (String, usize) {
        let grid = self.term.grid();
        let last = self.term.last_column();
        let mut start = target.line;
        while start > self.term.topmost_line() && grid[start - 1i32][last].flags.contains(Flags::WRAPLINE) {
            start -= 1i32;
        }
        let mut text = String::new();
        let mut index = 0;
        let mut count = 0;
        let mut line = start;
        loop {
            for (col, cell) in grid[line][..].iter().enumerate() {
                if line == target.line && col == target.column.0 {
                    index = count;
                }
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                text.push(if cell.c == '\t' { ' ' } else { cell.c });
                count += 1;
                if text.len() > 16_384 {
                    return (text, index);
                }
            }
            if line >= self.term.bottommost_line() || !grid[line][last].flags.contains(Flags::WRAPLINE) {
                break;
            }
            line += 1i32;
        }
        (text, index)
    }

    /// Where the cursor is in the view, if on screen.
    pub fn view_cursor(&self) -> Option<(usize, usize)> {
        let p = self.term.grid().cursor.point;
        let row = p.line.0 + self.display_offset() as i32;
        (row >= 0 && (row as usize) < self.term.screen_lines()).then_some((row as usize, p.column.0))
    }

    /// Start a selection at `point`; `side` is the half of the cell that was clicked.
    pub fn select_start(&mut self, kind: SelectionKind, point: Point, side: Side) {
        let ty = match kind {
            SelectionKind::Simple => SelectionType::Simple,
            SelectionKind::Word => SelectionType::Semantic,
            SelectionKind::Line => SelectionType::Lines,
        };
        self.term.selection = Some(Selection::new(ty, point, side));
    }

    /// Extend the selection to `point`.
    pub fn select_update(&mut self, point: Point, side: Side) {
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, side);
        }
    }

    pub fn select_clear(&mut self) {
        self.term.selection = None;
    }

    /// Select everything, history included.
    pub fn select_all(&mut self) {
        let start = Point::new(self.term.topmost_line(), Column(0));
        let end = Point::new(self.term.bottommost_line(), self.term.last_column());
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(end, Side::Right);
        self.term.selection = Some(selection);
    }

    /// The selected cells as grid points (for drawing), if anything is selected.
    pub fn selection_range(&self) -> Option<SelectionRange> {
        self.term
            .selection
            .as_ref()
            .filter(|s| !s.is_empty())
            .and_then(|s| s.to_range(&self.term))
    }

    /// The selected text: soft-wrapped rows are joined without a newline, hard line ends keep theirs.
    pub fn selection_text(&self) -> Option<String> {
        self.selection_range()?;
        self.term.selection_to_string().filter(|s| !s.is_empty())
    }

    /// The whole transcript (history and screen) as plain text, built on request.
    pub fn transcript(&self) -> String {
        let start = Point::new(self.term.topmost_line(), Column(0));
        let end = Point::new(self.term.bottommost_line(), self.term.last_column());
        let text = self.term.bounds_to_string(start, end);
        let trimmed = text.trim_end_matches(['\n', ' ']).len();
        let mut text = text;
        text.truncate(trimmed);
        text
    }

    /// Whether the transcript has any text (cheap: history rows count as text, the screen is
    /// read only when there is no history).
    pub fn has_text(&self) -> bool {
        if self.term.history_size() > 0 {
            return true;
        }
        let screen = self.term.screen_lines();
        (0..screen).any(|row| {
            self.term.grid()[Line(row as i32)]
                .into_iter()
                .any(|cell| cell.c != ' ' && cell.c != '\0')
        })
    }

    /// Empty the transcript: history and screen, the cursor back at the top left (Session >
    /// Clear Transcript). The parser keeps any escape sequence that is still incomplete, and the
    /// activity counters keep counting.
    pub fn clear(&mut self) {
        self.term.selection = None;
        self.term.scroll_display(Scroll::Bottom);
        // Clearing the screen moves its rows into the history, so the history goes second.
        Handler::clear_screen(&mut self.term, ClearMode::All);
        Handler::clear_screen(&mut self.term, ClearMode::Saved);
        Handler::goto(&mut self.term, 0, 0);
        self.pending.clear();
        self.revision += 1;
    }

    /// Write the transcript to `path` as UTF-8 text with a final line end (File > Save
    /// Transcript). Replaces the file.
    pub fn save_transcript(&self, path: &std::path::Path) -> std::io::Result<()> {
        let mut text = self.transcript();
        text.push('\n');
        std::fs::write(path, text)
    }

    /// Read access to the underlying alacritty terminal, for drawing.
    pub fn term(&self) -> &Term<VoidListener> {
        &self.term
    }

    /// Approximate heap bytes held by the grid: cells of every allocated row.
    pub fn heap_bytes(&self) -> usize {
        let rows = self.term.grid().total_lines();
        rows * self.term.columns() * std::mem::size_of::<Cell>()
    }
}

#[cfg(test)]
mod tests;
