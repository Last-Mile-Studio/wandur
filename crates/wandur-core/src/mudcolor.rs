//! SMAUG and SWR style colour codes, and ANSI SGR, in short strings that are not transcript
//! text: script panel widgets, gauge captions and table cells (the C# `MudColorCodes`). MSDP
//! strings often still carry them, so a panel shows "A Vicious Womprat" in yellow instead of
//! "&228A Vicious Womprat&D" literally.
//!
//! - `&` sets the foreground and `^` the background: one SMAUG letter, or exactly three digits
//!   naming an xterm 256 colour (the Legends of the Jedi extension, 000 to 255).
//! - `&D` and `&d` reset to the default style. `&&` and `^^` are the literal characters; any
//!   other character after a marker stays as text, marker included.
//! - `ESC [ ... m` is applied as the transcript applies SGR (bold with a basic colour selects the
//!   bright entry); every other escape sequence is dropped.
//!
//! Every string starts from the default style, so a code never leaks into the next widget. The
//! result names palette entries, not colours: the UI resolves them with the theme, as it does
//! for the transcript.

/// A colour as a code names it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Paint {
    /// The widget's own colour.
    #[default]
    Default,
    /// A palette entry: 0 to 15 follow the theme, 16 to 255 the xterm cube and grey ramp.
    Index(u8),
    /// A direct colour (`38;2;r;g;b`).
    Rgb(u8, u8, u8),
}

impl Paint {
    /// The theme palette entry (0 to 15), if this is one: what a theme recolours.
    pub fn palette_index(self) -> Option<u8> {
        match self {
            Paint::Index(i) if i < 16 => Some(i),
            _ => None,
        }
    }
}

/// The style of one run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RunStyle {
    pub fg: Paint,
    pub bg: Paint,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// A piece of text in one style.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Run {
    pub text: String,
    pub style: RunStyle,
}

const ESCAPE: char = '\u{1b}';

/// Whether the text could carry a code, so plain strings keep the plain path.
pub fn has_codes(text: &str) -> bool {
    text.contains(['&', '^', ESCAPE])
}

/// The text without codes or escape sequences (dock titles, accessible names).
pub fn strip(text: &str) -> String {
    if !has_codes(text) {
        return text.to_string();
    }
    parse(text).into_iter().map(|r| r.text).collect()
}

/// The SMAUG letters, on the sixteen palette entries the transcript uses.
fn letter(c: char) -> Option<u8> {
    Some(match c {
        'x' => 0,
        'r' => 1,
        'g' => 2,
        'O' => 3,
        'b' => 4,
        'p' => 5,
        'c' => 6,
        'w' => 7,
        'z' => 8,
        'R' => 9,
        'G' => 10,
        'Y' => 11,
        'B' => 12,
        'P' => 13,
        'C' => 14,
        'W' => 15,
        _ => return None,
    })
}

/// The styled runs of one string. Plain text is one default run; an empty string has none.
pub fn parse(text: &str) -> Vec<Run> {
    if text.is_empty() {
        return Vec::new();
    }
    if !has_codes(text) {
        return vec![Run {
            text: text.to_string(),
            style: RunStyle::default(),
        }];
    }
    let chars: Vec<char> = text.chars().collect();
    let mut p = Parser::default();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        let next = chars.get(i + 1).copied();
        if ch == '&' || ch == '^' {
            if next == Some(ch) {
                p.text.push(ch);
                i += 2;
                continue;
            }
            if let Some(index) = next.and_then(letter) {
                p.set(ch == '&', index);
                i += 2;
                continue;
            }
            if ch == '&' && matches!(next, Some('D' | 'd')) {
                p.reset();
                i += 2;
                continue;
            }
            if i + 3 < chars.len() && chars[i + 1..=i + 3].iter().all(char::is_ascii_digit) {
                let number = chars[i + 1..=i + 3]
                    .iter()
                    .fold(0u32, |n, d| n * 10 + d.to_digit(10).unwrap_or(0));
                if number <= 255 {
                    p.set(ch == '&', number as u8);
                    i += 4;
                    continue;
                }
            }
            p.text.push(ch);
            i += 1;
        } else if ch == ESCAPE {
            if next != Some('[') {
                i += 1;
                continue;
            }
            let mut end = i + 2;
            while end < chars.len() && (chars[end].is_ascii_digit() || chars[end] == ';') {
                end += 1;
            }
            if end >= chars.len() {
                break;
            }
            if chars[end] == 'm' {
                let parameters: String = chars[i + 2..end].iter().collect();
                p.sgr(&parameters);
            }
            i = end + 1;
        } else {
            p.text.push(ch);
            i += 1;
        }
    }
    p.finish()
}

