//! Server text as plain lines, for history: the text rules of the C# `AnsiTerminal` without its
//! styles. Escape sequences are dropped (CSI, and OSC, DCS, PM and APC strings), a carriage
//! return moves to the start of the line so later text overwrites it, backspace steps back, a tab
//! goes to the next multiple of four, `ESC[K` erases to the end of the line (`ESC[2K` the whole
//! line), `ESC[2J`/`ESC[3J` forget everything. A line stops growing at [`MAX_LINE`] characters
//! and is marked truncated until the next line feed.

/// Longest line kept, in characters (the C# parser's limit).
pub const MAX_LINE: usize = 4096;
/// Longest CSI parameter text read; longer sequences are skipped.
const MAX_SEQUENCE: usize = 128;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Text,
    Escape,
    Csi,
    DiscardCsi,
    Osc,
    OscEscape,
}

/// A completed line, and whether it reached [`MAX_LINE`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Default)]
pub struct LineParser {
    current: Vec<char>,
    cursor: usize,
    truncated: bool,
    state: State,
    sequence: String,
}

impl LineParser {
    /// Feed server text; completed lines are pushed to `out`.
    pub fn append(&mut self, text: &str, out: &mut Vec<Line>) {
        for c in text.chars() {
            match self.state {
                State::Text => match c {
                    '\u{1b}' => self.state = State::Escape,
                    '\n' => self.new_line(out),
                    '\r' => self.cursor = 0,
                    '\u{8}' => self.cursor = self.cursor.saturating_sub(1),
                    '\t' => {
                        for _ in 0..4 - self.cursor % 4 {
                            self.put(' ');
                        }
                    }
                    c if c.is_control() => {}
                    c => self.put(c),
                },
                State::Escape => {
                    self.state = match c {
                        '[' => {
                            self.sequence.clear();
                            State::Csi
                        }
                        ']' | 'P' | '^' | '_' => State::Osc,
                        _ => State::Text,
                    }
                }
                State::Csi => {
                    if ('@'..='~').contains(&c) {
                        self.apply_csi(c);
                        self.state = State::Text;
                    } else if self.sequence.len() < MAX_SEQUENCE {
                        self.sequence.push(c);
                    } else {
                        self.state = State::DiscardCsi;
                    }
                }
                State::DiscardCsi => {
                    if ('@'..='~').contains(&c) {
                        self.state = State::Text;
                    }
                }
                State::Osc => match c {
                    '\u{7}' => self.state = State::Text,
                    '\u{1b}' => self.state = State::OscEscape,
                    _ => {}
                },
                State::OscEscape => self.state = if c == '\\' { State::Text } else { State::Osc },
            }
        }
    }

    /// Text the client wrote itself (a sent command): printable characters and line feeds only,
    /// so it cannot finish a server escape sequence or run a control.
    pub fn append_local(&mut self, text: &str, out: &mut Vec<Line>) {
        for c in text.chars() {
            if c == '\n' {
                self.new_line(out);
            } else if !c.is_control() {
                self.put(c);
            }
        }
    }

    /// The line in progress.
    pub fn current(&self) -> String {
        self.current.iter().collect()
    }

    /// Whether the line in progress reached [`MAX_LINE`] (sticky until a line feed or clear,
    /// even if it was erased shorter since).
    pub fn current_truncated(&self) -> bool {
        self.truncated
    }

    /// Forget the line in progress and any half-read escape sequence.
    pub fn clear(&mut self) {
        self.current.clear();
        self.cursor = 0;
        self.truncated = false;
        self.state = State::Text;
        self.sequence.clear();
    }

    fn put(&mut self, c: char) {
        if self.cursor >= MAX_LINE {
            self.truncated = true;
            return;
        }
        if self.cursor < self.current.len() {
            self.current[self.cursor] = c;
        } else {
            self.current.push(c);
        }
        self.cursor += 1;
    }

    fn new_line(&mut self, out: &mut Vec<Line>) {
        out.push(Line {
            text: self.current(),
            truncated: self.truncated,
        });
        self.current.clear();
        self.cursor = 0;
        self.truncated = false;
    }

    fn apply_csi(&mut self, command: char) {
        // The first parameter, as `int.TryParse` would read it (0 when it is not a number).
        let first = self
            .sequence
            .split(';')
            .next()
            .and_then(|p| p.parse::<i64>().ok())
            .unwrap_or(0);
        match command {
            'K' if first == 0 => self.current.truncate(self.cursor.min(self.current.len())),
            'K' if first == 2 => {
                self.current.clear();
                self.cursor = 0;
            }
            'J' if first == 2 || first == 3 => {
                self.current.clear();
                self.cursor = 0;
                self.truncated = false;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(parser: &mut LineParser, text: &str) -> Vec<String> {
        let mut out = Vec::new();
        parser.append(text, &mut out);
        out.into_iter().map(|l| l.text).collect()
    }

    #[test]
    fn escapes_are_dropped_and_lines_join_across_chunks() {
        let mut p = LineParser::default();
        assert!(lines(&mut p, "A silver fre").is_empty());
        assert_eq!(lines(&mut p, "ighter\r\nEcho: hun\u{1b}[3"), ["A silver freighter"]);
        assert_eq!(
            lines(&mut p, "2mter2\u{1b}[0m is hidden.\r\n"),
            ["Echo: hunter2 is hidden."]
        );
        assert_eq!(
            lines(&mut p, "\u{1b}]8;;http://x\u{7}link\u{1b}]8;;\u{1b}\\\n"),
            ["link"]
        );
    }

    #[test]
    fn carriage_return_backspace_tab_and_erase() {
        let mut p = LineParser::default();
        assert_eq!(lines(&mut p, "abcdef\rXY\n"), ["XYcdef"]);
        assert_eq!(lines(&mut p, "abcdef\rXY\u{1b}[K\n"), ["XY"]);
        assert_eq!(lines(&mut p, "abc\u{8}\u{8}Z\n"), ["aZc"]);
        assert_eq!(lines(&mut p, "a\tb\n"), ["a   b"]);
        assert_eq!(lines(&mut p, "gone\u{1b}[2Kkept\n"), ["kept"]);
    }

    #[test]
    fn long_lines_stop_at_the_cap_and_stay_marked() {
        let mut p = LineParser::default();
        let mut out = Vec::new();
        p.append(&format!("{}\r{}\u{1b}[K", "x".repeat(5000), "y".repeat(10)), &mut out);
        assert!(out.is_empty());
        assert!(p.current_truncated());
        assert_eq!(p.current(), "y".repeat(10));
        p.append("\n", &mut out);
        assert!(out[0].truncated);
        assert!(!p.current_truncated());
        p.append_local(&format!("{}\n", "z".repeat(MAX_LINE)), &mut out);
        assert!(!out[1].truncated);
        assert_eq!(out[1].text.len(), MAX_LINE);
    }

    #[test]
    fn local_text_cannot_finish_a_server_sequence() {
        let mut p = LineParser::default();
        let mut out = Vec::new();
        p.append_local("look\u{1b}[31m\n", &mut out);
        assert_eq!(out[0].text, "look[31m");
    }
}
