//! Mudlet trigger and alias patterns as the JavaScript regular expressions Wandur's host API
//! matches with (the C# `MudletRegex`). Plain-text pattern types are escaped. Perl-style (PCRE)
//! expressions are carried over where JavaScript means the same thing, with the few common
//! spellings that differ rewritten (a leading `(?i)`, `\A`, `\z`, Python-style named groups).
//! Constructs JavaScript lacks, such as possessive quantifiers, atomic groups, recursion and
//! inline option changes, are refused rather than approximated.

/// A pattern the script engine can run: a JavaScript regular expression source and its flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptPattern {
    pub source: String,
    pub flags: String,
}

impl ScriptPattern {
    pub fn new(source: impl Into<String>, flags: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            flags: flags.into(),
        }
    }
}

/// Plain text matched as it is, optionally anchored at the start and the end of the line.
pub fn literal(text: &str, from_start: bool, to_end: bool) -> ScriptPattern {
    let mut source = String::with_capacity(text.len() + 4);
    if from_start {
        source.push('^');
    }
    for c in text.chars() {
        if matches!(
            c,
            '.' | '*' | '+' | '?' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' | '/'
        ) {
            source.push('\\');
        }
        source.push(c);
    }
    if to_end {
        source.push('$');
    }
    ScriptPattern::new(source, "")
}

/// A Perl-style pattern as JavaScript, or `None` when it uses something JavaScript lacks.
pub fn from_perl(pattern: &str) -> Option<ScriptPattern> {
    let units = pattern.encode_utf16().count();
    if units == 0 || units > 4000 {
        return None;
    }
    let mut chars: Vec<char> = pattern.chars().collect();
    let mut flags = String::new();
    // A leading (?imsx) becomes flags; x has no JavaScript form.
    if chars.len() > 3 && chars[0] == '(' && chars[1] == '?' {
        let options: Vec<char> = chars[2..]
            .iter()
            .take_while(|c| matches!(c, 'i' | 'm' | 's' | 'x'))
            .copied()
            .collect();
        if !options.is_empty() && chars.get(2 + options.len()) == Some(&')') {
            if options.contains(&'x') {
                return None;
            }
            let mut sorted = options.clone();
            sorted.sort_unstable();
            sorted.dedup();
            flags = sorted.into_iter().collect();
            chars.drain(..3 + options.len());
        }
    }
    let at = |i: usize| chars.get(i).copied().unwrap_or('\0');
    let mut output = String::with_capacity(pattern.len() + 8);
    let mut in_class = false;
    let mut after_quantifier = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut quantifier = false;
        if c == '\\' {
            if i + 1 >= chars.len() {
                return None;
            }
            i += 1;
            let e = chars[i];
            match e {
                'A' if !in_class => output.push('^'),
                'Z' | 'z' if !in_class => output.push('$'),
                'e' => output.push_str("\\x1b"),
                'a' => output.push_str("\\x07"),
                'Q' | 'E' | 'G' | 'K' | 'R' | 'X' | 'C' | 'h' | 'H' | 'v' | 'V' | 'N' | 'g' | 'o' | 'p' | 'P' | 'A'
                | 'Z' | 'z' => return None,
                'x' if at(i + 1) == '{' => return None,
                _ => {
                    output.push('\\');
                    output.push(e);
                }
            }
        } else if in_class {
            if c == '[' && matches!(at(i + 1), ':' | '=' | '.') {
                return None;
            }
            if c == ']' {
                in_class = false;
            }
            output.push(c);
        } else if c == '[' {
            in_class = true;
            output.push(c);
            // A ] right after [ or [^ is a literal in PCRE but ends an empty class in JavaScript.
            if at(i + 1) == '^' {
                output.push('^');
                i += 1;
            }
            if at(i + 1) == ']' {
                output.push_str("\\]");
                i += 1;
            }
        } else if c == '(' && at(i + 1) == '?' {
            let rest: String = chars[i + 2..].iter().collect();
            let consumed;
            if rest.starts_with("<=") || rest.starts_with("<!") {
                output.push_str("(?");
                output.push_str(&rest[..2]);
                consumed = 4;
            } else if rest.starts_with([':', '=', '!']) {
                output.push_str("(?");
                output.push(rest.chars().next().unwrap_or(':'));
                consumed = 3;
            } else if let Some((name, length)) = named_group(&rest) {
                output.push_str("(?<");
                output.push_str(&name);
                output.push('>');
                consumed = 2 + length;
            } else if let Some((name, length)) = named_reference(&rest) {
                output.push_str("\\k<");
                output.push_str(&name);
                output.push('>');
                consumed = 2 + length;
            } else if rest.starts_with('#')
                && let Some(end) = chars[i..].iter().position(|&ch| ch == ')')
            {
                consumed = end + 1;
            } else {
                return None;
            }
            i += consumed;
            after_quantifier = false;
            continue;
        } else {
            if matches!(c, '*' | '+' | '?' | '}') {
                // A + after a quantifier makes it possessive in PCRE; JavaScript has no such thing.
                if after_quantifier && c == '+' {
                    return None;
                }
                quantifier = !(after_quantifier && c == '?');
            }
            output.push(c);
        }
        after_quantifier = quantifier;
        i += 1;
    }
    if in_class {
        return None;
    }
    Some(ScriptPattern::new(output, flags))
}

