use super::*;
use alacritty_terminal::vte::ansi::NamedColor;

fn term(columns: usize, rows: usize, scrollback: usize) -> Terminal {
    Terminal::new(TermSize::new(columns, rows), scrollback)
}

fn row_text(t: &Terminal, row: usize) -> String {
    let mut s: String = t
        .view_row(row)
        .iter()
        .filter(|c| {
            !c.flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        })
        .map(|c| c.c)
        .collect();
    let n = s.trim_end().len();
    s.truncate(n);
    s
}

#[test]
fn bare_line_feed_starts_a_new_line() {
    let mut t = term(20, 5, 100);
    t.feed(b"one\ntwo\r\nthree");
    assert_eq!(row_text(&t, 0), "one");
    assert_eq!(row_text(&t, 1), "two");
    assert_eq!(row_text(&t, 2), "three");
    assert_eq!(t.lines_total(), 2);
    assert_eq!(t.current_line(), "three");
}

#[test]
fn line_events_join_soft_wraps_and_are_opt_in() {
    let mut t = term(10, 4, 100);
    t.feed(b"before\n");
    let mut lines = Vec::new();
    t.take_lines(&mut lines);
    assert!(lines.is_empty(), "off by default");
    t.set_line_events(true);
    t.feed(b"\x1b[31mabcdefghijklmnopqrstuvwxy\x1b[0m\r\nshort\n");
    t.take_lines(&mut lines);
    assert_eq!(
        lines,
        [
            CompletedLine {
                seq: 2,
                text: "abcdefghijklmnopqrstuvwxy".into()
            },
            CompletedLine {
                seq: 3,
                text: "short".into()
            }
        ]
    );
}

#[test]
fn local_text_raises_no_line_events() {
    let mut t = term(30, 4, 100);
    t.set_line_events(true);
    t.feed(b"server one\n");
    t.feed_local("look\n", 8);
    t.feed_local("[Notice]\n", 8);
    t.feed(b"server two\n");
    let mut lines = Vec::new();
    t.take_lines(&mut lines);
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["server one", "server two"]);
    assert_eq!(t.lines_total(), 4);
}

#[test]
fn line_events_are_bounded() {
    let mut t = term(20, 4, 10);
    t.set_line_events(true);
    for i in 0..(MAX_PENDING_LINES + 10) {
        t.feed(format!("{i}\n").as_bytes());
    }
    let mut lines = Vec::new();
    t.take_lines(&mut lines);
    assert_eq!(lines.len(), MAX_PENDING_LINES);
    assert_eq!(t.dropped_lines(), 10);
    assert_eq!(lines.last().unwrap().text, (MAX_PENDING_LINES + 9).to_string());
}

#[test]
fn scrollback_is_bounded_and_counters_keep_growing() {
    let mut t = term(40, 10, 100);
    for i in 0..10_000 {
        t.feed(format!("line {i}\n").as_bytes());
    }
    assert_eq!(t.history_len(), 100);
    assert_eq!(t.lines_total(), 10_000);
    let transcript = t.transcript();
    assert!(transcript.starts_with("line 9891\n"), "{}", &transcript[..40]);
    assert!(transcript.ends_with("line 9999"));
}

#[test]
fn scrollback_memory_does_not_grow_with_volume() {
    let mut t = term(80, 24, 200);
    let line = "\x1b[1;31mThe orc\x1b[0m hits you with \x1b[38;5;200mclub\x1b[0m.\r\n".repeat(50);
    for _ in 0..20 {
        t.feed(line.as_bytes());
    }
    let small = t.heap_bytes();
    for _ in 0..2_000 {
        t.feed(line.as_bytes());
    }
    assert_eq!(t.heap_bytes(), small);
}

#[test]
fn shrinking_the_scrollback_drops_old_rows_and_is_capped() {
    let mut t = term(20, 5, 1_000);
    for i in 0..500 {
        t.feed(format!("{i}\n").as_bytes());
    }
    t.set_scrollback(10);
    assert_eq!(t.history_len(), 10);
    t.set_scrollback(usize::MAX);
    assert_eq!(t.scrollback(), MAX_SCROLLBACK);
    assert_eq!(term(20, 5, usize::MAX).scrollback(), MAX_SCROLLBACK);
}