#[derive(Default)]
struct Parser {
    runs: Vec<Run>,
    text: String,
    style: RunStyle,
    /// A basic foreground (30 to 37), which bold turns bright.
    basic: Option<u8>,
}

impl Parser {
    fn flush(&mut self) {
        if !self.text.is_empty() {
            self.runs.push(Run {
                text: std::mem::take(&mut self.text),
                style: self.style,
            });
        }
    }

    fn set(&mut self, foreground: bool, index: u8) {
        self.flush();
        if foreground {
            self.basic = None;
            self.style.fg = Paint::Index(index);
        } else {
            self.style.bg = Paint::Index(index);
        }
    }

    fn reset(&mut self) {
        self.flush();
        self.basic = None;
        self.style = RunStyle::default();
    }

    /// The transcript's SGR rules.
    fn sgr(&mut self, parameters: &str) {
        self.flush();
        let values: Vec<i64> = parameters
            .split(';')
            .map(|part| if part.is_empty() { 0 } else { part.parse().unwrap_or(-1) })
            .collect();
        let mut i = 0;
        while i < values.len() {
            let n = values[i];
            if (30..=37).contains(&n) {
                self.basic = Some((n - 30) as u8);
            } else if n == 0 || n == 39 || (90..=97).contains(&n) {
                self.basic = None;
            }
            let s = &mut self.style;
            match n {
                0 => *s = RunStyle::default(),
                1 => s.bold = true,
                22 => s.bold = false,
                3 => s.italic = true,
                23 => s.italic = false,
                4 => s.underline = true,
                24 => s.underline = false,
                39 => s.fg = Paint::Default,
                49 => s.bg = Paint::Default,
                30..=37 => s.fg = Paint::Index((n - 30) as u8),
                90..=97 => s.fg = Paint::Index((n - 90 + 8) as u8),
                40..=47 => s.bg = Paint::Index((n - 40) as u8),
                100..=107 => s.bg = Paint::Index((n - 100 + 8) as u8),
                _ => {}
            }
            if (n == 38 || n == 48) && i + 1 < values.len() {
                let clamp = |v: i64| v.clamp(0, 255) as u8;
                let mut paint = None;
                if values[i + 1] == 5 && i + 2 < values.len() {
                    paint = Some(Paint::Index(clamp(values[i + 2])));
                    i += 2;
                } else if values[i + 1] == 2 && i + 4 < values.len() {
                    paint = Some(Paint::Rgb(
                        clamp(values[i + 2]),
                        clamp(values[i + 3]),
                        clamp(values[i + 4]),
                    ));
                    i += 4;
                }
                if let Some(paint) = paint {
                    if n == 38 {
                        self.basic = None;
                        self.style.fg = paint;
                    } else {
                        self.style.bg = paint;
                    }
                }
            }
            i += 1;
        }
        if let Some(basic) = self.basic {
            self.style.fg = Paint::Index(basic + if self.style.bold { 8 } else { 0 });
        }
    }

    fn finish(mut self) -> Vec<Run> {
        self.flush();
        self.runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (text, palette foreground, palette background), as the C# tests shape runs.
    fn shape(runs: &[Run]) -> Vec<(&str, Option<u8>, Option<u8>)> {
        runs.iter()
            .map(|r| (r.text.as_str(), r.style.fg.palette_index(), r.style.bg.palette_index()))
            .collect()
    }

    #[test]
    fn plain_text_is_one_default_run() {
        let runs = parse("A plain title");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "A plain title");
        assert_eq!(runs[0].style, RunStyle::default());
        assert!(!has_codes("A plain title"));
        assert!(parse("").is_empty());
    }

    #[test]
    fn every_smaug_letter_maps_to_the_transcript_palette_entry() {
        for (index, letter) in "xrgObpcwzRGYBPCW".chars().enumerate() {
            let runs = parse(&format!("&{letter}text"));
            assert_eq!(runs.len(), 1);
            assert_eq!(runs[0].text, "text");
            assert_eq!(runs[0].style.fg, Paint::Index(index as u8), "{letter}");
            let back = parse(&format!("^{letter}text"));
            assert_eq!(back.len(), 1);
            assert_eq!(back[0].style.bg, Paint::Index(index as u8));
            assert_eq!(back[0].style.fg, Paint::Default);
        }
    }

