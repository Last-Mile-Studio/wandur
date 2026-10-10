//! Word wrapping at display time.
//!
//! The grid stays the model: alacritty wraps a long line at exactly the column width, and that
//! is what selection, copy, search and line events see. Drawing goes through [`DisplayRow`]s
//! instead of grid rows: a logical line that spans several grid rows (soft wraps) is laid out
//! again, breaking after spaces (and hyphens, and around wide characters) as a MUD client
//! does, and only words longer than the width are cut. Each display row names the cells it
//! shows, so a point on screen maps back to a grid cell and copying still gives the original
//! line.
//!
//! Only normal flowing text is rewrapped: the alternate screen (full-screen programs), lines
//! that fit in one row and very long logical lines (more than [`MAX_WRAP_ROWS`] grid rows)
//! keep the grid's exact columns.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::SelectionRange;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::{Cell, Flags, LineLength};

use crate::Terminal;

/// Logical lines longer than this many grid rows are drawn as the grid has them: laying one out
/// costs a walk over all its cells, and nobody reads a wall of text that long as prose.
pub const MAX_WRAP_ROWS: usize = 64;

/// How the transcript is wrapped for display.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct WrapOptions {
    /// Rewrap soft-wrapped lines at word boundaries.
    pub words: bool,
    /// Columns continuation rows are indented by (0: none). At most a third of the width is used.
    pub indent: usize,
}

/// One row on screen: a run of cells of one logical line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayRow {
    /// The grid line its logical line starts on.
    pub line: Line,
    /// The cells shown, as offsets from the start of the logical line: offset `o` is the cell at
    /// grid line `line + o / columns`, column `o % columns`.
    pub start: usize,
    pub end: usize,
    /// Blank display columns before the first cell (a hanging indent).
    pub indent: usize,
    /// The last row of its logical line (a selection past it includes the line end).
    pub last: bool,
}

/// Display columns a cell takes: spacers take none (the wide character before them counts two).
#[inline]
fn width(cell: &Cell) -> usize {
    if cell
        .flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
    {
        0
    } else if cell.flags.contains(Flags::WIDE_CHAR) {
        2
    } else {
        1
    }
}

#[inline]
fn is_space(cell: &Cell) -> bool {
    cell.c == ' ' || cell.c == '\t'
}

/// Lay out one logical line of `len` cells at `columns` display columns. `cell(o)` reads offset
/// `o`. Rows are pushed as `(start, end, indent)`. Greedy: a row ends at the last break
/// opportunity that fits; spaces at a break stay at the end of the row (they may run past the
/// edge, unseen); a word wider than the row is cut where it reaches the edge.
pub fn wrap_line<'c>(
    len: usize,
    columns: usize,
    indent: usize,
    cell: impl Fn(usize) -> &'c Cell,
    mut push: impl FnMut(usize, usize, usize),
) {
    let columns = columns.max(2);
    let indent = indent.min(columns / 3);
    let mut start = 0;
    let mut room = columns;
    let mut row_indent = 0;
    let mut used = 0;
    // The latest offset after `start` where a row may begin.
    let mut brk = None;
    // A break is allowed before the next visible cell (after a space, a hyphen, a wide character).
    let mut after_break = false;
    let mut prev_alnum = false;
    let mut o = 0;
    while o < len {
        let c = cell(o);
        let w = width(c);
        if w == 0 {
            o += 1;
            continue;
        }
        if is_space(c) {
            used += w;
            after_break = true;
            prev_alnum = false;
            o += 1;
            continue;
        }
        let wide = w == 2;
        if (after_break || wide) && o > start {
            brk = Some(o);
        }
        if used + w > room && o > start {
            let cut = brk.filter(|&b| b > start).unwrap_or(o);
            push(start, cut, row_indent);
            start = cut;
            row_indent = indent;
            room = columns - indent;
            brk = None;
            after_break = false;
            used = (start..o).map(|i| width(cell(i))).sum();
            // Re-examine `o` against the new row.
            continue;
        }
        used += w;
        after_break = wide || (c.c == '-' && prev_alnum);
        prev_alnum = c.c.is_alphanumeric();
        o += 1;
    }
    push(start, len.max(start), row_indent);
}

impl Terminal {
    fn columns_(&self) -> usize {
        self.term.columns()
    }