#[test]
fn resize_reflows_wrapped_text() {
    let mut t = term(10, 5, 100);
    t.feed(b"0123456789abcdefghij\nnext");
    assert_eq!(row_text(&t, 0), "0123456789");
    assert_eq!(row_text(&t, 1), "abcdefghij");
    assert!(t.resize(TermSize::new(30, 5)));
    assert_eq!(row_text(&t, 0), "0123456789abcdefghij");
    assert_eq!(row_text(&t, 1), "next");
    assert!(!t.resize(TermSize::new(30, 5)));
}

#[test]
fn wide_characters_take_two_cells() {
    let mut t = term(10, 3, 10);
    t.feed("漢字ab\n".as_bytes());
    let row = t.view_row(0);
    assert_eq!(row[0].c, '漢');
    assert!(row[0].flags.contains(Flags::WIDE_CHAR));
    assert!(row[1].flags.contains(Flags::WIDE_CHAR_SPACER));
    assert_eq!(row[2].c, '字');
    assert_eq!(row[4].c, 'a');
    assert_eq!(row_text(&t, 0), "漢字ab");
}

#[test]
fn wide_character_at_the_edge_wraps_whole() {
    let mut t = term(5, 3, 10);
    t.set_line_events(true);
    t.feed("abcd漢\n".as_bytes());
    assert_eq!(row_text(&t, 0), "abcd");
    assert_eq!(t.view_row(1)[0].c, '漢');
    let mut lines = Vec::new();
    t.take_lines(&mut lines);
    assert_eq!(lines[0].text, "abcd漢");
}

#[test]
fn combining_marks_stay_with_their_base() {
    let mut t = term(10, 3, 10);
    t.set_line_events(true);
    t.feed("e\u{301}x\n".as_bytes());
    let row = t.view_row(0);
    assert_eq!(row[0].c, 'e');
    assert_eq!(row[0].zerowidth(), Some(&['\u{301}'][..]));
    assert_eq!(row[1].c, 'x');
    let mut lines = Vec::new();
    t.take_lines(&mut lines);
    assert_eq!(lines[0].text, "e\u{301}x");
}

#[test]
fn colours_and_attributes_reach_the_cells() {
    let mut t = term(20, 3, 10);
    t.feed(b"\x1b[1;31mA\x1b[0;38;5;208mB\x1b[48;2;1;2;3mC\x1b[0;4;7mD\x1b[0m");
    let row = t.view_row(0);
    assert_eq!(row[0].fg, Color::Named(NamedColor::Red));
    assert!(row[0].flags.contains(Flags::BOLD));
    assert_eq!(row[1].fg, Color::Indexed(208));
    assert!(!row[1].flags.contains(Flags::BOLD));
    assert!(matches!(row[2].bg, Color::Spec(rgb) if (rgb.r, rgb.g, rgb.b) == (1, 2, 3)));
    assert!(row[3].flags.contains(Flags::UNDERLINE | Flags::INVERSE));
}

#[test]
fn local_echo_keeps_server_state() {
    let mut t = term(30, 4, 10);
    t.feed(b"\x1b[31mred \x1b[");
    t.feed_local("look\n", 8);
    t.feed(b"1mbold");
    let row = t.view_row(0);
    assert_eq!(row[0].fg, Color::Named(NamedColor::Red));
    assert_eq!(row[4].c, 'l');
    assert_eq!(row[4].fg, Color::Indexed(8));
    assert_eq!(row_text(&t, 0), "red look");
    let next = t.view_row(1);
    assert_eq!(next[0].c, 'b');
    assert_eq!(next[0].fg, Color::Named(NamedColor::Red));
    assert!(next[0].flags.contains(Flags::BOLD));
    assert_eq!(t.lines_total(), 1);
}

