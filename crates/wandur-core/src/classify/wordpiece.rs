//! A BERT word-piece tokenizer for the classifier's encoder, written from the documented
//! behaviour of the package's `tokenizer.json` (a `BertNormalizer` with text cleaning, CJK
//! spacing, lowercasing and accent stripping; a `BertPreTokenizer`; `WordPiece` with the `##`
//! prefix and 100 characters per word at most). The parity fixtures pin its output.

use std::collections::HashMap;

use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::UnicodeNormalization;

const MAX_CHARS_PER_WORD: usize = 100;

#[derive(Clone, Debug)]
pub struct WordPiece {
    ids: HashMap<String, u32>,
    cls: u32,
    sep: u32,
    unk: u32,
    /// Word pieces per text, `[CLS]` and `[SEP]` included.
    max_pieces: usize,
}

impl WordPiece {
    /// From a vocabulary indexed by id. Fails when `[CLS]`, `[SEP]` or `[UNK]` is missing.
    pub fn new(vocabulary: &[String], max_pieces: usize) -> Option<Self> {
        let ids: HashMap<String, u32> = vocabulary
            .iter()
            .enumerate()
            .map(|(i, t)| (t.clone(), i as u32))
            .collect();
        Some(Self {
            cls: *ids.get("[CLS]")?,
            sep: *ids.get("[SEP]")?,
            unk: *ids.get("[UNK]")?,
            ids,
            max_pieces: max_pieces.max(2),
        })
    }

    /// `[CLS]`, at most `max_pieces - 2` word pieces of the text, `[SEP]` (cut on the right).
    pub fn encode(&self, text: &str) -> Vec<u32> {
        let limit = self.max_pieces - 2;
        let mut out = Vec::with_capacity(64);
        out.push(self.cls);
        'words: for word in pre_tokenize(&normalize(text)) {
            for id in self.pieces(&word) {
                if out.len() > limit {
                    break 'words;
                }
                out.push(id);
            }
        }
        out.push(self.sep);
        out
    }

    /// Greedy longest match first; a word with no full split is one `[UNK]`.
    fn pieces(&self, word: &str) -> Vec<u32> {
        let chars: Vec<(usize, char)> = word.char_indices().collect();
        if chars.len() > MAX_CHARS_PER_WORD {
            return vec![self.unk];
        }
        let mut pieces = Vec::new();
        let mut start = 0;
        let mut candidate = String::new();
        while start < chars.len() {
            let mut end = chars.len();
            let mut found = None;
            while start < end {
                let from = chars[start].0;
                let to = chars.get(end).map_or(word.len(), |c| c.0);
                candidate.clear();
                if start > 0 {
                    candidate.push_str("##");
                }
                candidate.push_str(&word[from..to]);
                if let Some(&id) = self.ids.get(candidate.as_str()) {
                    found = Some(id);
                    break;
                }
                end -= 1;
            }
            match found {
                Some(id) => pieces.push(id),
                None => return vec![self.unk],
            }
            start = end;
        }
        pieces
    }
}

/// The BertNormalizer: drop NUL, U+FFFD and other control or format characters, make every
/// whitespace a space, space out CJK ideographs, strip accents (NFD, marks removed), lowercase.
pub fn normalize(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\0' || c == '\u{FFFD}' || is_control(c) {
            continue;
        }
        if is_whitespace(c) {
            cleaned.push(' ');
        } else if is_cjk(c) {
            cleaned.push(' ');
            cleaned.push(c);
            cleaned.push(' ');
        } else {
            cleaned.push(c);
        }
    }
    let stripped: String = cleaned
        .nfd()
        .filter(|c| get_general_category(*c) != GeneralCategory::NonspacingMark)
        .collect();
    stripped.to_lowercase()
}

/// The BertPreTokenizer: split on whitespace, every punctuation character its own word.
pub fn pre_tokenize(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        if c.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else if is_punctuation(c) {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            words.push(c.to_string());
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

fn is_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r') || get_general_category(c) == GeneralCategory::SpaceSeparator
}

fn is_control(c: char) -> bool {
    if matches!(c, '\t' | '\n' | '\r') {
        return false;
    }
    matches!(
        get_general_category(c),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::Surrogate
            | GeneralCategory::PrivateUse
            | GeneralCategory::Unassigned
    )
}

fn is_punctuation(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(
            get_general_category(c),
            GeneralCategory::ConnectorPunctuation
                | GeneralCategory::DashPunctuation
                | GeneralCategory::OpenPunctuation
                | GeneralCategory::ClosePunctuation
                | GeneralCategory::InitialPunctuation
                | GeneralCategory::FinalPunctuation
                | GeneralCategory::OtherPunctuation
        )
}

fn is_cjk(c: char) -> bool {
    matches!(
        c as u32,
        0x4E00..=0x9FFF
            | 0x3400..=0x4DBF
            | 0x20000..=0x2A6DF
            | 0x2A700..=0x2B73F
            | 0x2B740..=0x2B81F
            | 0x2B920..=0x2CEAF
            | 0xF900..=0xFAFF
            | 0x2F800..=0x2FA1F
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> Vec<String> {
        [
            "[PAD]", "[UNK]", "[CLS]", "[SEP]", "hall", "##s", "the", ".", "cafe", "un", "##known",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn words_split_into_pieces_with_punctuation_apart_and_accents_stripped() {
        let wp = WordPiece::new(&vocab(), 256).unwrap();
        assert_eq!(wp.encode("The HALLS."), [2, 6, 4, 5, 7, 3]);
        assert_eq!(wp.encode("Café\nunknown"), [2, 8, 9, 10, 3]);
        // No full split: the whole word is unknown.
        assert_eq!(wp.encode("hallx"), [2, 1, 3]);
        assert_eq!(wp.encode(&"a".repeat(101)), [2, 1, 3]);
    }

    #[test]
    fn long_texts_are_cut_to_the_piece_limit() {
        let wp = WordPiece::new(&vocab(), 5).unwrap();
        assert_eq!(wp.encode("the the the the the"), [2, 6, 6, 6, 3]);
        assert_eq!(wp.encode("halls halls"), [2, 4, 5, 4, 3]);
    }

    #[test]
    fn controls_vanish_and_whitespace_becomes_a_space() {
        assert_eq!(normalize("A\u{200B}b\u{0}c\u{00A0}D"), "abc d");
        assert_eq!(pre_tokenize("a,b  c"), ["a", ",", "b", "c"]);
        assert_eq!(normalize("北京"), " 北  京 ");
    }
}