    #[test]
    fn reset_letters_restore_the_default_foreground_and_background() {
        assert_eq!(
            shape(&parse("&r^bRed&D plain")),
            [("Red", Some(1), Some(4)), (" plain", None, None)]
        );
        assert_eq!(
            shape(&parse("&RRed&d plain")),
            [("Red", Some(9), None), (" plain", None, None)]
        );
    }

    #[test]
    fn three_digit_codes_use_the_xterm_256_table() {
        let runs = parse("&228A Vicious Womprat&D");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "A Vicious Womprat");
        assert_eq!(runs[0].style.fg.palette_index(), None);
        assert_eq!(runs[0].style.fg, Paint::Index(228));
        // Indexes below 16 keep their palette identity so themes recolour them.
        assert_eq!(parse("&009bright red")[0].style.fg.palette_index(), Some(9));
        let back = parse("^017deep blue");
        assert_eq!(back[0].style.bg, Paint::Index(17));
        assert_eq!(back[0].style.fg, Paint::Default);
    }

    #[test]
    fn doubled_markers_are_literal_and_unknown_codes_stay_as_text() {
        for (text, expected) in [
            ("&&", "&"),
            ("^^", "^"),
            ("Tom && Jerry", "Tom & Jerry"),
            ("&", "&"),
            ("&q", "&q"),
            ("&1", "&1"),
            ("&12x", "&12x"),
            ("&999", "&999"),
            ("&256", "&256"),
            ("^", "^"),
            ("^-", "^-"),
            ("50% && rising", "50% & rising"),
        ] {
            let runs = parse(text);
            assert_eq!(runs.len(), 1, "{text}");
            assert_eq!(runs[0].text, expected);
            assert_eq!(runs[0].style, RunStyle::default());
            assert_eq!(strip(text), expected);
        }
    }

    #[test]
    fn codes_never_leak_past_the_end_of_a_string() {
        assert_eq!(shape(&parse("&Rstill red")), [("still red", Some(9), None)]);
        assert_eq!(shape(&parse("next")), [("next", None, None)]);
        assert_eq!(shape(&parse("&Ra&G")), [("a", Some(9), None)]);
        assert!(parse("&R&G&D").is_empty());
    }

    #[test]
    fn adjacent_codes_collapse_and_the_last_one_wins() {
        assert_eq!(shape(&parse("&R&Ggreen")), [("green", Some(10), None)]);
    }

    #[test]
    fn ansi_sgr_sequences_are_applied_like_the_transcript_and_other_escapes_are_stripped() {
        assert_eq!(
            shape(&parse("\x1b[31mred\x1b[0m plain")),
            [("red", Some(1), None), (" plain", None, None)]
        );
        assert_eq!(shape(&parse("\x1b[1;31;44mbright")), [("bright", Some(9), Some(4))]);
        assert_eq!(
            shape(&parse("\x1b[91mbright\x1b[22;31mdim")),
            [("bright", Some(9), None), ("dim", Some(1), None)]
        );
        let extended = parse("\x1b[38;5;228mx\x1b[48;2;1;2;3my\x1b[39;49mz");
        assert_eq!(extended[0].style.fg, Paint::Index(228));
        assert_eq!(extended[1].style.bg, Paint::Rgb(1, 2, 3));
        assert_eq!(extended[2].style, RunStyle::default());
        assert!(!extended[0].style.bold);
        // Cursor movement and a bare escape carry no text at all.
        assert_eq!(strip("\x1b[2Jcle\x1ban\x1b[K"), "clean");
        let styled = parse("\x1b[1;4;3mstyled");
        assert!(styled[0].style.bold && styled[0].style.italic && styled[0].style.underline);
    }

    #[test]
    fn mixed_codes_and_escapes_in_one_string_agree() {
        assert_eq!(
            shape(&parse("&RA\x1b[31mB\x1b[0mC&WD")),
            [
                ("A", Some(9), None),
                ("B", Some(1), None),
                ("C", None, None),
                ("D", Some(15), None)
            ]
        );
    }

    #[test]
    fn strip_removes_every_code_and_keeps_literals() {
        assert_eq!(strip("&228A Vicious Womprat&D"), "A Vicious Womprat");
        assert_eq!(strip("&RTom && &GJerry ^^ up&D"), "Tom & Jerry ^ up");
        assert_eq!(strip("plain"), "plain");
        assert_eq!(strip(""), "");
        assert!(has_codes("&Rx"));
        assert!(has_codes("\x1b[31m"));
        assert!(!has_codes("50% done"));
    }

    #[test]
    fn non_ascii_text_keeps_its_characters() {
        assert_eq!(
            shape(&parse("&Gé🌍&D ok")),
            [("é🌍", Some(10), None), (" ok", None, None)]
        );
    }
}