#[test]
fn carriage_return_and_erase_in_line() {
    let mut t = term(20, 3, 10);
    t.feed(b"Loading 10%\rLoading 99%");
    assert_eq!(t.current_line(), "Loading 99%");
    t.feed(b"\rDone\x1b[K\n");
    assert_eq!(row_text(&t, 0), "Done");
    t.feed(b"abc\x08\x08X");
    assert_eq!(row_text(&t, 1), "aXc");
}

#[test]
fn prompt_line_and_line_start() {
    let mut t = term(20, 3, 10);
    assert!(t.at_line_start());
    t.feed(b"hello\n\x1b[32m<100hp>\x1b[0m ");
    assert!(!t.at_line_start());
    assert_eq!(t.current_line(), "<100hp>");
    t.feed(b"\n");
    assert!(t.at_line_start());
    assert_eq!(t.current_line(), "");
}

#[test]
fn selection_spans_scrollback_and_joins_soft_wraps() {
    let mut t = term(10, 3, 100);
    // "first line here" wraps into two rows; the others are short.
    t.feed(b"first line here\nsecond\nthird\nfourth\nfifth\n");
    // Rows: history "first line", " here", "second", "third"; screen "fourth", "fifth", "".
    assert_eq!(t.history_len(), 4);
    let start = Point::new(Line(-4), Column(6));
    let end = Point::new(Line(1), Column(2));
    t.select_start(SelectionKind::Simple, start, Side::Left);
    t.select_update(end, Side::Right);
    assert_eq!(
        t.selection_text().as_deref(),
        Some("line here\nsecond\nthird\nfourth\nfif")
    );
}

#[test]
fn selection_survives_new_output() {
    let mut t = term(20, 3, 100);
    t.feed(b"alpha\nbeta\ngamma\n");
    let (row, _) = t.view_cursor().unwrap();
    // Select "gamma" on screen.
    let p = t.view_point(row - 1, 0);
    t.select_start(SelectionKind::Simple, p, Side::Left);
    t.select_update(t.view_point(row - 1, 4), Side::Right);
    assert_eq!(t.selection_text().as_deref(), Some("gamma"));
    for i in 0..20 {
        t.feed(format!("more {i}\n").as_bytes());
    }
    assert_eq!(t.selection_text().as_deref(), Some("gamma"));
}

#[test]
fn selection_is_cleared_when_its_rows_leave_the_scrollback() {
    let mut t = term(20, 3, 5);
    t.feed(b"old\n");
    t.select_start(SelectionKind::Simple, Point::new(Line(0), Column(0)), Side::Left);
    t.select_update(Point::new(Line(0), Column(2)), Side::Right);
    assert_eq!(t.selection_text().as_deref(), Some("old"));
    for i in 0..50 {
        t.feed(format!("{i}\n").as_bytes());
    }
    assert_eq!(t.selection_text(), None);
}

#[test]
fn word_and_line_selection() {
    let mut t = term(30, 3, 10);
    t.feed(b"The orc hits you hard.\nnext\n");
    let p = Point::new(Line(0), Column(5));
    t.select_start(SelectionKind::Word, p, Side::Left);
    assert_eq!(t.selection_text().as_deref(), Some("orc"));
    t.select_start(SelectionKind::Line, p, Side::Left);
    assert_eq!(t.selection_text().as_deref(), Some("The orc hits you hard.\n"));
}

#[test]
fn view_points_follow_the_scroll_offset() {
    let mut t = term(20, 3, 100);
    for i in 0..10 {
        t.feed(format!("{i}\n").as_bytes());
    }
    t.scroll(2);
    assert_eq!(t.display_offset(), 2);
    assert_eq!(t.view_point(0, 0), Point::new(Line(-2), Column(0)));
    assert_eq!(row_text(&t, 0), "6");
    // New output while scrolled up keeps the view on the same text.
    t.feed(b"10\n");
    assert_eq!(t.display_offset(), 3);
    assert_eq!(row_text(&t, 0), "6");
    t.scroll_to_bottom();
    assert_eq!(t.display_offset(), 0);
    assert!(t.view_cursor().is_some());
}

