//! Inline completion for the command line, as the C# `CompletionTrie`, `CompletionLearner` and
//! `CompletionSuggester`.
//!
//! A session learns words from the public lines it receives and the commands it sends; the
//! composer then offers the rest of the word being typed (or of an earlier command the draft
//! starts) as muted ghost text. The caller applies the privacy rules: nothing private ever
//! reaches [`Learner::learn`], and [`suggest`] offers nothing while input is private.
//!
//! The trie is bounded in words and nodes. When a bound is crossed, one sweep keeps the newest
//! nine tenths, so a single insert never pays for eviction.

/// Words held by default (the C# bound).
pub const MAX_ENTRIES: usize = 20_000;
/// Trie nodes held by default, the root included (the C# bound).
pub const MAX_NODES: usize = 400_000;
/// The shortest prefix that gets suggestions.
pub const MINIMUM_PREFIX_LENGTH: usize = 2;
/// Longer words are not kept.
pub const MAXIMUM_WORD_LENGTH: usize = 64;
/// Learned tokens are three to thirty-two characters long.
pub const MINIMUM_TOKEN_LENGTH: usize = 3;
pub const MAXIMUM_TOKEN_LENGTH: usize = 32;

/// Case folding for keys: one character in, one character out (as .NET's `ToLowerInvariant` on
/// one UTF-16 unit), so a key and its word have the same number of characters.
#[inline]
fn fold(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

#[derive(Debug)]
struct Entry {
    /// The casing the word was last seen with.
    text: String,
    count: u32,
    sequence: u64,
}

#[derive(Debug, Default)]
struct Node {
    /// Children by folded character. Most nodes have one or two, so a short vector beats a map.
    children: Vec<(char, u32)>,
    entry: Option<Box<Entry>>,
}

/// A case-insensitive prefix trie over the words a session has seen, bounded in entries and
/// nodes. Each word keeps its last seen casing, how often it was seen and a recency stamp the
/// caller supplies.
#[derive(Debug)]
pub struct CompletionTrie {
    nodes: Vec<Node>,
    count: usize,
    max_entries: usize,
    max_nodes: usize,
}

impl Default for CompletionTrie {
    fn default() -> Self {
        Self::new(MAX_ENTRIES, MAX_NODES)
    }
}

impl CompletionTrie {
    pub fn new(max_entries: usize, max_nodes: usize) -> Self {
        Self {
            nodes: vec![Node::default()],
            count: 0,
            max_entries: max_entries.max(1),
            max_nodes: max_nodes.max(2),
        }
    }

    /// Distinct words held.
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Nodes allocated, the root included.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn child(&self, node: u32, key: char) -> Option<u32> {
        self.nodes[node as usize]
            .children
            .iter()
            .find(|(k, _)| *k == key)
            .map(|&(_, n)| n)
    }

    /// The node for `word`, made if missing.
    fn walk_or_make(&mut self, word: &str) -> u32 {
        let mut node = 0u32;
        for c in word.chars() {
            let key = fold(c);
            node = match self.child(node, key) {
                Some(next) => next,
                None => {
                    let next = self.nodes.len() as u32;
                    self.nodes.push(Node::default());
                    self.nodes[node as usize].children.push((key, next));
                    next
                }
            };
        }
        node
    }

    /// Record one sighting of `word`. Words seen together may share a sequence (then frequency
    /// decides between them); a later sequence always wins over an earlier one.
    pub fn insert(&mut self, word: &str, sequence: u64) {
        if word.is_empty() || word.chars().count() > MAXIMUM_WORD_LENGTH {
            return;
        }
        let node = self.walk_or_make(word) as usize;
        match &mut self.nodes[node].entry {
            Some(entry) => {
                entry.count = entry.count.saturating_add(1);
                if sequence >= entry.sequence {
                    entry.sequence = sequence;
                    if entry.text != word {
                        entry.text.clear();
                        entry.text.push_str(word);
                    }
                }
            }
            slot @ None => {
                *slot = Some(Box::new(Entry {
                    text: word.to_string(),
                    count: 1,
                    sequence,
                }));
                self.count += 1;
            }
        }
        if self.count > self.max_entries || self.nodes.len() > self.max_nodes {
            self.sweep();
        }
    }

    /// The node `prefix` leads to, if any.
    fn find(&self, prefix: &str) -> Option<u32> {
        let mut node = 0u32;
        for c in prefix.chars() {
            node = self.child(node, fold(c))?;
        }
        Some(node)
    }

    /// Every entry below `node` (not `node`'s own).
    fn below(&self, node: u32) -> impl Iterator<Item = &Entry> {
        let mut stack: Vec<u32> = self.nodes[node as usize].children.iter().map(|&(_, n)| n).collect();
        std::iter::from_fn(move || {
            while let Some(current) = stack.pop() {
                let n = &self.nodes[current as usize];
                stack.extend(n.children.iter().map(|&(_, c)| c));
                if let Some(entry) = &n.entry {
                    return Some(&**entry);
                }
            }
            None
        })
    }

    /// Words that extend `prefix`, most recent first, then most frequent, then shortest, then
    /// in ordinal order. The prefix itself is never a candidate, and a prefix shorter than two
    /// characters gives nothing.
    pub fn suggest(&self, prefix: &str, limit: usize) -> Vec<String> {
        let length = prefix.chars().count();
        if !(MINIMUM_PREFIX_LENGTH..=MAXIMUM_WORD_LENGTH).contains(&length) || limit == 0 {
            return Vec::new();
        }
        let Some(node) = self.find(prefix) else {
            return Vec::new();
        };
        let mut found: Vec<&Entry> = self.below(node).collect();
        found.sort_by(|a, b| rank(a, b));
        found.into_iter().take(limit).map(|e| e.text.clone()).collect()
    }

    /// The best word that extends `prefix` (what [`Self::suggest`] gives first), without
    /// collecting or sorting the others.
    pub fn best(&self, prefix: &str) -> Option<&str> {
        let length = prefix.chars().count();
        if !(MINIMUM_PREFIX_LENGTH..=MAXIMUM_WORD_LENGTH).contains(&length) {
            return None;
        }
        let node = self.find(prefix)?;
        self.below(node).min_by(|a, b| rank(a, b)).map(|e| e.text.as_str())
    }

    /// Every word held, in no particular order (tests and diagnostics).
    pub fn words(&self) -> Vec<String> {
        let mut words = Vec::with_capacity(self.count);
        words.extend(self.below(0).map(|e| e.text.clone()));
        words
    }

    /// Rebuild from the newest nine tenths of the entries, most recent first, stopping early if
    /// the node budget runs out.
    fn sweep(&mut self) {
        let mut entries: Vec<Box<Entry>> = self.nodes.iter_mut().filter_map(|n| n.entry.take()).collect();
        entries.sort_by(|a, b| b.sequence.cmp(&a.sequence).then(b.count.cmp(&a.count)));
        entries.truncate(self.max_entries * 9 / 10);
        self.nodes.clear();
        self.nodes.push(Node::default());
        self.count = 0;
        let budget = self.max_nodes * 9 / 10;
        for entry in entries {
            if self.nodes.len() >= budget {
                break;
            }
            let node = self.walk_or_make(&entry.text) as usize;
            self.nodes[node].entry = Some(entry);
            self.count += 1;
        }
        self.nodes.shrink_to(self.max_nodes);
    }
}

/// Most recent first, then most frequent, then shortest, then ordinal.
fn rank(a: &Entry, b: &Entry) -> std::cmp::Ordering {
    b.sequence
        .cmp(&a.sequence)
        .then(b.count.cmp(&a.count))
        .then(a.text.chars().count().cmp(&b.text.chars().count()))
        .then(a.text.cmp(&b.text))
}

/// Feeds a session's trie from what the reader sees and sends: one plain line at a time (the
/// terminal grid has already resolved escape sequences and carriage returns).
#[derive(Debug, Default)]
pub struct Learner {
    pub words: CompletionTrie,
    sequence: u64,
}

impl Learner {
    pub fn new(words: CompletionTrie) -> Self {
        Self { words, sequence: 0 }
    }

    /// Lines learned so far; also the recency stamp of the last line.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// One plain line, received or sent. Its words share one recency stamp, so among the words
    /// of a line the one seen most often ranks first.
    pub fn learn(&mut self, line: &str) {
        self.sequence += 1;
        let sequence = self.sequence;
        for token in tokens(line) {
            self.words.insert(token, sequence);
        }
    }
}

/// Letters and digits (Unicode), apostrophes (straight and curly) and hyphens.
#[inline]
fn word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '\'' | '-' | '\u{2019}')
}