    /// Whether display rows are rewrapped at all right now (never on the alternate screen).
    pub fn wraps_words(&self, options: WrapOptions) -> bool {
        options.words && !self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    fn wrapped(&self, line: Line) -> bool {
        line < self.term.bottommost_line()
            && self.term.grid()[line][self.term.last_column()]
                .flags
                .contains(Flags::WRAPLINE)
    }

    /// The first and last grid lines of the logical line holding `line`. A logical line longer
    /// than [`MAX_WRAP_ROWS`] is not followed: each of its rows stands alone.
    fn logical_bounds(&self, line: Line) -> (Line, Line) {
        let top = self.term.topmost_line();
        let cap = MAX_WRAP_ROWS as i32;
        let mut start = line;
        while start > top && self.wrapped(start - 1i32) {
            if line.0 - start.0 >= cap {
                return (line, line);
            }
            start -= 1i32;
        }
        let mut end = line;
        while self.wrapped(end) {
            if end.0 - start.0 >= cap {
                return (line, line);
            }
            end += 1i32;
        }
        (start, end)
    }

    /// The display rows of the logical line `start..=end`, top to bottom.
    fn line_rows(&self, start: Line, end: Line, options: WrapOptions, out: &mut Vec<DisplayRow>) {
        let columns = self.columns_();
        let rows = (end.0 - start.0 + 1) as usize;
        if rows == 1 || !self.wraps_words(options) {
            for r in 0..rows {
                out.push(DisplayRow {
                    line: start + r as i32,
                    start: 0,
                    end: columns,
                    indent: 0,
                    last: r + 1 == rows,
                });
            }
            return;
        }
        let grid = self.term.grid();
        let len = (rows - 1) * columns + grid[end].line_length().0;
        let first = out.len();
        wrap_line(
            len,
            columns,
            options.indent,
            |o| &grid[start + (o / columns) as i32][Column(o % columns)],
            |a, b, indent| {
                out.push(DisplayRow {
                    line: start,
                    start: a,
                    end: b,
                    indent,
                    last: false,
                })
            },
        );
        if let Some(last) = out[first..].last_mut() {
            last.last = true;
        }
    }

    /// Display rows ending with grid line `bottom` (a logical line running on below it is cut
    /// after that row), at most `max`, oldest first.
    pub fn display_rows_up(&self, bottom: Line, max: usize, options: WrapOptions, out: &mut Vec<DisplayRow>) {
        out.clear();
        if max == 0 {
            return;
        }
        let columns = self.columns_();
        let top = self.term.topmost_line();
        let bottom = bottom.min(self.term.bottommost_line());
        let mut scratch = Vec::new();
        let mut line = bottom;
        // Built newest first, reversed at the end.
        while line >= top && out.len() < max {
            let (start, end) = self.logical_bounds(line);
            scratch.clear();
            self.line_rows(start, end, options, &mut scratch);
            // Rows of this logical line drawn from cells up to the end of grid line `line`.
            let limit = (line.0 - start.0 + 1) as usize * columns;
            for row in scratch.iter().rev() {
                let row_first = (row.line.0 - start.0) as usize * columns + row.start;
                if row_first >= limit {
                    continue;
                }
                out.push(*row);
                if out.len() == max {
                    break;
                }
            }
            line = start - 1i32;
        }
        out.reverse();
    }

    /// Display rows starting at grid line `top` (a logical line that began above it starts with
    /// the row holding `top`'s first cell), at most `max`, oldest first.
    pub fn display_rows_down(&self, top: Line, max: usize, options: WrapOptions, out: &mut Vec<DisplayRow>) {
        let columns = self.columns_();
        let bottom = self.term.bottommost_line();
        let mut scratch = Vec::new();
        let mut line = top.max(self.term.topmost_line());
        while line <= bottom && out.len() < max {
            let (start, end) = self.logical_bounds(line);
            scratch.clear();
            self.line_rows(start, end, options, &mut scratch);
            let from = (line.0 - start.0) as usize * columns;
            for row in &scratch {
                let row_last = (row.line.0 - start.0) as usize * columns + row.end.max(row.start + 1) - 1;
                if row_last < from {
                    continue;
                }
                out.push(*row);
                if out.len() == max {
                    break;
                }
            }
            line = end + 1i32;
        }
    }

    /// The display rows of the view, `visible` rows tall, oldest first. With word wrapping off
    /// (or on the alternate screen) they are the view's grid rows. Otherwise the view's bottom
    /// row stays at the bottom (the live screen: the cursor's row, or a lower one holding
    /// text) and rewrapping takes room at the top; scrolled all the way up, the oldest row stays
    /// at the top instead, so every row can be reached.
    pub fn view_display_rows(&self, visible: usize, options: WrapOptions, out: &mut Vec<DisplayRow>) {
        out.clear();
        let visible = visible.min(self.term.screen_lines()).max(1);
        let offset = self.display_offset() as i32;
        let columns = self.columns_();
        if !self.wraps_words(options) {
            out.extend((0..visible).map(|r| DisplayRow {
                line: Line(r as i32 - offset),
                start: 0,
                end: columns,
                indent: 0,
                last: true,
            }));
            return;
        }
        if offset > 0 && offset as usize >= self.term.history_size() {
            self.display_rows_down(self.term.topmost_line(), visible, options, out);
            return;
        }
        let bottom = if offset == 0 {
            let grid = self.term.grid();
            let cursor = grid.cursor.point.line;
            let mut last = Line(self.term.screen_lines() as i32 - 1);
            while last > cursor && grid[last].line_length().0 == 0 {
                last -= 1i32;
            }
            last.min(Line(visible as i32 - 1))
        } else {
            Line(visible as i32 - 1 - offset)
        };
        self.display_rows_up(bottom, visible, options, out);
        if out.len() < visible {
            let mut below = Vec::new();
            self.display_rows_down(bottom + 1i32, visible - out.len(), options, &mut below);
            out.append(&mut below);
        }
    }

    /// The newest display rows (what the live view shows while the transcript is scrolled back),
    /// at most `max`, oldest first. The cursor's row counts when it holds text.
    pub fn tail_display_rows(&self, max: usize, options: WrapOptions, out: &mut Vec<DisplayRow>) {
        let grid = self.term.grid();
        let cursor = grid.cursor.point;
        let mut newest = cursor.line;
        if cursor.column.0 == 0 && grid[newest].line_length().0 == 0 && newest > self.term.topmost_line() {
            newest -= 1i32;
        }
        self.display_rows_up(newest, max, options, out);
    }

    /// The grid point of offset `o` of a display row's logical line.
    pub fn row_point(&self, row: &DisplayRow, o: usize) -> Point {
        let columns = self.columns_();
        Point::new(row.line + (o / columns) as i32, Column(o % columns))
    }

    fn offset_of(&self, row: &DisplayRow, p: Point) -> isize {
        (p.line.0 - row.line.0) as isize * self.columns_() as isize + p.column.0 as isize
    }

    fn cell_of(&self, row: &DisplayRow, o: usize) -> &Cell {
        let p = self.row_point(row, o);
        &self.term.grid()[p.line][p.column]
    }

    /// The cells of a display row as grid row slices, each with the display column it starts at.
    /// Cells past the width (spaces left at a break) are not included.
    pub fn row_segments<'a>(&'a self, row: &DisplayRow, out: &mut Vec<(&'a [Cell], usize)>) {
        out.clear();
        let columns = self.columns_();
        let grid = self.term.grid();
        let mut x = row.indent;
        let mut o = row.start;
        while o < row.end && x < columns {
            let line = row.line + (o / columns) as i32;
            if line > self.term.bottommost_line() {
                break;
            }
            let col = o % columns;
            let stop = (col + row.end - o).min(columns);
            let cells = &grid[line][..][col..stop];
            out.push((cells, x));
            x += cells.iter().map(width).sum::<usize>();
            o += stop - col;
        }
    }

    /// The grid point under display column `col` of a row, and the side of that cell. Left of
    /// the row's text is its first cell; right of it, the last (or, on the last row of a logical
    /// line, the blank cells after it, so a selection can take in the line end).
    pub fn display_point(&self, row: &DisplayRow, col: usize, side: Side) -> (Point, Side) {
        let columns = self.columns_();
        if col < row.indent || row.start >= row.end {
            let o = row.start.min(row.end.saturating_sub(1));
            return (self.row_point(row, o), Side::Left);
        }
        let mut x = row.indent;
        let mut last_visible = row.start;
        for o in row.start..row.end {
            let w = width(self.cell_of(row, o));
            if w == 0 {
                continue;
            }
            if col < x + w {
                let side = if w == 2 {
                    if col == x { Side::Left } else { Side::Right }
                } else {
                    side
                };
                return (self.row_point(row, o), side);
            }
            x += w;
            last_visible = o;
        }
        if row.last {
            // Blank cells after the text, within the grid row the text ends on.
            let end = row.end.max(1) - 1;
            let row_end = (end / columns + 1) * columns - 1;
            let o = (row.end + (col - x)).min(row_end);
            (self.row_point(row, o), side)
        } else {
            (self.row_point(row, last_visible), Side::Right)
        }
    }

    /// Display columns `[from, to)` of a row that `range` selects, if any.
    pub fn row_selection(&self, row: &DisplayRow, range: &SelectionRange) -> Option<(usize, usize)> {
        if row.start >= row.end {
            return None;
        }
        let columns = self.columns_();
        let first = row.start as isize;
        let last = row.end as isize - 1;
        let lo = self.offset_of(row, range.start).max(first);
        let hi = self.offset_of(row, range.end);
        if lo > last || hi < first {
            return None;
        }
        let mut x = row.indent;
        let mut from = None;
        let mut to = x;
        for o in row.start..row.end {
            let w = width(self.cell_of(row, o));
            if (o as isize) >= lo && (o as isize) <= hi && w > 0 {
                from.get_or_insert(x);
                to = x + w;
            }
            x += w;
        }
        // The selection runs on past a logical line's last row: the line end is selected too.
        if row.last && hi > last {
            to = columns;
        }
        from.map(|f| (f.min(columns), to.min(columns)))
    }
}