#[test]
fn select_all_copies_the_transcript() {
    let mut t = term(10, 3, 100);
    t.feed(b"one\ntwo wraps here\nthree");
    t.select_all();
    assert_eq!(t.selection_text().unwrap().trim_end(), "one\ntwo wraps here\nthree");
    assert_eq!(t.transcript(), "one\ntwo wraps here\nthree");
}

#[test]
fn utf8_split_across_feeds() {
    let mut t = term(10, 3, 10);
    let bytes = "é漢".as_bytes();
    for split in 0..bytes.len() {
        let mut t2 = term(10, 3, 10);
        t2.feed(&bytes[..split]);
        t2.feed(&bytes[split..]);
        assert_eq!(t2.current_line(), "é漢", "split {split}");
    }
    t.feed(bytes);
    assert_eq!(t.current_line(), "é漢");
}

#[test]
fn spare_rows_are_released_once_the_history_is_full() {
    let mut t = term(100, 20, 1_500);
    for i in 0..5_000 {
        t.feed(format!("{i}\n").as_bytes());
    }
    // 1,500 history rows plus 20 screen rows (the freed spares are checked by `wandur-bench micro`).
    assert_eq!(t.term().grid().total_lines(), 1_520);
    assert!(t.transcript().ends_with("4999"));
    // Still works after the release: more output rotates the full ring.
    t.feed(b"after\n");
    assert!(t.transcript().ends_with("after"));
    assert_eq!(t.history_len(), 1_500);
}

#[test]
fn clear_empties_history_and_screen_and_output_continues_at_the_top() {
    let mut t = term(20, 4, 100);
    assert!(!t.has_text());
    for i in 0..30 {
        t.feed(format!("line {i}\n").as_bytes());
    }
    t.feed(b"\x1b[31mprompt> ");
    t.select_all();
    assert!(t.has_text() && t.history_len() > 0);
    let lines = t.lines_total();
    t.clear();
    assert!(!t.has_text());
    assert_eq!(t.transcript(), "");
    assert_eq!(t.history_len(), 0);
    assert!(t.selection_text().is_none());
    assert_eq!(t.view_cursor(), Some((0, 0)));
    assert_eq!(t.lines_total(), lines, "activity counters keep counting");
    t.feed(b"after\nnext");
    assert_eq!(t.transcript(), "after\nnext");
}

#[test]
fn save_transcript_writes_the_grid_text() {
    let mut t = term(12, 3, 100);
    t.feed("first line\nwraps across rows here\n\x1b[32mgreen\x1b[0m é漢\n> ".as_bytes());
    let path = std::env::temp_dir().join(format!("wandur-term-transcript-{}.txt", std::process::id()));
    t.save_transcript(&path).unwrap();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert_eq!(saved, format!("{}\n", t.transcript()));
    assert!(
        saved.starts_with("first line\nwraps across rows here\ngreen é漢\n>"),
        "{saved:?}"
    );
    t.clear();
    t.save_transcript(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "\n");
    let _ = std::fs::remove_file(&path);
}

fn cells_text(cells: &[Cell]) -> String {
    let s: String = cells.iter().map(|c| c.c).collect();
    s.trim_end().to_string()
}

#[test]
fn blinking_text_is_marked_and_every_way_of_ending_it_clears_the_mark() {
    let mut t = term(30, 3, 10);
    t.feed(b"a\x1b[5mb\x1b[25mc\x1b[6;31md\x1b[0me\x1b[5;1mf\x1b[22mg");
    let row = t.view_row(0);
    let blinking: Vec<bool> = row[..7].iter().map(|c| c.flags.contains(BLINK)).collect();
    assert_eq!(blinking, [false, true, false, true, false, true, true]);
    // The other attributes still work alongside it.
    assert!(row[3].fg == Color::Named(NamedColor::Red));
    assert!(row[5].flags.contains(Flags::BOLD) && !row[6].flags.contains(Flags::BOLD));
    // Local echo does not inherit it.
    t.feed(b"\x1b[5m");
    t.feed_local("x", 3);
    let row = t.view_row(0);
    assert!(!row[7].flags.contains(BLINK));
}