/// Runs of letters, digits, apostrophes and hyphens with the punctuation trimmed from both ends,
/// three to thirty-two characters long and not digits alone.
pub fn tokens(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| !word_char(c)).filter_map(|run| {
        let token = run.trim_matches(|c: char| !c.is_alphanumeric());
        let length = token.chars().count();
        if !(MINIMUM_TOKEN_LENGTH..=MAXIMUM_TOKEN_LENGTH).contains(&length) || token.chars().all(|c| c.is_numeric()) {
            None
        } else {
            Some(token)
        }
    })
}

/// Whether `text` starts with `prefix`, ignoring case one character at a time. Returns the byte
/// length of the matched part of `text`.
fn starts_with_folded(text: &str, prefix: &str) -> Option<usize> {
    let mut chars = text.char_indices();
    for p in prefix.chars() {
        let (_, c) = chars.next()?;
        if fold(c) != fold(p) {
            return None;
        }
    }
    Some(chars.next().map_or(text.len(), |(i, _)| i))
}

/// What the composer may offer after the caret, or `None`. Line mode first: when the draft
/// starts an earlier command (ignoring case), the rest of the most recent such command.
/// Otherwise word mode: the rest of the best known word for the word being typed. Nothing while
/// private, with an empty draft, or with the caret anywhere but the end.
pub fn suggest<'a>(
    text: &str,
    caret_at_end: bool,
    private: bool,
    history: &'a [String],
    words: &'a CompletionTrie,
) -> Option<&'a str> {
    if private || text.is_empty() || !caret_at_end {
        return None;
    }
    for line in history.iter().rev() {
        if line.len() > text.len()
            && let Some(at) = starts_with_folded(line, text)
            && at < line.len()
        {
            return Some(&line[at..]);
        }
    }
    let word = &text[text.rfind(' ').map_or(0, |i| i + 1)..];
    if word.chars().count() < MINIMUM_PREFIX_LENGTH {
        return None;
    }
    let best = words.best(word)?;
    let at = starts_with_folded(best, word)?;
    Some(&best[at..])
}

