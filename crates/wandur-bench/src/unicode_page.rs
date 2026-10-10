//! A test page for text rendering: Latin with precomposed and combining accents, Greek, Cyrillic,
//! CJK with a column alignment check, box drawing, symbols, emoji, ANSI colours and attributes,
//! and right-to-left text. `wandur-bench mud-server --page unicode` sends it on connect.

use std::fmt::Write;

pub fn unicode_page() -> String {
    let mut s = String::new();
    let h = |s: &mut String, title: &str| {
        let _ = write!(s, "\r\n\x1b[1;36m== {title} ==\x1b[0m\r\n");
    };
    s.push_str(
        "\x1b[1;33mWandur Unicode test page\x1b[0m (every line ends in CR LF; the | columns should line up)\r\n",
    );

    h(&mut s, "Latin: precomposed, then the same with combining marks");
    s.push_str("café naïve résumé Ångström Øresund straße Œuvre ğüşıöç Łódź\r\n");
    s.push_str("cafe\u{301} nai\u{308}ve re\u{301}sume\u{301} A\u{30a}ngstro\u{308}m n\u{303} o\u{302}\r\n");

    h(&mut s, "Greek and Cyrillic");
    s.push_str("Ελληνικά: Καλημέρα κόσμε    Русский: Здравствуй, мир\r\n");

    h(&mut s, "CJK (two cells per character)");
    s.push_str("中文: 你好，世界！  日本語: こんにちは世界 カタカナ  한국어: 안녕하세요\r\n");
    s.push_str("|漢字かな|  |한국어ab|\r\n");
    s.push_str("|abcdefgh|  |abcdefgh|\r\n");
    s.push_str("|ｆｕｌｌ|  |12345678|\r\n");

    h(&mut s, "Box drawing and blocks");
    s.push_str("┌──────┬──────┐  ╔══════╗  ░░▒▒▓▓██\r\n");
    s.push_str("│ name │ hp   │  ║ room ║  ▀▄▌▐▖▗▘▝\r\n");
    s.push_str("├──────┼──────┤  ╚══════╝  ╭─╮ ╰─╯\r\n");
    s.push_str("│ orc  │ 120  │  |abcdef|\r\n");
    s.push_str("└──────┴──────┘\r\n");

    h(&mut s, "Symbols");
    s.push_str("arrows ← ↑ → ↓ ↔ ⇒   math ∑ √ ∞ ≈ ≠ ≤ ≥ π µ   cards ♠ ♣ ♥ ♦\r\n");
    s.push_str("misc ★ ☆ ☀ ☂ ♪ ✓ ✗ ⚔ ⚑ ☠ € £ ¥ ° ± §   braille ⠁⠃⠉⠙\r\n");
    s.push_str("|★☆✓✗|  |abcd|\r\n");

    h(&mut s, "Emoji (colour emoji are a known limitation)");
    s.push_str("🙂 🐉 🏰 ⚔️ 👍🏽 👨‍👩‍👧\r\n");

    h(&mut s, "ANSI colours");
    s.push_str("normal  ");
    for i in 0..8 {
        let _ = write!(s, "\x1b[3{i}m█ {i} \x1b[0m");
    }
    s.push_str("\r\nbold    ");
    for i in 0..8 {
        let _ = write!(s, "\x1b[1;3{i}m█ {i} \x1b[0m");
    }
    s.push_str("\r\nbright  ");
    for i in 0..8 {
        let _ = write!(s, "\x1b[9{i}m█ {i} \x1b[0m");
    }
    s.push_str("\r\nbg      ");
    for i in 0..8 {
        let _ = write!(s, "\x1b[4{i}m  {i} \x1b[0m");
    }
    s.push_str("\r\n256     ");
    for i in (16..232).step_by(6) {
        let _ = write!(s, "\x1b[48;5;{i}m \x1b[0m");
    }
    s.push_str("\r\ngreys   ");
    for i in 232..256 {
        let _ = write!(s, "\x1b[48;5;{i}m \x1b[0m");
    }
    s.push_str("\r\nrgb     ");
    for i in 0..36 {
        let r = 255 - i * 7;
        let g = i * 7;
        let _ = write!(s, "\x1b[48;2;{r};{g};128m \x1b[0m");
    }
    s.push_str("\r\n");

    h(&mut s, "Attributes");
    s.push_str("plain \x1b[1mbold\x1b[0m \x1b[2mdim\x1b[0m \x1b[3mitalic\x1b[0m \x1b[4munderline\x1b[0m \x1b[7minverse\x1b[0m \x1b[9mstrike\x1b[0m \x1b[1;3;4mall three\x1b[0m\r\n");
    s.push_str("\x1b[1mBold text uses the bundled bold face: The quick brown fox jumps over the lazy dog.\x1b[0m\r\n");
    s.push_str("Regular text for comparison: The quick brown fox jumps over the lazy dog.\r\n");

    h(&mut s, "Right to left (not reordered: shown in logical order)");
    s.push_str("עברית  العربية\r\n");

    h(&mut s, "Wrapping");
    s.push_str("A long line that wraps at the window edge so selection and copy can be checked: ");
    for i in 0..12 {
        let _ = write!(s, "word{i} ");
    }
    s.push_str("end.\r\n\r\n");
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn page_is_valid_and_crlf_terminated() {
        let page = super::unicode_page();
        assert!(page.contains("漢字") && page.contains("┌") && page.contains("\x1b[48;2;"));
        assert!(page.split("\r\n").all(|l| !l.contains('\n')));
    }
}