#[test]
fn the_tail_rows_follow_the_newest_output_whatever_the_view_shows() {
    let mut t = term(20, 4, 100);
    for i in 0..30 {
        t.feed(format!("Line {i}\n").as_bytes());
    }
    // The empty row the last line feed left behind is not shown.
    assert_eq!(t.tail_row(0).map(cells_text).as_deref(), Some("Line 29"));
    assert_eq!(t.tail_row(5).map(cells_text).as_deref(), Some("Line 24"));
    t.scroll(10);
    assert_eq!(t.tail_row(0).map(cells_text).as_deref(), Some("Line 29"));
    // A prompt with no line feed is the newest row.
    t.feed(b"Crossroads >");
    assert_eq!(t.tail_row(0).map(cells_text).as_deref(), Some("Crossroads >"));
    assert_eq!(t.tail_row(1).map(cells_text).as_deref(), Some("Line 29"));
    // New output keeps the reader's place in the scrolled-back view.
    let top = cells_text(t.view_row(0));
    t.feed(b"\nmore\n");
    assert_eq!(cells_text(t.view_row(0)), top);
    assert_eq!(t.tail_row(0).map(cells_text).as_deref(), Some("more"));
    assert!(t.tail_row(500).is_none());
}

#[test]
fn the_line_under_a_cell_joins_soft_wraps() {
    let mut t = term(10, 4, 100);
    t.feed(b"see https://example.org/x ok\n");
    // Row 1, column 2 is the 13th character of the logical line.
    let (text, index) = t.view_line_at(1, 2);
    assert!(text.starts_with("see https://example.org/x ok"), "{text:?}");
    assert_eq!(index, 12);
    let (_, index) = t.view_line_at(0, 0);
    assert_eq!(index, 0);
}

// Word wrapping at display time (wrap.rs).

const WORDS: WrapOptions = WrapOptions { words: true, indent: 0 };

fn display_text(t: &Terminal, row: &DisplayRow) -> String {
    let mut segments = Vec::new();
    t.row_segments(row, &mut segments);
    let mut s = " ".repeat(row.indent);
    for (cells, _) in &segments {
        for c in cells.iter() {
            if !c
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                s.push(c.c);
            }
        }
    }
    let n = s.trim_end().len();
    s.truncate(n);
    s
}

fn display_width(t: &Terminal, row: &DisplayRow) -> usize {
    let mut segments = Vec::new();
    t.row_segments(row, &mut segments);
    let text = display_text(t, row);
    let wide = text.chars().filter(|c| !c.is_ascii()).count();
    text.chars().count() + wide
}

fn view_texts(t: &Terminal, options: WrapOptions) -> Vec<String> {
    let mut rows = Vec::new();
    t.view_display_rows(t.size().rows, options, &mut rows);
    rows.iter()
        .map(|r| display_text(t, r))
        .filter(|s| !s.is_empty())
        .collect()
}

#[test]
fn word_wrap_breaks_after_spaces_not_inside_words() {
    let mut t = term(16, 6, 100);
    t.feed(b"the quick brown fox jumps over the lazy dog\n");
    assert_eq!(row_text(&t, 1), "fox jumps over t", "the grid wraps at the column");
    assert_eq!(
        view_texts(&t, WORDS),
        ["the quick brown", "fox jumps over", "the lazy dog"]
    );
    let off = WrapOptions {
        words: false,
        indent: 0,
    };
    assert_eq!(
        view_texts(&t, off),
        ["the quick brown", "fox jumps over t", "he lazy dog"]
    );
}

#[test]
fn word_wrap_cuts_only_words_longer_than_the_width() {
    let mut t = term(10, 6, 100);
    t.feed(b"abcdefghijklmnopqrstuvwxy end\n");
    assert_eq!(view_texts(&t, WORDS), ["abcdefghij", "klmnopqrst", "uvwxy end"]);
    // A hyphen is a break opportunity too.
    let mut t = term(12, 6, 100);
    t.feed(b"well-known lantern\n");
    assert_eq!(view_texts(&t, WORDS), ["well-known", "lantern"]);
    let mut t = term(8, 6, 100);
    t.feed(b"far-reaching\n");
    assert_eq!(view_texts(&t, WORDS), ["far-", "reaching"]);
}