/// Turns raw server text into complete plain lines, as the C# `CompletionLearner.Observe`: a
/// line that a network read cut in half waits for the rest, escape sequences never become
/// words, and text after a bare carriage return replaces what came before it.
#[derive(Debug, Default)]
pub struct LineObserver {
    pending: String,
    /// The plain text of the line being learned (reused).
    line: String,
    /// Text was withheld (private input) since the last line feed: the line it was part of is
    /// not learned.
    broken: bool,
}

/// A line nobody ever ends must not grow without bound (the C# limit).
const MAX_PENDING: usize = 16_384;

impl LineObserver {
    /// Public received text, escape sequences and all; calls `line` for each completed line.
    pub fn observe(&mut self, text: &str, mut line: impl FnMut(&str)) {
        let mut rest = text;
        while let Some(i) = rest.find('\n') {
            self.pending.push_str(&rest[..i]);
            if !self.broken {
                plain(&self.pending, &mut self.line);
                line(&self.line);
            }
            self.pending.clear();
            self.broken = false;
            rest = &rest[i + 1..];
        }
        self.pending.push_str(rest);
        if self.pending.len() > MAX_PENDING {
            self.pending.clear();
            self.broken = true;
        }
    }

    /// Some text was not shown to the learner: drop the line in progress.
    pub fn interrupt(&mut self) {
        self.pending.clear();
        self.broken = true;
    }
}

