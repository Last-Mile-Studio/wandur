//! Deterministic MUD-like output with the same shape as the C# bench's `AnsiGenerator`: room text,
//! channel chatter, combat lines with 16, 256 and truecolor SGR, prompts and the odd long line.
//! The proportions and words match; the random sequence does not (different generator), so the
//! bytes differ while the mix of text and escape sequences is the same.

const WORDS: &[&str] = &[
    "lantern", "river", "stone", "quiet", "forest", "tower", "ember", "glass", "harbor", "willow", "copper", "north",
    "market", "shadow", "silver", "path", "gate", "bridge", "orchard", "mist", "valley", "cart", "rope", "anvil",
    "the", "a", "of", "and", "to", "under", "beside", "toward", "old", "narrow", "bright", "cold", "warm", "small",
];
const CHANNELS: &[&str] = &["gossip", "ooc", "newbie", "trade", "clan"];
const NAMES: &[&str] = &["Talek", "Ilsa", "Morrow", "Benn", "Quill", "Ardo", "Vesk"];

pub struct AnsiGenerator {
    state: u64,
    chat: bool,
}

impl AnsiGenerator {
    pub fn new(seed: u64, chat: bool) -> Self {
        Self {
            state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            chat,
        }
    }

    /// Uniform in `min..max` (exclusive), like .NET `Random.Next(min, max)`.
    fn next(&mut self, min: usize, max: usize) -> usize {
        // xorshift64*
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let r = self.state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        min + (r % (max - min) as u64) as usize
    }

    fn word(&mut self) -> &'static str {
        WORDS[self.next(0, WORDS.len())]
    }

    fn sentence(&mut self, out: &mut String, min: usize, max: usize) {
        let count = self.next(min, max + 1);
        for i in 0..count {
            if i > 0 {
                out.push(' ');
            }
            let w = self.word();
            out.push_str(w);
        }
        out.push('.');
    }

    /// Append one line ending in CR LF.
    pub fn next_line(&mut self, out: &mut String) {
        use std::fmt::Write;
        let kind = self.next(0, 100);
        if kind < 30 {
            out.push_str("\x1b[0;37m");
            self.sentence(out, 8, 16);
            out.push_str("\x1b[0m");
        } else if kind < 45 && !self.chat {
            out.push_str("\x1b[0;37m");
            out.push_str(NAMES[self.next(0, NAMES.len())]);
            out.push_str(" says, '");
            self.sentence(out, 4, 12);
            out.push_str("'\x1b[0m");
        } else if kind < 45 {
            let _ = write!(
                out,
                "\x1b[1;36m[{}]\x1b[0m \x1b[33m{}\x1b[0m: ",
                CHANNELS[self.next(0, CHANNELS.len())],
                NAMES[self.next(0, NAMES.len())]
            );
            self.sentence(out, 4, 12);
        } else if kind < 70 {
            let name = NAMES[self.next(0, NAMES.len())];
            let color = self.next(16, 256);
            let word = self.word();
            let damage = self.next(1, 400);
            let (r, g, b) = (self.next(0, 256), self.next(0, 256), self.next(0, 256));
            let _ = write!(
                out,
                "\x1b[1;31m{name}\x1b[0m hits you with \x1b[38;5;{color}m{word}\x1b[0m for \x1b[1;33m{damage}\x1b[0m damage. \x1b[38;2;{r};{g};{b}m"
            );
            self.sentence(out, 2, 5);
            out.push_str("\x1b[0m");
        } else if kind < 85 {
            let (hp, m, mv) = (self.next(100, 999), self.next(10, 300), self.next(10, 300));
            let _ = write!(out, "\x1b[32m<{hp}hp {m}m {mv}mv>\x1b[0m ");
            self.sentence(out, 1, 4);
        } else if kind < 97 {
            out.push_str("\x1b[1;32m");
            let start = out.len();
            self.sentence(out, 2, 4);
            if let Some(first) = out[start..].chars().next() {
                let upper = first.to_ascii_uppercase();
                out.replace_range(start..start + first.len_utf8(), &upper.to_string());
            }
            out.push_str("\x1b[0m\r\n\x1b[0;37m");
            self.sentence(out, 14, 24);
            out.push_str("\x1b[0m\r\n\x1b[36mExits: north east south.\x1b[0m");
        } else {
            out.push_str("\x1b[35m");
            self.sentence(out, 40, 60);
            out.push_str("\x1b[0m");
        }
        out.push_str("\r\n");
    }

    /// Text chunks of about `chunk` bytes holding at least `lines` display lines.
    pub fn chunks(&mut self, lines: usize, chunk: usize) -> Vec<String> {
        let mut chunks = Vec::new();
        let mut current = String::with_capacity(chunk + 512);
        let mut produced = 0;
        while produced < lines {
            let before = current.len();
            self.next_line(&mut current);
            produced += current[before..].matches('\n').count();
            if current.len() >= chunk {
                chunks.push(std::mem::replace(&mut current, String::with_capacity(chunk + 512)));
            }
        }
        if !current.is_empty() {
            chunks.push(current);
        }
        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_shaped_like_mud_output() {
        let mut a = AnsiGenerator::new(7, true);
        let mut b = AnsiGenerator::new(7, true);
        let (mut x, mut y) = (String::new(), String::new());
        for _ in 0..200 {
            a.next_line(&mut x);
            b.next_line(&mut y);
        }
        assert_eq!(x, y);
        assert!(x.contains("\x1b[38;5;") && x.contains("\x1b[38;2;") && x.contains("hp "));
        assert!(x.contains("\x1b[1;36m["), "chat lines present");
        let mut n = AnsiGenerator::new(7, false);
        let mut z = String::new();
        for _ in 0..200 {
            n.next_line(&mut z);
        }
        assert!(!z.contains("\x1b[1;36m["), "no chat lines");
    }

    #[test]
    fn chunks_hold_the_requested_lines() {
        let chunks = AnsiGenerator::new(1, true).chunks(1000, 4096);
        let lines: usize = chunks.iter().map(|c| c.matches('\n').count()).sum();
        assert!(lines >= 1000);
        assert!(chunks.iter().rev().skip(1).all(|c| c.len() >= 4096));
    }
}