#[test]
fn word_wrap_keeps_each_cells_colour_across_a_break() {
    let mut t = term(12, 6, 100);
    t.feed(b"\x1b[31mred words here\x1b[0m and plain\n");
    let mut rows = Vec::new();
    t.view_display_rows(6, WORDS, &mut rows);
    let texts: Vec<String> = rows.iter().map(|r| display_text(&t, r)).collect();
    assert_eq!(&texts[..3], ["red words", "here and", "plain"]);
    let mut segments = Vec::new();
    t.row_segments(&rows[1], &mut segments);
    let cells: Vec<&Cell> = segments.iter().flat_map(|(c, _)| c.iter()).collect();
    assert_eq!(cells[0].c, 'h');
    assert_eq!(
        cells[0].fg,
        Color::Named(NamedColor::Red),
        "the red run continues on the next row"
    );
    assert_eq!(cells[3].fg, Color::Named(NamedColor::Red));
    assert_eq!(cells[5].c, 'a');
    assert_eq!(cells[5].fg, Color::Named(NamedColor::Foreground));
    // The row is drawn from two grid rows: the end of the first and the start of the second.
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].1, 0);
    assert_eq!(segments[1].1, 2, "the second slice starts after 'he'");
}

#[test]
fn word_wrap_never_splits_a_wide_character() {
    let mut t = term(10, 6, 100);
    t.feed("ab 漢字漢字漢字 end\n".as_bytes());
    let mut rows = Vec::new();
    t.view_display_rows(6, WORDS, &mut rows);
    for row in &rows {
        assert!(display_width(&t, row) <= 10, "{:?}", display_text(&t, row));
    }
    let texts: Vec<String> = rows
        .iter()
        .map(|r| display_text(&t, r))
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(texts.join("").replace(' ', ""), "ab漢字漢字漢字end");
    // A wide character is one point: either half maps to its first cell.
    let (p, side) = t.display_point(&rows[0], 4, Side::Left);
    assert!(t.term().grid()[p.line][p.column].flags.contains(Flags::WIDE_CHAR));
    assert_eq!(side, Side::Right);
}

#[test]
fn word_wrap_reflows_on_resize() {
    let text = "a rider in a grey cloak arrives from the west and asks the keeper for news of the river road";
    let mut t = term(30, 10, 100);
    t.feed(format!("{text}\n").as_bytes());
    let narrow = view_texts(&t, WORDS);
    assert!(narrow.iter().all(|r| r.len() <= 30));
    assert_eq!(narrow.join(" "), text);
    t.resize(TermSize::new(50, 10));
    let wide = view_texts(&t, WORDS);
    assert!(wide.len() < narrow.len());
    assert!(wide.iter().all(|r| r.len() <= 50));
    assert_eq!(wide.join(" "), text);
    for row in &wide {
        assert!(!row.ends_with(char::is_alphabetic) || text.contains(&format!("{row} ")) || text.ends_with(row));
    }
}