/// The text a line shows: escape sequences (CSI, OSC, and two-character ones) removed, and only
/// what follows the last carriage return that has text after it.
fn plain(raw: &str, out: &mut String) {
    out.clear();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' || (c == '\u{1b}' && chars.peek() == Some(&'\\')) {
                            if c == '\u{1b}' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' => {
                if chars.peek().is_some() {
                    out.clear();
                }
            }
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
}

/// One piece of a batch handed to the learning thread.
#[derive(Debug)]
enum Part {
    /// Public server text (a range of the batch's text).
    Observe(std::ops::Range<usize>),
    /// Text was withheld here.
    Interrupt,
    /// A whole line the person sent.
    Line(std::ops::Range<usize>),
}

#[derive(Debug, Default)]
struct Batch {
    text: String,
    parts: Vec<Part>,
}

impl Batch {
    fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    fn clear(&mut self) {
        self.text.clear();
        self.parts.clear();
    }

    fn run(&self, observer: &mut LineObserver, shared: &Shared) {
        let learn = |line: &str| {
            let mut learner = shared.learner.lock().unwrap_or_else(|e| e.into_inner());
            learner.learn(line);
        };
        for part in &self.parts {
            match part {
                Part::Observe(range) => observer.observe(&self.text[range.clone()], learn),
                Part::Interrupt => observer.interrupt(),
                Part::Line(range) => learn(&self.text[range.clone()]),
            }
        }
    }
}

/// A session's vocabulary, learned on its own thread. Learning every word of a 1 MB/s flood on
/// the UI thread cost about 0.6 ms a frame (+50%), and reading each completed line back from the
/// grid another 0.2 ms; here the UI thread copies the server text it applied into one buffer per
/// frame and hands it over, and the composer reads the trie under a lock held only while one
/// line is learned.
///
/// Batches go through a bounded queue; when it is full a batch is dropped (completion is a
/// convenience; nothing waits for it). If the thread cannot be started, batches are learned on
/// the caller's thread instead.
pub struct Vocabulary {
    shared: std::sync::Arc<Shared>,
    tx: Option<std::sync::mpsc::SyncSender<Batch>>,
    /// Batches the thread is done with, to be filled again.
    spent: Option<std::sync::mpsc::Receiver<Batch>>,
    batch: Batch,
    spare: Vec<Batch>,
    /// For learning on the caller's thread.
    observer: LineObserver,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[derive(Debug, Default)]
struct Shared {
    learner: std::sync::Mutex<Learner>,
    queued: std::sync::atomic::AtomicU64,
    learned: std::sync::atomic::AtomicU64,
}

/// Batches waiting at most.
const QUEUE: usize = 64;

impl Vocabulary {
    /// A vocabulary with its learning thread, named `name`.
    pub fn spawn(name: &str) -> Self {
        let shared = std::sync::Arc::new(Shared::default());
        let (tx, rx) = std::sync::mpsc::sync_channel::<Batch>(QUEUE);
        let (spent_tx, spent_rx) = std::sync::mpsc::sync_channel::<Batch>(QUEUE);
        let thread_shared = std::sync::Arc::clone(&shared);
        let worker = std::thread::Builder::new().name(name.into()).spawn(move || {
            let mut observer = LineObserver::default();
            while let Ok(batch) = rx.recv() {
                batch.run(&mut observer, &thread_shared);
                thread_shared.learned.fetch_add(1, std::sync::atomic::Ordering::Release);
                let _ = spent_tx.try_send(batch);
            }
        });
        match worker {
            Ok(worker) => Self {
                shared,
                tx: Some(tx),
                spent: Some(spent_rx),
                batch: Batch::default(),
                spare: Vec::new(),
                observer: LineObserver::default(),
                worker: Some(worker),
            },
            Err(_) => Self::inline(),
        }
    }