/// `P<name>`, `<name>` or `'name'` after `(?`: the name and how many characters it took.
fn named_group(rest: &str) -> Option<(String, usize)> {
    let prefix = if rest.starts_with("P<") {
        2
    } else if rest.starts_with(['<', '\'']) {
        1
    } else {
        return None;
    };
    let name = identifier(&rest[prefix..])?;
    // As in C#, either closing character ends any opening one.
    rest[prefix + name.len()..]
        .starts_with(['>', '\''])
        .then(|| (name.to_string(), prefix + name.len() + 1))
}

/// `P=name)` after `(?`: a back-reference by name.
fn named_reference(rest: &str) -> Option<(String, usize)> {
    let after = rest.strip_prefix("P=")?;
    let name = identifier(after)?;
    after[name.len()..]
        .starts_with(')')
        .then(|| (name.to_string(), 2 + name.len() + 1))
}

/// A group name: a letter or underscore, then up to 31 letters, digits or underscores.
fn identifier(text: &str) -> Option<&str> {
    let mut end = 0;
    for (index, c) in text.char_indices() {
        let ok = if index == 0 {
            c.is_ascii_alphabetic() || c == '_'
        } else {
            c.is_ascii_alphanumeric() || c == '_'
        };
        if !ok || index >= 32 {
            break;
        }
        end = index + c.len_utf8();
    }
    (end > 0).then(|| &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C# `PerlPatternsAreRewrittenWhereJavaScriptDiffers`.
    #[test]
    fn perl_patterns_are_rewritten_where_javascript_differs() {
        for (perl, source, flags) in [
            (r"(?i)^Wren", "^Wren", "i"),
            (r"\Aone\z", "^one$", ""),
            (r"(?P<who>\w+) waves (?P=who)", r"(?<who>\w+) waves \k<who>", ""),
            (r"[]a]x", r"[\]a]x", ""),
            (r"a(?#note)b", "ab", ""),
            (r"(?:a|b)+?c", "(?:a|b)+?c", ""),
            (
                r"^(\w+) arrives from the (?P<dir>\w+)\.$",
                r"^(\w+) arrives from the (?<dir>\w+)\.$",
                "",
            ),
        ] {
            assert_eq!(from_perl(perl), Some(ScriptPattern::new(source, flags)), "{perl}");
        }
    }

    /// C# `PerlOnlyConstructsAreRefused`.
    #[test]
    fn perl_only_constructs_are_refused() {
        for perl in [
            "a++b",
            "(?>atomic)",
            r"\Qliteral\E",
            "(?R)",
            "[[:alpha:]]",
            "a(?i)b",
            "(?x) spaced",
            r"\p{L}",
            "[unclosed",
            "",
        ] {
            assert_eq!(from_perl(perl), None, "{perl}");
        }
    }

    #[test]
    fn literal_text_escapes_every_metacharacter() {
        let pattern = literal("a.b*c(d)[e]{f}|g^h$i\\j/k+l?", true, true);
        assert_eq!(pattern.source, r"^a\.b\*c\(d\)\[e\]\{f\}\|g\^h\$i\\j\/k\+l\?$");
        assert_eq!(literal("lantern", false, false).source, "lantern");
    }
}