#[test]
fn copying_across_word_wrapped_rows_gives_the_original_line() {
    let text = "Talek gossips: has anyone restocked with hunted goods at the market yet";
    let mut t = term(24, 10, 100);
    t.feed(format!("before\n{text}\nafter\n").as_bytes());
    let mut rows = Vec::new();
    t.view_display_rows(10, WORDS, &mut rows);
    let texts: Vec<String> = rows.iter().map(|r| display_text(&t, r)).collect();
    let first = texts.iter().position(|s| s.starts_with("Talek")).unwrap();
    let last = texts.iter().position(|s| s.ends_with("yet")).unwrap();
    assert!(last > first + 1);
    // Drag from the first cell of the line to past the end of its last display row.
    let (start, side) = t.display_point(&rows[first], 0, Side::Left);
    t.select_start(SelectionKind::Simple, start, side);
    let (end, side) = t.display_point(&rows[last], 23, Side::Right);
    t.select_update(end, side);
    assert_eq!(t.selection_text().unwrap(), format!("{text}\n").trim_end_matches('\n'));
    // Each display row's highlight covers its text, and the last one runs to the edge.
    let range = t.selection_range().unwrap();
    for row in &rows[first..last] {
        let (from, to) = t.row_selection(row, &range).unwrap();
        assert_eq!(from, 0);
        assert!(to >= display_text(&t, row).len());
    }
    assert_eq!(t.row_selection(&rows[last], &range), Some((0, 24)));
    assert_eq!(t.row_selection(&rows[first - 1], &range), None);
    // A selection inside one display row that sits across a grid wrap.
    let row = rows[first + 1];
    let (a, sa) = t.display_point(&row, 0, Side::Left);
    t.select_start(SelectionKind::Simple, a, sa);
    let (b, sb) = t.display_point(&row, display_text(&t, &row).len() - 1, Side::Right);
    t.select_update(b, sb);
    assert_eq!(t.selection_text().unwrap(), display_text(&t, &row));
}

#[test]
fn the_alternate_screen_keeps_exact_columns() {
    let mut t = term(16, 6, 100);
    t.feed(b"\x1b[?1049hthe quick brown fox jumps over");
    assert!(!t.wraps_words(WORDS));
    let mut rows = Vec::new();
    t.view_display_rows(6, WORDS, &mut rows);
    assert_eq!(rows.len(), 6);
    for (r, row) in rows.iter().enumerate() {
        assert_eq!((row.line, row.start, row.end, row.indent), (Line(r as i32), 0, 16, 0));
    }
    assert_eq!(display_text(&t, &rows[1]), "fox jumps over");
    t.feed(b"\x1b[?1049l");
    assert!(t.wraps_words(WORDS));
}

#[test]
fn a_hanging_indent_shifts_continuation_rows() {
    let mut t = term(16, 6, 100);
    t.feed(b"the quick brown fox jumps over the lazy dog\n");
    let options = WrapOptions { words: true, indent: 2 };
    let mut rows = Vec::new();
    t.view_display_rows(6, options, &mut rows);
    assert_eq!(rows[0].indent, 0);
    assert_eq!(rows[1].indent, 2);
    for row in &rows {
        assert!(display_width(&t, row) <= 16);
    }
    assert_eq!(
        view_texts(&t, options),
        ["the quick brown", "  fox jumps over", "  the lazy dog"]
    );
    // Left of the indent is the row's first cell.
    let (p, _) = t.display_point(&rows[1], 0, Side::Right);
    assert_eq!(t.term().grid()[p.line][p.column].c, 'f');
}

#[test]
fn rewrapped_rows_keep_the_newest_at_the_bottom_and_the_oldest_reachable() {
    let mut t = term(16, 4, 100);
    for i in 0..10 {
        t.feed(format!("line {i} with a few more words\n").as_bytes());
    }
    // Live: the cursor's (empty) row is the bottom row, the newest text just above it.
    let live = view_texts(&t, WORDS);
    assert_eq!(live.last().unwrap(), "few more words");
    assert_eq!(live[live.len() - 2], "line 9 with a");
    // Scrolled all the way up: the oldest row is at the top.
    t.scroll(1_000);
    let top = view_texts(&t, WORDS);
    assert_eq!(top[0], "line 0 with a");
    // A short transcript starts at the top.
    let mut t = term(16, 8, 100);
    t.feed(b"one\ntwo words that wrap here\n");
    assert_eq!(view_texts(&t, WORDS), ["one", "two words that", "wrap here"]);
}

#[test]
fn the_live_view_rows_end_with_the_newest_text() {
    let mut t = term(16, 4, 100);
    for i in 0..10 {
        t.feed(format!("line {i} with a few more words\n").as_bytes());
    }
    t.scroll(5);
    let mut rows = Vec::new();
    t.tail_display_rows(3, WORDS, &mut rows);
    let texts: Vec<String> = rows.iter().map(|r| display_text(&t, r)).collect();
    assert_eq!(texts, ["few more words", "line 9 with a", "few more words"]);
}