    /// A vocabulary that learns on the caller's thread (no thread available).
    pub fn inline() -> Self {
        Self {
            shared: Default::default(),
            tx: None,
            spent: None,
            batch: Batch::default(),
            spare: Vec::new(),
            observer: LineObserver::default(),
            worker: None,
        }
    }

    /// Public server text, as received (escape sequences and partial lines included).
    pub fn observe(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let start = self.batch.text.len();
        self.batch.text.push_str(text);
        match self.batch.parts.last_mut() {
            Some(Part::Observe(range)) if range.end == start => range.end = self.batch.text.len(),
            _ => self.batch.parts.push(Part::Observe(start..self.batch.text.len())),
        }
    }

    /// Server text was withheld (input was private): the line it belongs to is not learned.
    pub fn interrupt(&mut self) {
        if !matches!(self.batch.parts.last(), Some(Part::Interrupt)) {
            self.batch.parts.push(Part::Interrupt);
        }
    }

    /// A command the person sent.
    pub fn learn_line(&mut self, line: &str) {
        let start = self.batch.text.len();
        self.batch.text.push_str(line);
        self.batch.parts.push(Part::Line(start..self.batch.text.len()));
    }

    /// Hand what was gathered over to be learned. Returns at once.
    pub fn flush(&mut self) {
        use std::sync::atomic::Ordering;
        if self.batch.is_empty() {
            return;
        }
        let Some(tx) = &self.tx else {
            self.batch.run(&mut self.observer, &self.shared);
            self.batch.clear();
            return;
        };
        if let Some(spent) = &self.spent {
            self.spare.extend(spent.try_iter());
        }
        let mut next = self.spare.pop().unwrap_or_default();
        next.clear();
        let batch = std::mem::replace(&mut self.batch, next);
        self.shared.queued.fetch_add(1, Ordering::Relaxed);
        if let Err(e) = tx.try_send(batch) {
            // Full (or gone): this batch is not learned, and the line it ended in is broken.
            self.shared.queued.fetch_sub(1, Ordering::Relaxed);
            let (std::sync::mpsc::TrySendError::Full(mut batch)
            | std::sync::mpsc::TrySendError::Disconnected(mut batch)) = e;
            batch.clear();
            self.spare.push(batch);
            self.batch.parts.push(Part::Interrupt);
        }
    }

    /// Read the trie (the composer's suggestion), under the lock.
    pub fn with<R>(&self, read: impl FnOnce(&CompletionTrie) -> R) -> R {
        let learner = self.shared.learner.lock().unwrap_or_else(|e| e.into_inner());
        read(&learner.words)
    }

    /// Hand over what was gathered and wait until it is learned (tests), at most `timeout`.
    pub fn settle(&mut self, timeout: std::time::Duration) -> bool {
        use std::sync::atomic::Ordering;
        self.flush();
        let deadline = std::time::Instant::now() + timeout;
        while self.shared.learned.load(Ordering::Acquire) < self.shared.queued.load(Ordering::Relaxed) {
            if std::time::Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        true
    }
}

impl Drop for Vocabulary {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl std::fmt::Debug for Vocabulary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vocabulary")
            .field("threaded", &self.tx.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recency_outranks_frequency_then_frequency_then_length() {
        let mut trie = CompletionTrie::default();
        trie.insert("wombat", 1);
        trie.insert("wombat", 1);
        trie.insert("wombat", 1);
        trie.insert("womprat", 2);
        trie.insert("women", 3);
        trie.insert("womenfolk", 3);
        trie.insert("womenfolk", 3);
        trie.insert("wombs", 3);
        assert_eq!(
            trie.suggest("wom", 10),
            ["womenfolk", "wombs", "women", "womprat", "wombat"]
        );
        assert_eq!(trie.suggest("wom", 2), ["womenfolk", "wombs"]);
        assert_eq!(trie.best("wom"), Some("womenfolk"));
    }

    #[test]
    fn keys_are_case_insensitive_and_the_last_seen_casing_is_kept() {
        let mut trie = CompletionTrie::default();
        trie.insert("Womprat", 1);
        assert_eq!(trie.suggest("wom", 8), ["Womprat"]);
        assert_eq!(trie.suggest("WOM", 8), ["Womprat"]);
        trie.insert("womprat", 2);
        assert_eq!(trie.suggest("Wom", 8), ["womprat"]);
        assert_eq!(trie.len(), 1);
    }

    #[test]
    fn exact_match_is_excluded_and_short_prefixes_give_nothing() {
        let mut trie = CompletionTrie::default();
        trie.insert("look", 1);
        trie.insert("looking", 2);
        assert_eq!(trie.suggest("look", 8), ["looking"]);
        assert!(trie.suggest("looking", 8).is_empty());
        assert!(trie.suggest("l", 8).is_empty());
        assert!(trie.suggest("", 8).is_empty());
        assert!(trie.suggest("zz", 8).is_empty());
        assert_eq!(trie.best("looking"), None);
        assert_eq!(trie.best("l"), None);
    }

    #[test]
    fn eviction_keeps_the_most_recent_entries() {
        let mut trie = CompletionTrie::new(100, MAX_NODES);
        for i in 0..150u64 {
            trie.insert(&format!("word{i:03}"), i);
        }
        assert!(trie.len() <= 100, "{} entries after the sweep", trie.len());
        assert!(trie.suggest("word00", 8).is_empty());
        assert_eq!(trie.suggest("word", 1)[0], "word149");
        let words = trie.words();
        assert!(words.iter().any(|w| w == "word140"));
        assert!(!words.iter().any(|w| w == "word010"));
    }

    #[test]
    fn node_budget_also_triggers_a_sweep() {
        let mut trie = CompletionTrie::new(1000, 20);
        for i in 0..40u64 {
            trie.insert(&format!("abc{i:02}"), i);
        }
        // A sweep may overshoot its budget by the last word it re-inserts, never by more.
        assert!(trie.node_count() <= 25, "{} nodes", trie.node_count());
        assert!(trie.len() < 40, "{} entries", trie.len());
        assert_eq!(trie.suggest("abc", 1)[0], "abc39");
    }

    /// The bound holds under a long stream of distinct words, at the default limits.
    #[test]
    fn the_trie_stays_bounded() {
        let mut trie = CompletionTrie::default();
        let mut learner_seq = 0;
        for i in 0..120_000u64 {
            learner_seq += 1;
            trie.insert(&format!("w{i:x}longerword{}", i % 7), learner_seq);
            assert!(trie.len() <= MAX_ENTRIES);
            assert!(trie.node_count() <= MAX_NODES + MAXIMUM_WORD_LENGTH);
        }
        assert!(trie.len() >= MAX_ENTRIES * 8 / 10, "a sweep keeps most: {}", trie.len());
        assert_eq!(
            trie.best("w1d4bf"),
            Some(format!("w1d4bflongerword{}", 0x1d4bf % 7).as_str())
        );
    }

    #[test]
    fn tokens_are_letters_digits_apostrophes_and_hyphens_of_three_to_thirty_two_characters() {
        let line = format!(
            "A Vicious Womprat's scurry-past, 'quoted' 1234 ab {} x1 don't -dash-",
            "x".repeat(33)
        );
        let found: Vec<&str> = tokens(&line).collect();
        assert_eq!(
            found,
            ["Vicious", "Womprat's", "scurry-past", "quoted", "don't", "dash"]
        );
    }

    #[test]
    fn learning_a_line_gives_its_words_one_stamp() {
        let mut learner = Learner::default();
        learner.learn("A Vicious Womprat scurries past.");
        learner.learn("A bantha wanders by.");
        assert_eq!(learner.words.suggest("wom", 8), ["Womprat"]);
        assert_eq!(learner.words.suggest("ban", 8), ["bantha"]);
        assert_eq!(learner.sequence(), 2);
    }

    fn history(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn line_mode_wins_over_word_mode_and_word_mode_needs_two_characters() {
        let history = history(&["look", "look womprat"]);
        let mut learner = Learner::default();
        learner.learn("A Vicious Womprat scurries past.");
        learner.learn("look womprat");
        let w = &learner.words;
        assert_eq!(suggest("look", true, false, &history, w), Some(" womprat"));
        assert_eq!(suggest("loo", true, false, &history, w), Some("k womprat"));
        assert_eq!(suggest("LOO", true, false, &history, w), Some("k womprat"));
        assert_eq!(suggest("kill wom", true, false, &history, w), Some("prat"));
        assert_eq!(suggest("kill w", true, false, &history, w), None);
        assert_eq!(suggest("look womprat", true, false, &history, w), None);
    }

    #[test]
    fn nothing_while_private_empty_or_with_the_caret_away_from_the_end() {
        let history = history(&["look womprat"]);
        let mut learner = Learner::default();
        learner.learn("A Vicious Womprat scurries past.");
        let w = &learner.words;
        assert_eq!(suggest("look", true, true, &history, w), None);
        assert_eq!(suggest("", true, false, &history, w), None);
        assert_eq!(suggest("look", false, false, &history, w), None);
        assert_eq!(suggest("wom", false, false, &history, w), None);
    }

    #[test]
    fn suggestions_keep_the_seen_casing_and_handle_non_ascii() {
        let mut learner = Learner::default();
        learner.learn("Ärger und Öl im Straßenbahn-Depot");
        let w = &learner.words;
        assert_eq!(suggest("go är", true, false, &[], w), Some("ger"));
        assert_eq!(suggest("STRA", true, false, &[], w), Some("ßenbahn-Depot"));
    }

    /// CompletionTests.ObserveLearnsCompleteLinesWithoutEscapeSequencesAndSkipsRejectedOnes, on
    /// the learning thread: a line cut by a read waits for its end, escape sequences are not
    /// words, and a line that text was withheld from (private input) is not learned.
    #[test]
    fn observe_learns_complete_lines_without_escape_sequences_on_its_thread() {
        let mut v = Vocabulary::spawn("test-vocabulary");
        let wait = std::time::Duration::from_secs(5);
        v.observe("\u{1b}[32mA Vicious Wom");
        assert!(v.settle(wait));
        assert!(v.with(|w| w.suggest("wom", 8)).is_empty());
        v.observe("prat\u{1b}[0m scurries past.\r\nsecret passphrase ");
        v.interrupt();
        v.observe("here\r\nA bantha ");
        assert!(v.settle(wait));
        assert_eq!(v.with(|w| w.suggest("wom", 8)), ["Womprat"]);
        assert!(v.with(|w| w.suggest("passph", 8)).is_empty());
        assert!(v.with(|w| w.suggest("ban", 8)).is_empty());
        v.observe("wanders by.\n\u{1b}]0;title\u{7}Over\rUnder the bridge\n");
        v.learn_line("look zyxxyz");
        assert!(v.settle(wait));
        assert_eq!(v.with(|w| w.suggest("ban", 8)), ["bantha"]);
        assert_eq!(v.with(|w| w.suggest("und", 8)), ["Under"]);
        assert!(v.with(|w| w.suggest("tit", 8)).is_empty() && v.with(|w| w.suggest("ove", 8)).is_empty());
        assert!(v.with(|w| w.words()).contains(&"zyxxyz".to_string()));
        // Many batches: buffers come back and are reused; nothing is lost while the queue keeps up.
        for i in 0..200 {
            v.observe(&format!("marker{i:03} words\n"));
            v.flush();
            if i % 50 == 0 {
                assert!(v.settle(wait));
            }
        }
        assert!(v.settle(wait));
        let mut inline = Vocabulary::inline();
        inline.observe("Inline learning works\n");
        inline.flush();
        assert_eq!(inline.with(|w| w.suggest("inl", 8)), ["Inline"]);
    }
}
